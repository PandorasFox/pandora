use crate::pandora::Pandora;
use pithos::commands::{CommandType, ScrollCommand, ThreadCommand};
use pithos::config::{DaemonConfig, RenderModeConfig};
use pithos::misc::get_new_image_dimensions;

use std::collections::HashMap;
use std::ops::Index;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use niri_ipc::{Event, Output, Request, Response, Workspace};
use niri_ipc::socket::Socket;


// todo: 
pub struct NiriAgent {
    pandora: Arc<Pandora>,
    config: DaemonConfig,
    cfg_notif: Arc<Mutex<Receiver<DaemonConfig>>>,
    pub notif: Sender<DaemonConfig>,
}

impl NiriAgent {
    // constructor will get handed an Arc<Pandora> later. For now, if it's none, we fall back to sending to the @pandora socket.
    pub fn new(config: DaemonConfig, pandora: Arc<Pandora>) -> Arc<NiriAgent> {
        match Socket::connect() {
            Ok(_) => {
                let (send, recv) = channel::<DaemonConfig>();
                Arc::new(NiriAgent {
                    pandora,
                    config,
                    cfg_notif: Arc::new(Mutex::new(recv)),
                    notif: send
                })
            },
            Err(e) => panic!("{e:?}"),
        }
    }

    pub fn start(&self) {
        let pandora = self.pandora.clone();
        let config = self.config.clone();
        let cfg_notif = self.cfg_notif.clone();
        match thread::Builder::new().name("niri agent".to_string())
            .spawn(move || {
                run(config, pandora, cfg_notif);
                eprintln!("niri agent thread exiting (is session exiting?)");
            }
        ) {
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

    return (outputs_response, workspaces_response);
}

fn run(config: DaemonConfig, pandora: Arc<Pandora>, cfg_notif: Arc<Mutex<Receiver<DaemonConfig>>>) {
    let mut socket = Socket::connect().unwrap();
    let mut processor = NiriProcessor::default();
    processor.config = config;

    let (outputs_response, workspaces_response) = get_niri_state(&mut socket);
    
    for cmd in processor.init_state(pandora.clone(), &outputs_response, &workspaces_response) {
        let _ = pandora.handle_cmd(&cmd);
    }

    let reply = socket.send(Request::EventStream).unwrap();
    if matches!(reply, Ok(Response::Handled)) {
        let mut read_event = socket.read_events();
        while let Ok(event) = read_event() {
            for cmd in processor.process(event) {
                let _ = pandora.handle_cmd(&cmd);
            }
            match cfg_notif.lock() {
                Ok(channel) => {
                    match channel.try_recv() {
                        Ok(conf) => {
                            processor.config = conf
                            // TODO: come back later and figure out what the correct post-cfg-reload action is
                        },
                        Err(_) => (), // assuming this is just "no config sent over the wire"
                    }
                },
                Err(e) => {
                    eprintln!("> niri-agent: error acquiring channel lock: {e:?}");
                }
            }
        }
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

    fn init_state(&mut self, pandora: Arc<Pandora>, outputs: &HashMap<String, Output>, workspaces: &Vec<Workspace>) -> Vec<CommandType> {
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

                pandora.load_image(&img_path).unwrap(); // can explode on invalid images l0l

                let (image_width, image_height) = match pandora.get_image_dimensions(img_path.clone()) {
                    Ok((w, h)) => (w, h),
                    Err(_) => unreachable!(), // LoadImage should've exploded
                };
               
                let (scaled_width, scaled_height) = get_new_image_dimensions(image_width, image_height, scale_width, scale_height);

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
        return self.reseat_scroll_positions();

    }

    fn reseat_scroll_positions(&self) -> Vec<CommandType> {
        let mut cmds = Vec::<CommandType>::new();
        for workspace in &self.workspaces {
            if workspace.is_active {
                match self.gen_scroll_cmd_for_workspace_id(workspace.id) {
                    Some(cmd) => cmds.push(cmd),
                    None => (),
                };
            }
        }
        return cmds;
    }

    fn process(&mut self, e: niri_ipc::Event) -> Vec<CommandType> {
        let mut cmds = Vec::<CommandType>::new();
        match e {
            Event::WorkspacesChanged { workspaces } => {
                for output in &mut self.outputs {
                    output.1.max_workspace_idx = 0;
                }
                self.update_workspaces(&workspaces);
                cmds = self.reseat_scroll_positions();
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
                    let last_scroll_pos = output.img_height - output.height;
                    let first_scroll_pos = 0;
                    // idx 1: 0, .... idx N: last_scroll_pos
                    // scroll pos of idx x is ((last - first) / (N - 1)) * (x-1)
                    // scroll dist should be min(that, output_height) so that if we have too few workspaces we scroll in a continuous manner
                    let scroll_per_workspace = output.height.min((last_scroll_pos - first_scroll_pos) / (output.max_workspace_idx - 1) as i32);
                    let pos = scroll_per_workspace as u32 * (curr_idx - 1) as u32;
                    let cmd = ThreadCommand::Scroll(ScrollCommand {
                        output: output_name,
                        position: pos,
                    });
                    self.log(format!("idx: {curr_idx}, max: {} | scroll dist {scroll_per_workspace} to {pos} | img {} , output {}", output.max_workspace_idx, output.img_height, output.height));
                    return Some(CommandType::Tc(cmd));
                },
                _ => return None,
            },
        };
    }

    fn log(&self, s: String) {
        println!("> niri-agent: {s}");
    }
}