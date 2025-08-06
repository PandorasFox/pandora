use pithos::commands::{CommandType, RenderCommand, RenderMode, StopCommand, ThreadCommand};
use pithos::config::{DaemonConfig, RenderModeConfig};

use std::sync::Arc;
use std::thread;

use wayrs_client::global::GlobalExt;
use wayrs_client::protocol::wl_output::{self, WlOutput};
use wayrs_client::protocol::wl_registry::{self, GlobalArgs};
use wayrs_client::{Connection, EventCtx, IoMode};

use crate::pandora::Pandora;

// generic wayland agent for handling output plug/unplug events
pub struct OutputHandler {
    config: Option<DaemonConfig>,
    pandora: Option<Arc<Pandora>>,
}

impl OutputHandler {
    pub fn new(config: DaemonConfig, pandora: Arc<Pandora>) -> Arc<OutputHandler> {
        return Arc::new(OutputHandler {
            config: Some(config),
            pandora: Some(pandora),
        });
    }

    pub fn start(&self) {
        let config = self.config.clone().unwrap();
        let pandora = self.pandora.clone().unwrap();
        match thread::Builder::new().name("output handler".to_string())
        .spawn(|| {
            run(config, pandora);
        }) {
            Ok(_) => (), // [tf2 medic voice] i will live forever!
            Err(e) => panic!("could not spawn output events handler thread: {e:?}"),
        }
    }
}   

fn run(config: DaemonConfig, pandora: Arc<Pandora>) {
    let mut conn = Connection::connect().unwrap();
    let mut state = State::default();
    state.config = config;
    state.pandora = Some(pandora);
    conn.add_registry_cb(wl_registry_cb);
    loop {
        conn.flush(IoMode::Blocking).unwrap();
        conn.recv_events(IoMode::Blocking).unwrap();
        conn.dispatch_events(&mut state);
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
        }
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
        }
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
        wl_output::Event::Done => {
            let output_name = output.name.as_ref().unwrap().clone();
            let config_outputs = &ctx.state.config.outputs;
            let output_config = config_outputs.iter().find(|oc| oc.name == output_name).expect(format!("could not find a config stanza for {output_name}").as_str());
            let mode = match output_config.mode.as_ref() {
                Some(m) => match m {
                    // map the config enum onto the command enum, fetching position from agent
                    // TODO: refactor to a simpler 'agent command', drop config and complexity
                    RenderModeConfig::Static => RenderMode::Static,
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