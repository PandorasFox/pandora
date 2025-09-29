use crate::daemon::Daemon;
use crate::pithos::commands::{RenderCommand, RenderThreadCommand, ScrollCommand};
use crate::pithos::config::DaemonConfig;
use crate::threads::Thread;
use crate::wayland::render_base::{OutputRenderStateVariety, RenderThreadState, WaylandGlobals};

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, RwLock, Weak};
use std::thread;

use wayrs_client::{Connection, IoMode};

pub struct WallpaperThreadHandle {
    pub inbox: Arc<Sender<RenderThreadCommand>>,
    receiv: Arc<Mutex<Receiver<RenderThreadCommand>>>,
}

impl Thread for WallpaperThreadHandle {
    fn new() -> Arc<WallpaperThreadHandle> {
        let (host_sender, thread_receiver) = channel::<RenderThreadCommand>();
        Arc::new(WallpaperThreadHandle {
            inbox: Arc::new(host_sender),
            receiv: Arc::new(Mutex::new(thread_receiver)),
        })
    }

    fn start(&self, daemon: Weak<dyn Daemon + Sync + Send>, config: DaemonConfig) {
        let cmd_queue = self.receiv.clone();
        match thread::Builder::new()
            .name("wallpaper renderer".to_string())
            .spawn(move || {
                let thread = WallpaperThread {
                    pandora: daemon,
                    cmd_queue,
                    config: Arc::new(RwLock::new(config)),
                };
                thread.start();
            }) {
            Ok(_) => (), // [tf2 medic voice] i will live forever!
            Err(e) => panic!("could not spawn niri ipc handler thread: {e:?}"),
        };
    }
}

struct WallpaperThread {
    pandora: Weak<dyn Daemon>,
    cmd_queue: Arc<Mutex<Receiver<RenderThreadCommand>>>,
    config: Arc<RwLock<DaemonConfig>>,
}

impl WallpaperThread {
    fn log(&self, msg: String) {
        self.pandora.upgrade().unwrap().log("wallpaper", msg);
    }
    fn debug(&self, msg: String) {
        self.pandora.upgrade().unwrap().debug("wallpaper", msg);
    }
    fn verbose(&self, msg: String) {
        self.pandora.upgrade().unwrap().verbose("wallpaper", msg);
    }

    pub fn start(&self) {
        let mut conn = Connection::<RenderThreadState>::connect().unwrap();
        let mut thread_state = RenderThreadState {
            outputs: Vec::new(),
            detached_outputs: Vec::new(),
            globals: WaylandGlobals::new(&mut conn),
            pandora: self.pandora.clone(),
            reseat_needed: false,
        };
        self.verbose("getting initial output states".to_string());
        thread_state.get_outputs(&mut conn);
        self.verbose("initializing wallpaper states".to_string());
        let config = self.config.read().unwrap();
        crate::wayland::render_base::initialize_wallpaper_outputs(
            &mut conn,
            &config,
            &mut thread_state,
            &self.collect_scroll_events(),
        );
        drop(config);
        self.log("entering draw loop :3".to_string());
        self.draw_loop(&mut conn, &mut thread_state);
    }

    fn draw_loop(&self, conn: &mut Connection<RenderThreadState>, state: &mut RenderThreadState) {
        loop {
            conn.flush(IoMode::Blocking).unwrap();
            let received_events = conn.recv_events(IoMode::NonBlocking);

            conn.dispatch_events(state);

            // Handle output reseating outside of dispatch context
            if state.reseat_needed {
                state.reseat_needed = false;
                state.try_reseat_outputs(conn);
            }

            self.handle_inbound_commands(conn, state);

            if received_events.is_err() {
                // did not process any animation commands this tick; block on command queue lazy style
                if !state.is_animating() {
                    // not currently animating; block on an inbound event
                    self.verbose("parking at wait for inbound command".to_string());
                    match self.cmd_queue.lock() {
                        Ok(queue) => self.handle_cmd(
                            conn,
                            state,
                            &queue
                                .recv()
                                .expect("thread exploded during blocking read on inbound commands"),
                        ),
                        Err(e) => self.log(format!("{e:?}")),
                    }
                }
            }
        }
    }

    fn collect_scroll_events(&self) -> HashMap<String, (f64, f64)> {
        let mut scroll_states = HashMap::new();

        match self.cmd_queue.lock() {
            Ok(queue) => loop {
                match queue.try_recv() {
                    Ok(RenderThreadCommand::Scroll(scroll_cmd)) => {
                        scroll_states.insert(
                            scroll_cmd.output.clone(),
                            (scroll_cmd.position_x, scroll_cmd.position_y),
                        );
                    }
                    Ok(_) => break,  // Non-scroll command, put it back and stop
                    Err(_) => break, // No more commands
                }
            },
            Err(_) => return scroll_states,
        }
        scroll_states
    }

    fn handle_inbound_commands(
        &self,
        conn: &mut Connection<RenderThreadState>,
        state: &mut RenderThreadState,
    ) {
        match self.cmd_queue.lock() {
            Ok(queue) => {
                while let Ok(cmd) = queue.try_recv() {
                    self.handle_cmd(conn, state, &cmd)
                }
            }
            Err(e) => self.log(format!("{e:?}")),
        }
    }

    fn handle_cmd(
        &self,
        conn: &mut Connection<RenderThreadState>,
        state: &mut RenderThreadState,
        cmd: &RenderThreadCommand,
    ) {
        self.verbose(format!("command: {:?}", cmd));
        match cmd {
            RenderThreadCommand::Render(cmd) => {
                self.render(conn, state, cmd);
            }
            RenderThreadCommand::Scroll(cmd) => {
                self.scroll(conn, state, cmd);
            }
            RenderThreadCommand::ConfigReload(new_config) => {
                self.config_reload(state, new_config);
            }
            RenderThreadCommand::Poke => (),
        }
    }

    fn render(
        &self,
        conn: &mut Connection<RenderThreadState>,
        state: &mut RenderThreadState,
        cmd: &RenderCommand,
    ) {
        self.verbose(format!(
            "rendering new image {} for output {}",
            cmd.image, cmd.output
        ));

        // Read config once at the beginning
        let config = self.config.read().unwrap();
        let slowdown = config.animation.slowdown.max(0.001);
        drop(config); // Release the lock early

        // Find the target output
        let (_output, output_state) = match state
            .outputs
            .iter_mut()
            .find(|(_, os)| os.name == cmd.output)
        {
            Some((o, os)) => (o, os),
            None => {
                return self.debug(format!(
                    "could not find output {} in state vec for render op",
                    cmd.output
                ));
            }
        };

        let scroll_position = Some((cmd.position.0, cmd.position.1));

        let temp_output_state = crate::wayland::render_base::OutputState {
            name: output_state.name.clone(),
            width: output_state.width,
            height: output_state.height,
            done: output_state.done,
            transform: output_state.transform,
            render_state: crate::wayland::render_base::OutputRenderStateVariety::None,
        };

        // Update existing wallpaper state with new image
        if let OutputRenderStateVariety::Wallpaper(wallpaper_state) =
            &mut output_state.render_state
        {
            wallpaper_state.update_image(
                conn,
                state.globals,
                state.pandora.clone(),
                &cmd.image,
                cmd.mode,
                &temp_output_state,
                scroll_position,
                slowdown,
            );
        } else {
            self.log(format!("received render command for output {} but no existing wallpaper state found - try reseat?", cmd.output));
            return;
        }

        self.verbose(format!(
            "successfully updated wallpaper for output {} with image {} at position ({}, {})",
            cmd.output, cmd.image, cmd.position.0, cmd.position.1
        ));
    }

    fn config_reload(&self, state: &mut RenderThreadState, new_config: &DaemonConfig) {
        self.debug("reloading render thread config".to_string());

        // Update our stored config
        if let Ok(mut config) = self.config.write() {
            *config = new_config.clone();
        }

        // Update slowdown values in all existing wallpaper states
        let new_slowdown = new_config.animation.slowdown.max(0.001);

        for (_, output_state) in &mut state.outputs {
            if let OutputRenderStateVariety::Wallpaper(wallpaper_state) =
                &mut output_state.render_state
            {
                wallpaper_state.slowdown = new_slowdown;
                self.verbose(format!(
                    "updated slowdown to {} for output {}",
                    new_slowdown, output_state.name
                ));
            }
        }

        self.log(format!(
            "render thread config reloaded, slowdown set to {}",
            new_slowdown
        ));
    }

    fn scroll(
        &self,
        conn: &mut Connection<RenderThreadState>,
        state: &mut RenderThreadState,
        cmd: &ScrollCommand,
    ) {
        let output_name = cmd.output.clone();
        let output_state = match state
            .outputs
            .iter_mut()
            .find(|(_, os)| os.name == output_name)
        {
            Some((_, os)) => os,
            None => {
                return self.debug("could not find output in state vec for scroll op".to_string());
            }
        };

        if let OutputRenderStateVariety::Wallpaper(render_state) =
            &mut output_state.render_state
        {
            render_state.scroll(conn, cmd.position_x, cmd.position_y);
        } else {
            self.debug("received scroll command, but no wallpaper state found on attached outputs. reseat pending/workspace change from output disconnect?".to_string());
        }
    }
}
