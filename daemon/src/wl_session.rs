use std::ops::{self, RangeInclusive};

use wayrs_client::global::{GlobalExt};
use wayrs_client::protocol::wl_output::{self, WlOutput};
use wayrs_client::{Connection, EventCtx, IoMode};
// mostly boilerplate from wayrs examples, introduced to a hacksaw

pub fn get_wloutput_by_name(conn: &mut Connection<State>, name: String) -> (WlOutput, Output) {
    let outputs = get_outputs(conn);
    for (wlo, info) in outputs {
        if info.name == name {
            return (wlo, info);
        }
    }
    panic!("aborting render thread: tried to create for a named output that does not exist")
}

fn get_outputs(conn: &mut Connection<State>) -> Vec<(WlOutput, Output)> {
    conn.blocking_roundtrip().unwrap();
    let mut state = State {
        outputs: conn
            .globals()
            .iter()
            .filter(|g| g.is::<WlOutput>())
            .map(|g| g.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|g| g.bind_with_cb(conn, 2..=4, wl_output_cb).unwrap())
            .map(|output| (output, Output::default()))
            .collect(),
    };

    conn.flush(IoMode::Blocking).unwrap();

    while !state.outputs.iter().all(|x| x.1.done) {
        conn.recv_events(IoMode::Blocking).unwrap();
        conn.dispatch_events(&mut state);
    }
    return state.outputs;
}

#[derive(Clone, Debug, Default)]
pub struct Mode {
    pub height: i32,
    pub width: i32,
    pub refresh: i32, // 59.997Hz => 59_997 int
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    done: bool,
    pub name: String,
    pub desc: String,
    pub scale: Option<i32>,
    pub mode: Mode,
}

#[derive(Default)]
pub struct State {
    outputs: Vec<(WlOutput, Output)>,
}

fn wl_output_cb(ctx: EventCtx<State, WlOutput>) {
    let output = &mut ctx
        .state
        .outputs
        .iter_mut()
        .find(|o| o.0 == ctx.proxy)
        .unwrap()
        .1;
    match ctx.event {
        wl_output::Event::Geometry(_) => (),
        wl_output::Event::Mode(mode) => {
            output.mode = Mode {height: mode.height, width: mode.width, refresh: mode.refresh};
        }
        wl_output::Event::Done => output.done = true,
        wl_output::Event::Scale(scale) => output.scale = Some(scale),
        wl_output::Event::Name(name) => output.name = name.into_string().unwrap(),
        wl_output::Event::Description(desc) => output.desc = desc.into_string().unwrap(),
        _ => (),
    }
}