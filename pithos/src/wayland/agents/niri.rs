use miette::miette;
use niri_ipc::{Request, Response};
use niri_ipc::socket::Socket;

use crate::config::DaemonConfig;

pub struct NiriAgent {
    // deps
    socket: Option<Socket>,
    _pandora: Option<()>,
    processor: Option<NiriProcessor>,    
}

impl NiriAgent {
    // constructor will get handed an Arc<Pandora> later. For now, if it's none, we fall back to sending to the @pandora socket.
    pub fn new(pandora: Option<()>) -> Result<NiriAgent, ()> {
        match Socket::connect() {
            Ok(s) => Ok(NiriAgent {
                socket: Some(s),
                _pandora: pandora,
                processor: None,
            }),
            Err(e) => panic!("{e:?}"),
        }
    }

    pub fn start(mut self) -> miette::Result<()> {
        let mut socket = self.socket.take().unwrap();
        let mut processor = NiriProcessor::default();
        processor.config = crate::config::load_config()?;
        // TODO: socket.send(Request::Outputs, Workspaces) => construct initial state
        // TODO: send initial render commands based on configs after constructing initial state
        // TODO: minimal wayrs Output handler => Stop/Start handler
        let reply = socket.send(Request::EventStream).unwrap();
        if matches!(reply, Ok(Response::Handled)) {
            let mut read_event = socket.read_events();
            while let Ok(event) = read_event() {
                processor.process(event);
            }
            return Ok(());
        }
        return Err(miette!("could not connect to niri IPC stream"));
    }
}

struct OutputState {
    width: i32,
    height: i32,
    // refresh: i32,
    current_image: String,
    img_width: i32,
    img_height: i32,
    workspaces: Vec<i32>,
}

#[derive(Default)]
struct NiriProcessor {
    config: DaemonConfig,
    outputs: Vec<(String, OutputState)>,
}

impl NiriProcessor {
    fn process(&mut self, e: niri_ipc::Event) {
        println!("Received event: {e:?}");
        // WorkspacesChanges: keep track of workspaces-per-output - MIGHT NEED A SCROLL MIGHT NOT 
        // WorkspaceActivated: compute which output had state change, compute direction/resulting position, dispatch!
        // WindowFocus: will require https://github.com/YaLTeR/niri/pull/1265 (or me addressing the comments) to handle lateral scrolling
        // TODO dispatch events
    }
}