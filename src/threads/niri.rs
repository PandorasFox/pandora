use crate::daemon::Daemon;
use crate::pithos::commands::{
    CommandType, DaemonCommand, RenderCommand, RenderMode, RenderThreadCommand, ScrollCommand,
};
use crate::pithos::config::DaemonConfig;

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, Weak};
use std::thread;

use miette::Error;
use niri_ipc::socket::Socket;
use niri_ipc::{Event, Output, Request, Response, Workspace};

pub struct NiriAgent {
    config: DaemonConfig,
    cmd_queue: Arc<Mutex<Receiver<DaemonCommand>>>,
    pub queue: Sender<DaemonCommand>,
}

impl NiriAgent {
    pub fn new(config: DaemonConfig) -> Result<Arc<NiriAgent>, Error> {
        match Socket::connect() {
            Ok(_) => {
                let (send, recv) = channel::<DaemonCommand>();
                Ok(Arc::new(NiriAgent {
                    config,
                    cmd_queue: Arc::new(Mutex::new(recv)),
                    queue: send,
                }))
            }
            Err(e) => Err(miette::miette!(e)), // todo
        }
    }

    pub fn start(&self, weak: Weak<dyn Daemon + Send + Sync>) {
        let pandora = weak.upgrade().unwrap();
        let config = self.config.clone();
        let cmd_queue = self.cmd_queue.clone();
        match thread::Builder::new()
            .name("niri agent".to_string())
            .spawn(move || {
                run(config, pandora.clone(), cmd_queue);
                pandora.log(
                    "niri-agent",
                    "thread exiting (is session exiting?)".to_string(),
                );
            }) {
            Ok(_) => (), // [tf2 medic voice] i will live forever!
            Err(e) => panic!("could not spawn niri ipc handler thread: {e:?}"),
        };
    }
}

fn get_niri_state(socket: &mut Socket) -> (HashMap<String, Output>, Vec<Workspace>) {
    let outputs_response = match socket.send(Request::Outputs).unwrap() {
        Ok(Response::Outputs(response)) => response,
        Ok(_) => unreachable!(), // must not receive a differente type of response
        Err(e) => panic!("error getting outputs from niri: {e:?}"),
    };
    let workspaces_response = match socket.send(Request::Workspaces).unwrap() {
        Ok(Response::Workspaces(response)) => response,
        Ok(_) => unreachable!(), // must not receive a differente type of response
        Err(e) => panic!("error getting workspaces from niri: {e:?}"),
    };

    (outputs_response, workspaces_response)
}

fn run(
    config: DaemonConfig,
    pandora: Arc<dyn Daemon + Send + Sync>,
    cmd_queue: Arc<Mutex<Receiver<DaemonCommand>>>,
) {
    let mut socket = Socket::connect().unwrap();
    let mut processor = NiriProcessor {
        config,
        ..Default::default()
    };

    processor.init_state(&mut socket);
    processor.reseat_scroll_positions(pandora.clone());

    let reply = socket.send(Request::EventStream).unwrap();
    if matches!(reply, Ok(Response::Handled)) {
        let mut read_event = socket.read_events();
        loop {
            match read_event() {
                Ok(event ) => {
                    processor.process(pandora.clone(), event);
                    match cmd_queue.lock() {
                        Ok(channel) => {
                            if let Ok(cmd) = channel.try_recv() {
                                match cmd {
                                    DaemonCommand::ReloadConfig(config) => {
                                        processor.update_config(config, pandora.clone())
                                    }
                                    DaemonCommand::Lock => (), // i think ?
                                    DaemonCommand::Stop => (),
                                }
                            }
                        }
                        Err(e) => {
                            pandora.log("niri-agent", format!("error acquiring channel lock: {e:?}"));
                        }
                    }
                },
                Err(e) => {
                    pandora.debug("niri-agent", format!("event read failed {e:?}"));
                }
            }
        }
    }
}

#[derive(Debug)]
struct OutputState {
    current_image: String,
    mode: Option<RenderMode>,
    max_workspace_idx: u8,
}

#[derive(Default)]
struct NiriProcessor {
    config: DaemonConfig,
    outputs: Vec<(String, OutputState)>,
    workspaces: Vec<Workspace>,
}

impl NiriProcessor {
    fn update_config(
        &mut self,
        new_config: DaemonConfig,
        pandora: Arc<dyn Daemon + Send + Sync>,
    ) {
        for new_output_conf in &new_config.outputs {
            let p = pandora.clone();
            let new_mode = new_output_conf.mode.unwrap_or(RenderMode::Static);
            let (output_name, state) = match self
                .outputs
                .iter_mut()
                .find(|o| o.0 == new_output_conf.name)
            {
                Some(v) => v,
                None => continue,
            };
            if state.current_image != new_output_conf.image
                || state.mode.unwrap_or(RenderMode::Static) != new_mode
            {
                let cmd = RenderCommand {
                    output: output_name.clone(),
                    image: new_output_conf.image.clone(),
                    mode: new_mode,
                    position: (0.0, 0.0), // todo: should compute scroll position + plumb this on the other end
                };
                p.handle_cmd(&CommandType::Tc(RenderThreadCommand::Render(cmd)));

                // Update state to reflect the change
                state.current_image = new_output_conf.image.clone();
                state.mode = Some(new_mode);
            }
        }
        self.config = new_config;
    }

    fn update_workspaces(&mut self, workspaces: &Vec<Workspace>) {
        for workspace in workspaces {
            if workspace.output.is_some() {
                let output_name = workspace.output.clone().unwrap();
                let output_state = match self.outputs.iter_mut().find(|os| os.0 == output_name) {
                    Some(v) => v,
                    None => continue,
                };
                let cur_max_idx = output_state.1.max_workspace_idx;
                output_state.1.max_workspace_idx = u8::max(workspace.idx, cur_max_idx);
            }
        }
        self.workspaces = workspaces.clone();
    }

    fn init_state(&mut self, niri_socket: &mut Socket) {
        let (outputs, workspaces) = get_niri_state(niri_socket);
        for (output_name, output) in outputs {
            let output_config = match self
                .config
                .outputs
                .iter()
                .find(|oc| oc.name == *output_name)
            {
                Some(c) => c,
                None => continue,
            };

            if output.current_mode.is_some() {
                let img_path = output_config.image.clone();
                let output_state = OutputState {
                    current_image: img_path,
                    mode: output_config.mode,
                    max_workspace_idx: 0,
                };
                self.outputs.push((output_name.clone(), output_state));
            }
        }
        self.update_workspaces(&workspaces);
    }

    fn reseat_scroll_positions(&self, pandora: Arc<dyn Daemon + Send + Sync>) {
        for workspace in &self.workspaces {
            if workspace.is_active {
                self.gen_scroll_cmd_for_workspace_id(pandora.clone(), workspace.id);
            }
        }
    }

    fn process(&mut self, pandora: Arc<dyn Daemon + Send + Sync>, e: niri_ipc::Event) {
        match e {
            Event::WorkspacesChanged { workspaces } => {
                for output in &mut self.outputs {
                    output.1.max_workspace_idx = 0;
                }
                self.update_workspaces(&workspaces);
            }
            Event::WorkspaceActivated { id, .. } => {
                self.gen_scroll_cmd_for_workspace_id(pandora, id)
            }
            Event::WindowFocusChanged { id: _ } => {
                // TODO - niri includes tile layouts in WindowLayout structs now
                // we should keep track of the full pixel width of each workspace,
                // as well as the position of the focused window within that mosaic
                // and compute a scroll percentage based on that
                // we'll want to then start using that whenever we gen_scroll_cmd,
                // and just trust the render thread to discard or use as needed.
            }
            _ => (), // idc about other events rn
        }
    }

    fn gen_scroll_cmd_for_workspace_id(&self, pandora: Arc<dyn Daemon + Send + Sync>, id: u64) {
        let workspace = self.workspaces.iter().find(|w| w.id == id).unwrap();
        let curr_idx = workspace.idx;

        let output_name = match workspace.output.clone() {
            Some(o) => o,
            None => return, // focused a workspace while no outputs connected / all outputs unplugged. whatever lol
        };
        let output = match &self.outputs.iter().find(|o| o.0 == output_name) {
            Some(tuple) => &tuple.1,
            None => {
                pandora.log(
                    "niri-agent",
                    format!("{output_name} not found in config; ignoring"),
                );
                return; // display not configured
            }
        };
        if let Some(cmd) = match &output.mode {
            None => None,
            Some(RenderMode::ScrollVertical) => {
                let mut scroll_percent =
                    100.0 * (curr_idx - 1) as f64 / (output.max_workspace_idx - 1) as f64;
                if scroll_percent.is_nan() {
                    // divided by zero because only one workspace on output
                    scroll_percent = 50.0;
                }
                let cmd = RenderThreadCommand::Scroll(ScrollCommand {
                    output: output_name,
                    position_x: 50.0,
                    position_y: scroll_percent,
                });
                Some(CommandType::Tc(cmd))
            }
            Some(RenderMode::ScrollLateral) => {
                // TODO: implement horizontal scrolling for window changes
                todo!()
            }
            Some(RenderMode::Static) => None,
        } {
            pandora.verbose("niri-agent", format!("emitting command {cmd:?}"));
            pandora.handle_cmd(&cmd);
        }
    }
}
