use std::ffi::CString;

use wayrs_client::global::{GlobalExt};
use wayrs_client::protocol::wl_output::{self, WlOutput};
use wayrs_client::{Connection, EventCtx, IoMode};
use wayrs_client::protocol::wl_compositor::WlCompositor;
use wayrs_client::protocol::wl_shm::WlShm;
use wayrs_client::protocol::WlSurface;
use wayrs_protocols::linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wayrs_protocols::wlr_layer_shell_unstable_v1::{ZwlrLayerSurfaceV1, ZwlrLayerShellV1, zwlr_layer_shell_v1::Layer};
use wayrs_protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::Anchor;
use wayrs_protocols::viewporter::{WpViewporter, WpViewport};

// mostly boilerplate from wayrs examples, introduced to a hacksaw

#[derive(Default)]
pub struct WaylandState {
    //pub globals: Option<WaylandGlobals>,
    pub outputs: Vec<(WlOutput, Output)>,
    pub _animation_state: Option<String>, // placeholder type
}

#[derive(Copy, Clone)]
pub struct WaylandGlobals {
    pub _output: WlOutput,
    pub output_info: OutputMode,
    pub shm: WlShm, // shared mem singleton
    pub _dma: ZwpLinuxDmabufV1,
    pub _compositor: WlCompositor,
    pub _layer_shell: ZwlrLayerShellV1,
    pub surface: WlSurface,
    pub _viewporter: WpViewporter,
    pub viewport: WpViewport,
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

fn layer_callback(mut ctx: EventCtx<WaylandState, ZwlrLayerSurfaceV1>) {
    // none of my debug prints in here ever popped, presumably because the execution context
    // didn't have a stdout or something. pretty sure this works right for just yolo-ACKing
    // these events. silly protocol.
    let layer: ZwlrLayerSurfaceV1 = ctx.proxy;
    match ctx.event {
        wayrs_protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::Event::Configure(args) => {
            layer.ack_configure(&mut ctx.conn, args.serial);
        },
        _ => (),
    }
}

pub fn initialize_wayland_handles(conn: &mut Connection<WaylandState>, output: String) -> WaylandGlobals {
    let (wl_output, output_info) = get_wloutput_by_name(conn, output);
    let width = output_info.mode.width;
    let height = output_info.mode.height;

    // TODO: vibe check if dma is easier/better to use in any meaningful way
    // (it's hopefully widely available?)
    // the current shm stuff seems to work fine enough for now, at least
    let shm = conn.bind_singleton::<WlShm>(2..=2).unwrap();
    let dma = conn.bind_singleton::<ZwpLinuxDmabufV1>(4..=5).unwrap();
    let layer_shell = conn.bind_singleton::<ZwlrLayerShellV1>(4..=5).unwrap();
    let compositor = conn.bind_singleton::<WlCompositor>(1..=6).unwrap();
    let viewporter = conn.bind_singleton::<WpViewporter>(1..=1).unwrap();

    let surface = compositor.create_surface(conn);
    let viewport = viewporter.get_viewport(conn, surface);
    //conn.blocking_roundtrip().unwrap();
    let layer_surface = layer_shell.get_layer_surface(conn, surface, Some(wl_output), Layer::Background, CString::new("pandora").unwrap());
    layer_surface.set_size(conn, width as u32, height as u32);
    layer_surface.set_anchor(conn, Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right );
    layer_surface.set_exclusive_zone(conn, -1);
    
    // set callback handler for 'layer_surface.configure' event
    conn.set_callback_for(layer_surface, layer_callback);
    //conn.blocking_roundtrip().unwrap();
    surface.commit(conn);
    conn.blocking_roundtrip().unwrap();

    return WaylandGlobals {
        _output: wl_output,
        shm: shm,
        _dma: dma,
        _compositor: compositor,
        _layer_shell: layer_shell,
        surface: surface,
        output_info: output_info.mode,
        _viewporter: viewporter,
        viewport: viewport,
    };
}

fn get_wloutput_by_name(conn: &mut Connection<WaylandState>, name: String) -> (WlOutput, Output) {
    let outputs = get_outputs(conn);
    for (wlo, info) in outputs {
        if info.name == name {
            return (wlo, info);
        }
    }
    panic!("aborting render thread: tried to create for a named output that does not exist")
}

fn get_outputs(conn: &mut Connection<WaylandState>) -> Vec<(WlOutput, Output)> {
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

    let mut state = WaylandState::default();
    state.outputs = outputs;

    while !state.outputs.iter().all(|x| x.1.done) {
        conn.recv_events(IoMode::Blocking).unwrap();
        conn.dispatch_events(&mut state);
    }
    return state.outputs;
}

fn wl_output_cb(ctx: EventCtx<WaylandState, WlOutput>) {
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
            output.mode = OutputMode {height: mode.height, width: mode.width, _refresh: mode.refresh};
        }
        wl_output::Event::Done => output.done = true,
        wl_output::Event::Scale(scale) => output.scale = Some(scale),
        wl_output::Event::Name(name) => output.name = name.into_string().unwrap(),
        wl_output::Event::Description(desc) => output.desc = desc.into_string().unwrap(),
        _ => (),
    }
}