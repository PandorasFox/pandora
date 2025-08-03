use crate::commands::RenderMode;

use std::fs::File;
use std::time::Duration;

use wayrs_client::{Connection, EventCtx, IoMode};
use wayrs_client::global::{GlobalExt};
use wayrs_client::protocol::wl_output::{self, WlOutput};
use wayrs_client::protocol::wl_surface::WlSurface;
use wayrs_client::protocol::WlBuffer;
use wayrs_client::protocol::WlShmPool;
use wayrs_protocols::viewporter::WpViewport;



#[derive(Default)]
pub struct RenderThreadWaylandState {
    //pub globals: Option<WaylandGlobals>,
    pub outputs: Option<Vec<(WlOutput, Output)>>,
    pub render_state: Option<RenderState>, // placeholder type
    pub viewport: Option<WpViewport>,
    pub surface: Option<WlSurface>,
    pub output_info: Option<OutputMode>,
}

#[derive(Copy, Clone)]
pub struct ScrollState {
    pub start_pos: u32,
    pub current_pos: u32,
    pub end_pos: u32,
    pub step: u32,
    pub remaining_duration: Duration,
    pub _num_frames: u32,
}

pub struct RenderState {
    pub mode: RenderMode,
    pub _img_path: String,
    pub _buf_file: File,
    pub buffer: WlBuffer,
    pub bufpool: WlShmPool,
    pub scrolling: Option<ScrollState>,
    pub crop_width: u32,
    pub crop_height: u32,
    pub orig_width: u32,
    pub orig_height: u32,
}


#[derive(Copy, Clone, Debug, Default)]
pub struct OutputMode {
    pub height: i32,
    pub width: i32,
    pub _refresh: i32, // 59.997Hz => 59_997 int
}

#[derive(Clone, Debug, Default)]
pub struct Output {
    done: bool,
    pub name: String,
    pub desc: String,
    pub scale: Option<i32>,
    pub mode: OutputMode,
}

pub fn get_wloutput_by_name(conn: &mut Connection<RenderThreadWaylandState>, name: String) -> (WlOutput, Output) {
    let outputs = get_outputs(conn);
    for (wlo, info) in outputs {
        if info.name == name {
            return (wlo, info);
        }
    }
    panic!("aborting render thread: tried to create for a named output that does not exist")
}

fn get_outputs(conn: &mut Connection<RenderThreadWaylandState>) -> Vec<(WlOutput, Output)> {
    conn.blocking_roundtrip().unwrap();
    let outputs : Vec<(WlOutput, Output)> = conn
            .globals()
            .iter()
            .filter(|g| g.is::<WlOutput>())
            .map(|g| g.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .map(|g| g.bind_with_cb(conn, 2..=4, wl_output_cb).unwrap())
            .map(|output| (output, Output::default()))
            .collect();

    conn.flush(IoMode::Blocking).unwrap();

    let mut state = RenderThreadWaylandState::default();
    state.outputs = Some(outputs);

    while !state.outputs.as_ref().unwrap().iter().all(|x| x.1.done) {
        conn.recv_events(IoMode::Blocking).unwrap();
        conn.dispatch_events(&mut state);
    }
    return state.outputs.unwrap();
}

fn wl_output_cb(ctx: EventCtx<RenderThreadWaylandState, WlOutput>) {
    let output = &mut ctx
        .state
        .outputs.as_mut().unwrap()
        .iter_mut()
        .find(|o| o.0 == ctx.proxy)
        .unwrap()
        .1;
    match ctx.event {
        wl_output::Event::Geometry(_) => (),
        wl_output::Event::Mode(mode) => {
            output.mode = OutputMode {height: mode.height, width: mode.width, _refresh: mode.refresh};
        }
        wl_output::Event::Done => output.done = true,
        wl_output::Event::Scale(scale) => output.scale = Some(scale),
        wl_output::Event::Name(name) => output.name = name.into_string().unwrap(),
        wl_output::Event::Description(desc) => output.desc = desc.into_string().unwrap(),
        _ => (),
    }
}