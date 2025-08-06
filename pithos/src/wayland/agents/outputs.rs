// generic wayland agent for handling output plug/unplug events
#[derive(Default)]
struct OutputHandler {
    config: Option<Vec<ConfigNode>>,
    thread: Option<JoinHandle<()>>,
    // TODO: glue in Arc<Pandora>, implement "GetCurrentScrollPosition" that directs over to active agent
}

impl OutputHandler {
    pub fn start(&mut self) {
        let config = self.config.take().unwrap();
        match thread::Builder::new().name("output handler".to_string())
        .spawn(|| {
            run(config);
        }) {
            Ok(handle) => self.thread = Some(handle),
            Err(e) => panic!("could not spawn output events handler thread: {e:?}"),
        }
    }
}

/// TODO INTRODUCE THE BELOW TO MY FRIEND THE VENDORING HACKSAW

use std::ffi::CString;
use std::thread::{self, JoinHandle};

use wayrs_client::global::GlobalExt;
use wayrs_client::protocol::wl_output::{self, WlOutput};
use wayrs_client::protocol::wl_registry::{self, GlobalArgs};
use wayrs_client::{Connection, EventCtx, IoMode};

use crate::config::ConfigNode;

fn run(config: Vec<ConfigNode>) {
    let mut conn = Connection::connect().unwrap();
    let mut state = State::default();
    state.config = config;
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
    config: Vec<ConfigNode>,
}

#[derive(Debug)]
struct Output {
    registry_name: u32,
    wl_output: WlOutput,
    name: Option<CString>,
    desc: Option<CString>,
    scale: Option<i32>,
    mode: Option<String>,
}

impl Output {
    fn bind(conn: &mut Connection<State>, global: &GlobalArgs) -> Self {
        Self {
            registry_name: global.name,
            wl_output: global.bind_with_cb(conn, 3..=4, wl_output_cb).unwrap(),
            name: None,
            desc: None,
            scale: None,
            mode: None,
        }
    }
}

fn wl_registry_cb(conn: &mut Connection<State>, state: &mut State, event: &wl_registry::Event) {
    match event {
        wl_registry::Event::Global(global) if global.is::<WlOutput>() => {
            state.outputs.push(Output::bind(conn, global));
        }
        wl_registry::Event::GlobalRemove(name) => {
            if let Some(i) = state.outputs.iter().position(|o| o.registry_name == *name) {
                let output = state.outputs.swap_remove(i);
                eprintln!("removed output: {}", output.name.unwrap().to_str().unwrap());
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
        wl_output::Event::Geometry(_) => (),
        wl_output::Event::Mode(mode) => {
            output.mode = Some(format!(
                "{}x{} @ {}Hz",
                mode.width,
                mode.height,
                mode.refresh as f64 * 1e-3
            ))
        }
        wl_output::Event::Done => {
            dbg!(output);
        }
        wl_output::Event::Scale(scale) => output.scale = Some(scale),
        wl_output::Event::Name(name) => output.name = Some(name),
        wl_output::Event::Description(desc) => output.desc = Some(desc),
        _ => (),
    }
}