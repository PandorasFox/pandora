use crate::commands::{CommandType, ScrollCommand, ThreadCommand};
use crate::config::{DaemonConfig, RenderModeConfig};
use crate::misc::get_new_image_dimensions;
use crate::sockets;

use std::collections::HashMap;
use std::ops::Index;

use image::ImageReader;
use miette::miette;
use niri_ipc::{Event, Output, Request, Response, Workspace};
use niri_ipc::socket::Socket;

pub struct NiriAgent {
    // deps
    socket: Option<Socket>,
    _pandora: Option<()>,
}

impl NiriAgent {
    // constructor will get handed an Arc<Pandora> later. For now, if it's none, we fall back to sending to the @pandora socket.
    pub fn new(pandora: Option<()>) -> Result<NiriAgent, ()> {
        match Socket::connect() {
            Ok(s) => Ok(NiriAgent {
                socket: Some(s),
                _pandora: pandora,
            }),
            Err(e) => panic!("{e:?}"),
        }
    }

    pub fn start(&mut self) -> miette::Result<()> {
        let mut socket = self.socket.take().unwrap();
        let mut processor = NiriProcessor::default();
        processor.config = crate::config::load_config()?;

        let outputs_response = match socket.send(Request::Outputs).unwrap() {
            Ok(Response::Outputs(response)) => response,
            Ok(_) => unreachable!(), // must not receive a differente type of response
            Err(e) => return Err(miette!(e)),
        };
        let workspaces_response = match socket.send(Request::Workspaces).unwrap() {
            Ok(Response::Workspaces(response)) => response,
            Ok(_) => unreachable!(), // must not receive a differente type of response
            Err(e) => return Err(miette!(e)),
        };
        
        processor.init_state(&outputs_response, &workspaces_response);

        let reply = socket.send(Request::EventStream).unwrap();
        if matches!(reply, Ok(Response::Handled)) {
            let mut read_event = socket.read_events();
            while let Ok(event) = read_event() {
                for cmd in processor.process(event) {
                    // replace with self.pandora.handle_cmd(cmd) when refactored in
                    let _ = sockets::write_command_to_daemon_socket(&cmd);
                }
            }
            return Ok(());
        }
        return Err(miette!("could not connect to niri IPC stream"));
    }
}

#[derive(Debug)]
struct OutputState {
    _width: i32,
    height: i32,
    // refresh: i32,
    _current_image: String,
    _img_width: i32,
    img_height: i32,
    mode: Option<RenderModeConfig>,
    max_workspace_idx: u8, // idx, name
}

#[derive(Default)]
struct NiriProcessor {
    config: DaemonConfig,
    outputs: Vec<(String, OutputState)>,
    workspaces: Vec<Workspace>,
}

impl NiriProcessor {
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

    fn init_state(&mut self, outputs: &HashMap<String, Output>, workspaces: &Vec<Workspace>) {
        for (output_name, output) in outputs {
            let output_config = match self.config.outputs.iter().find(|oc| oc.name == *output_name) {
                Some(c) => c,
                None => continue,
            };

            if output.current_mode.is_some() {
                let mode_idx = output.current_mode.unwrap();
                let mode = output.modes.index(mode_idx);
                let (output_width, output_height) = (mode.width as u32, mode.height as u32);
                let (scale_width, scale_height) = match &output_config.mode {
                    None => (Some(output_width), Some(output_height)),
                    Some(mode) => match mode {
                        RenderModeConfig::Static => (Some(output_width), Some(output_height)),
                        RenderModeConfig::ScrollVertical => (Some(output_width), None),
                        RenderModeConfig::ScrollLateral => (None, Some(output_height))
                    },
                };

                let img_path = output_config.image.clone();
                // TODO ask pandora for image dimensions :)
                let image = ImageReader::open(img_path.clone()).unwrap().decode().unwrap();
                let (scaled_width, scaled_height) = get_new_image_dimensions(image.width(), image.height(), scale_width, scale_height);

                let output_state = OutputState {
                    _width: mode.width as i32,
                    height: mode.height as i32,
                    _current_image: img_path,
                    _img_width: scaled_width as i32,
                    img_height: scaled_height as i32,
                    mode: output_config.mode.clone(),
                    max_workspace_idx: 0,
                };
                self.outputs.push((output_name.clone(), output_state));
            }
        }
        self.update_workspaces(&workspaces);
    }

    fn process(&mut self, e: niri_ipc::Event) -> Vec<CommandType> {
        let mut cmds = Vec::<CommandType>::new();
        match e {
            Event::WorkspacesChanged { workspaces } => {
                for output in &mut self.outputs {
                    output.1.max_workspace_idx = 0;
                }
                self.update_workspaces(&workspaces);
                for workspace in workspaces {
                    if workspace.is_active {
                        match self.gen_scroll_cmd_for_workspace_id(workspace.id) {
                            Some(cmd) => cmds.push(cmd),
                            None => (),
                        };
                    }
                }
            },
            Event::WorkspaceActivated {id, .. } => {
                match self.gen_scroll_cmd_for_workspace_id(id) {
                    Some(cmd) => cmds.push(cmd),
                    None => (),
                }
            },
            Event::WindowFocusChanged { id: _ } => {
                // TODO - needs https://github.com/YaLTeR/niri/pull/1265 or equivalent for window positioning info
            },
            _ => (), // idc about other events rn
        }
        return cmds;
    }
    
    fn gen_scroll_cmd_for_workspace_id(&self, id: u64) -> Option<CommandType> {
        let workspace = self.workspaces.iter().find(|w| w.id == id).unwrap();
        let curr_idx = workspace.idx;

        let output_name = match workspace.output.clone() {
            Some(o) => o,
            None => return None, // focused a workspace while no outputs connected / all outputs unplugged. whatever lol
        };
        let output = &self.outputs.iter().find(|o| o.0 == output_name).unwrap().1;
        match &output.mode {
            None => return None,
            Some(mode) => match mode {
                RenderModeConfig::ScrollVertical => {
                    let lower_scroll_pos_max = output.height * output.max_workspace_idx as i32;
                    let mut one_workspace_scroll_distance = output.height;
                    if lower_scroll_pos_max > output.img_height {
                        one_workspace_scroll_distance = output.img_height / (output.max_workspace_idx + 1) as i32; // todo think more about this lol
                    }
                    let cmd = ThreadCommand::Scroll(ScrollCommand {
                        output: output_name,
                        position: one_workspace_scroll_distance as u32 * (curr_idx - 1) as u32,
                    });
                    return Some(CommandType::Tc(cmd));
                },
                _ => return None,
            },
        };
    }
}