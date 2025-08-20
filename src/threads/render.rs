use crate::daemon::Daemon;
use crate::pithos::commands::{RenderThreadCommand, ScrollCommand};
use crate::pithos::config::DaemonConfig;
use crate::threads::Thread;
use crate::wayland::render_base::{OutputRenderStateVariety, RenderThreadState, WaylandGlobals};

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, Weak};
use std::thread;

use wayrs_client::{Connection, IoMode};

pub struct WallpaperThreadHandle {
    pub inbox: Arc<Sender<RenderThreadCommand>>,
    receiv: Arc<Mutex<Receiver<RenderThreadCommand>>>,
}

impl Thread for WallpaperThreadHandle {
    fn new() -> Arc<WallpaperThreadHandle> {
        let (host_sender, thread_receiver) = channel::<RenderThreadCommand>();
        return Arc::new(WallpaperThreadHandle {
            inbox: Arc::new(host_sender),
            receiv: Arc::new(Mutex::new(thread_receiver)),
        });
    }

    fn start(&self, daemon: Weak<dyn Daemon + Sync + Send>, config: DaemonConfig) {
        let cmd_queue = self.receiv.clone();
        match thread::Builder::new()
            .name("wallpaper renderer".to_string())
            .spawn(move || {
                let thread = WallpaperThread {
                    pandora: daemon,
                    cmd_queue,
                    config,
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
    config: DaemonConfig,
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
        };
        self.verbose("getting initial output states".to_string());
        thread_state.get_outputs(&mut conn);
        self.verbose("initializing wallpaper states".to_string());
        // thought: peek at self.cmd_queue => pull initial scroll events, if any, for initial position state?
        crate::wayland::render_base::initialize_wallpaper_outputs(
            &mut conn,
            &self.config,
            &mut thread_state,
        );
        self.log("entering draw loop".to_string());
        self.draw_loop(&mut conn, &mut thread_state);
    }

    fn draw_loop(&self, conn: &mut Connection<RenderThreadState>, state: &mut RenderThreadState) {
        loop {
            conn.flush(IoMode::Blocking).unwrap();
            let received_events = conn.recv_events(IoMode::NonBlocking);

            conn.dispatch_events(state);
            self.handle_inbound_commands(conn, state);

            if received_events.is_err() {
                // did not process any animation commands this tick; block on command queue lazy style
                if !state.is_animating() {
                    // not currently animating; block on an inbound event
                    match self.cmd_queue.lock() {
                        Ok(queue) => self.handle_cmd(
                            conn,
                            state,
                            &queue
                                .recv()
                                .expect("thread exploded during blocking read on inbound commands"),
                        ),
                        Err(e) => return self.log(format!("{e:?}")),
                    }
                }
            }
        }
    }

    fn handle_inbound_commands(
        &self,
        conn: &mut Connection<RenderThreadState>,
        state: &mut RenderThreadState,
    ) {
        match self.cmd_queue.lock() {
            Ok(queue) => loop {
                match queue.try_recv() {
                    Ok(cmd) => self.handle_cmd(conn, state, &cmd),
                    Err(_) => break,
                }
            },
            Err(e) => return self.log(format!("{e:?}")),
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
            RenderThreadCommand::Render(_) => {
                // could just handle as discard old render_state & make new . . . with new scroll position? hmmmmmmm
                //self.render(c, state).expect("error handling render command");
                todo!();
            }
            RenderThreadCommand::Scroll(cmd) => {
                self.scroll(conn, state, cmd);
            } // reload config :/
        }
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

        if let Some(OutputRenderStateVariety::Wallpaper(render_state)) =
            output_state.render_state.as_mut()
        {
            render_state.scroll(conn, cmd.position_x, cmd.position_y);
        } else {
            self.debug("received scroll command, but no wallpaper state found on attached outputs. reseat pending/workspace change from output disconnect?".to_string());
        }
    }
}
