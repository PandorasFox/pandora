use pithos::commands::{CommandType, RenderCommand, RenderMode, StopCommand, ThreadCommand};
use pithos::config::{DaemonConfig, RenderModeConfig};

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use wayrs_client::global::GlobalExt;
use wayrs_client::protocol::wl_output::{self, WlOutput};
use wayrs_client::protocol::wl_registry::{self, GlobalArgs};
use wayrs_client::{Connection, EventCtx, IoMode};

use crate::pandora::Pandora;

// generic wayland agent for handling output plug/unplug events
// TODO: should also listen for output mode changes => stop and (re)start affected render threads
// should also dispatch an agent command instructing the agent to reload re-init / re-poll for output state?
pub struct OutputHandler {
    config: Option<DaemonConfig>,
    pandora: Option<Arc<Pandora>>,
    cfg_notif: Arc<Mutex<Receiver<DaemonConfig>>>,
    pub notif: Sender<DaemonConfig>,
}

impl OutputHandler {
    pub fn new(config: DaemonConfig, pandora: Arc<Pandora>) -> Arc<OutputHandler> {
        let (send, recv) = channel::<DaemonConfig>();
        return Arc::new(OutputHandler {
            config: Some(config),
            pandora: Some(pandora),
            cfg_notif: Arc::new(Mutex::new(recv)),
            notif: send,
        });
    }

    pub fn start(&self) {
        let config = self.config.clone().unwrap();
        let pandora = self.pandora.clone().unwrap();
        let cfg_notif = self.cfg_notif.clone();
        match thread::Builder::new().name("output handler".to_string())
        .spawn(move || {
            run(config, pandora, cfg_notif);
        }) {
            Ok(_) => (), // [tf2 medic voice] i will live forever!
            Err(e) => panic!("could not spawn output events handler thread: {e:?}"),
        }
    }
}   

fn run(config: DaemonConfig, pandora: Arc<Pandora>, cfg_notif: Arc<Mutex<Receiver<DaemonConfig>>>) {
    let mut conn = Connection::connect().unwrap();
    let mut state = State::default();
    state.config = config;
    state.pandora = Some(pandora);
    conn.add_registry_cb(wl_registry_cb);
    loop {
        conn.flush(IoMode::Blocking).unwrap();
        conn.recv_events(IoMode::Blocking).unwrap();
        conn.dispatch_events(&mut state);
        match cfg_notif.lock() {
            Ok(channel) => {
                match channel.try_recv() {
                    Ok(conf) => state.config = conf,
                    Err(_) => (), // assuming this is just "no config sent over the wire"
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
}

impl Output {
    fn bind(conn: &mut Connection<State>, global: &GlobalArgs) -> Self {
        Self {
            registry_name: global.name,
            wl_output: global.bind_with_cb(conn, 3..=4, wl_output_cb).unwrap(),
            name: None,
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
                let cmd = ThreadCommand::Stop(StopCommand {
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
    let output = &mut ctx
        .state
        .outputs
        .iter_mut()
        .find(|o| o.wl_output == ctx.proxy)
        .unwrap();
    match ctx.event {
        wl_output::Event::Mode(_mode) => {
            // emit stop command and then start command for output
            // also emit "hey agent request output state again" command
            // we can ctx.state.pandora.dispatch_cmd from here to do all this :)
            // TODO
        }
        wl_output::Event::Done => {
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
                    RenderModeConfig::Static => RenderMode::Static,
                    // initial pos of 0 is fine because the agent picks up workspace changes and enforces reflowing
                    RenderModeConfig::ScrollVertical => RenderMode::ScrollingVertical(0),
                    RenderModeConfig::ScrollLateral => RenderMode::ScrollingLateral(0),
                },
                None => RenderMode::Static,
            };
            let cmd = ThreadCommand::Render(RenderCommand {
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