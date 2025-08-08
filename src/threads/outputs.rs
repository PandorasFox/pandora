use ::pandora::pithos::{config::DaemonConfig, commands::{CommandType, DaemonCommand, ModeCommand, RenderCommand, RenderMode, RenderThreadCommand, StopCommand}};

use std::sync::{Arc, Mutex, Weak, mpsc::{channel, Receiver, Sender}};
use std::thread;

use wayrs_client::global::GlobalExt;
use wayrs_client::protocol::wl_output::{self, WlOutput};
use wayrs_client::protocol::wl_registry::{self, GlobalArgs};
use wayrs_client::{Connection, EventCtx, IoMode};

use crate::pandora::Pandora;

// generic wayland agent for handling output plug/unplug events
pub struct OutputHandler {
    config: Option<DaemonConfig>,
    cmd_queue: Arc<Mutex<Receiver<DaemonCommand>>>,
    pub queue: Sender<DaemonCommand>,
}

impl OutputHandler {
    pub fn new(config: DaemonConfig) -> Arc<OutputHandler> {
        let (send, recv) = channel::<DaemonCommand>();
        return Arc::new(OutputHandler {
            config: Some(config),
            cmd_queue: Arc::new(Mutex::new(recv)),
            queue: send,
        });
    }

    pub fn start(&self, weak: Weak<Pandora>) {
        let pandora = weak.upgrade().unwrap();
        let config = self.config.clone().unwrap();
        let cmd_queue = self.cmd_queue.clone();
        match thread::Builder::new().name("output handler".to_string())
        .spawn(move || {
            run(config, pandora, cmd_queue);
        }) {
            Ok(_) => (), // [tf2 medic voice] i will live forever!
            Err(e) => panic!("could not spawn output events handler thread: {e:?}"),
        }
    }
}   

fn run(config: DaemonConfig, pandora: Arc<Pandora>, cmd_queue: Arc<Mutex<Receiver<DaemonCommand>>>) {
    let mut conn = Connection::connect().unwrap();
    let mut state = State::default();
    state.config = config;
    state.pandora = Some(pandora);
    conn.add_registry_cb(wl_registry_cb);
    loop {
        conn.flush(IoMode::Blocking).unwrap();
        conn.recv_events(IoMode::Blocking).unwrap();
        conn.dispatch_events(&mut state);
        match cmd_queue.lock() {
            Ok(channel) => {
                match channel.try_recv() {
                    Ok(cmd) => {
                        match cmd {
                            DaemonCommand::ReloadConfig(config) => {
                                state.config = config;
                            }
                            _ => (),
                        }
                    }
                    Err(_) => (), // ??
                }
            },
            Err(e) => {
                log(format!("error acquiring channel lock: {e:?}"));
            }
        }
    }
}

#[derive(Default)]
struct State {
    outputs: Vec<Output>,
    config: DaemonConfig,
    pandora: Option<Arc<Pandora>>,
}

#[derive(Debug)]
struct Output {
    registry_name: u32,
    wl_output: WlOutput,
    name: Option<String>,
    done: bool,
}

impl Output {
    fn bind(conn: &mut Connection<State>, global: &GlobalArgs) -> Self {
        Self {
            registry_name: global.name,
            wl_output: global.bind_with_cb(conn, 3..=4, wl_output_cb).unwrap(),
            name: None,
            done: false,
        }
    }
}

fn wl_registry_cb(conn: &mut Connection<State>, state: &mut State, event: &wl_registry::Event) {
    match event {
        wl_registry::Event::Global(global) if global.is::<WlOutput>() => {
            // kms lol
            state.outputs.push(Output::bind(conn, global));
        },
        wl_registry::Event::GlobalRemove(name) => {
            if let Some(i) = state.outputs.iter().position(|o| o.registry_name == *name) {
                let mut output = state.outputs.swap_remove(i);
                let output_name = output.name.take().unwrap();
                let cmd = RenderThreadCommand::Stop(StopCommand {
                    output: output_name,
                });
                let _ = state.pandora.as_ref().unwrap().handle_cmd(&CommandType::Tc(cmd));
                output.wl_output.release(conn);
            }
        },
        _ => (),
    }
}

fn wl_output_cb(ctx: EventCtx<State, WlOutput>) {
    let pandora = ctx.state.pandora.as_ref().unwrap();
    let output = &mut ctx
        .state
        .outputs
        .iter_mut()
        .find(|o| o.wl_output == ctx.proxy)
        .expect("could not find matching wl_output in vec");

    match ctx.event {
        wl_output::Event::Mode(new_mode) => {
            if output.done { // do not try to dispatch this during initial startup
                let output_name = output.name.as_ref().unwrap().clone();
                let config_outputs = &ctx.state.config.outputs;
                let output_config = match config_outputs.iter().find(|oc| oc.name == output_name) {
                    Some(conf) => conf,
                    None => {
                        log(format!("could not find a config stanza for {output_name}, ignoring"));
                        return;
                    },
                };
                let mode = match output_config.mode {
                    Some(m) => m,
                    None => RenderMode::Static,
                };
                let stop_cmd = RenderThreadCommand::Stop(StopCommand {
                    output: output_name.clone(),
                });
                let start_cmd = RenderThreadCommand::Render(RenderCommand {
                    output: output_name.clone(),
                    image: output_config.image.clone(),
                    mode: mode,
                });
                let mode_cmd = DaemonCommand::OutputModeChange(ModeCommand {
                    output: output_name.clone(),
                    new_width: new_mode.width,
                    new_height: new_mode.height,
                });
                pandora.handle_cmd(&CommandType::Tc(stop_cmd));
                pandora.handle_cmd(&CommandType::Tc(start_cmd));
                pandora.handle_cmd(&CommandType::Dc(mode_cmd));
            }
        }
        wl_output::Event::Done => {
            output.done = true;
            let output_name = output.name.as_ref().unwrap().clone();
            let config_outputs = &ctx.state.config.outputs;
            let output_config = match config_outputs.iter().find(|oc| oc.name == output_name) {
                Some(conf) => conf,
                None => {
                    log(format!("could not find a config stanza for {output_name}, ignoring"));
                    return;
                },
            };
            let mode = match output_config.mode.as_ref() {
                Some(m) => match m {
                    RenderMode::Static => RenderMode::Static,
                    // initial pos of 0 is fine because the agent picks up workspace changes and enforces reflowing
                    RenderMode::ScrollVertical => RenderMode::ScrollVertical,
                    RenderMode::ScrollLateral => RenderMode::ScrollLateral,
                },
                None => RenderMode::Static,
            };
            let cmd = RenderThreadCommand::Render(RenderCommand {
                output: output_name,
                image: output_config.image.clone(),
                mode: mode,
            });
            let _ = ctx.state.pandora.as_ref().unwrap().handle_cmd(&CommandType::Tc(cmd));
        },
        wl_output::Event::Name(name) => output.name = Some(name.into_string().unwrap()),
        _ => (),
    }
}

fn log(s: String) {
    println!("> outputs: {s}");
}