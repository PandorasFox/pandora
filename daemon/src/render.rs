use image::RgbaImage;
use pithos::commands::{RenderCommand, ScrollCommand, ThreadCommand};
use crate::pandora::Pandora;
use crate::wl_session::{Mode, State};

use std::io::Write;
use std::sync::Arc;
use std::{ffi::CString, fs::File, os::fd::OwnedFd, sync::mpsc::Receiver};

// todo: tidy up these imports once i'm done hackin'
use wayrs_client::{Connection, EventCtx};
use wayrs_client::protocol::wl_shm::Format;
use wayrs_client::protocol::wl_compositor::WlCompositor;
use wayrs_client::protocol::wl_output::WlOutput;
use wayrs_client::protocol::wl_shm::WlShm;
use wayrs_client::protocol::WlSurface;
use wayrs_client::protocol::WlBuffer;
use wayrs_protocols::wlr_layer_shell_unstable_v1::{ZwlrLayerShellV1, ZwlrLayerSurfaceV1, zwlr_layer_shell_v1::Layer};
use wayrs_protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::Anchor;
use wayrs_protocols::linux_dmabuf_v1::ZwpLinuxDmabufV1;



// note: output resize/mode-setting changes are currently not handled here
// the "best" solution will probably involve a new command from the compositor agent
// that will be sent upon an output mode/resolution change

struct WaylandState {
    conn: Connection<State>,
    output: WlOutput,
    output_info: Mode,
    shm: WlShm, // shared mem singleton
    compositor: WlCompositor,
    layer_shell: ZwlrLayerShellV1,
    surface: WlSurface,
}

pub struct RenderThread {
    pub receiver: Receiver<ThreadCommand>,
    pandora: Arc<Pandora>,
    // state below, ough
    state: Option<WaylandState>,
}

fn layer_callback(mut ctx: EventCtx<State, ZwlrLayerSurfaceV1>) {
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

impl RenderThread {
    pub fn new(recv: Receiver<ThreadCommand>, pandora: Arc<Pandora>) -> RenderThread {
        return RenderThread {
            receiver: recv,
            pandora: pandora,
            state: None,
        }
    }

    pub fn start(&mut self) {
        // TODO: refactor this; actual loop will be a draw loop - first command should always be a render command
        // can just block on that, explode/exit if not a render command
        // remaining polls of the command queue will have to happen in the draw loop lol
        loop {
            let cmd = self.receiver.recv().expect("thread exploded while waiting on next command recv");
            match cmd {
                ThreadCommand::Render(c) => {
                    self.render(c);
                }
                ThreadCommand::Stop(_) => {
                    break; // goodbye!
                }
                ThreadCommand::Scroll(c) => {
                    self.scroll(c);
                }
            }
        }
    }

    fn initialize_wayland_handles(&mut self, output: String) {
        // should make calls to wayland session, get output names, match against output
        // and initialize The Wayland Surface Buffer for this thread
        // this is some load-bearing hack work rn and needs to get cleaned up before a Big Release
        let mut conn = Connection::<State>::connect().unwrap();
        let (wl_output, output_info) = crate::wl_session::get_wloutput_by_name(&mut conn, output);

        let shm = conn.bind_singleton::<WlShm>(2..=2).unwrap();
        let _dma = conn.bind_singleton::<ZwpLinuxDmabufV1>(4..=5).unwrap();
        let layer_shell = conn.bind_singleton::<ZwlrLayerShellV1>(4..=5).unwrap();
        let compositor = conn.bind_singleton::<WlCompositor>(1..=6).unwrap();
        let surface = compositor.create_surface(&mut conn);

        let width = output_info.mode.width;
        let height = output_info.mode.height;

        conn.blocking_roundtrip().unwrap();

        let layer_surface = layer_shell.get_layer_surface(&mut conn, surface, Some(wl_output), Layer::Background, CString::new("pandora").unwrap());
        layer_surface.set_size(&mut conn, width as u32, height as u32);
        layer_surface.set_anchor(&mut conn, Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right );
        layer_surface.set_exclusive_zone(&mut conn, -1);
        // set callback handler for 'layer_surface.configure' event
        conn.set_callback_for(layer_surface, layer_callback);
        conn.blocking_roundtrip().unwrap();
        surface.commit(&mut conn);
        conn.blocking_roundtrip().unwrap();

        let state = WaylandState {
            conn: conn,
            output: wl_output,
            shm: shm,
            compositor: compositor,
            layer_shell: layer_shell,
            surface: surface,
            output_info: output_info.mode,
        };

        self.state = Some(state);
    }

    fn render(&mut self, cmd: RenderCommand) {
        // i think all the as i32/u32's sprinkled around are going to cause problems
        // one day when someone uses some really fuckin' big images. whatever.
        if self.state.is_none() {
            self.initialize_wayland_handles(cmd.output.clone());
        }
        // todo: generally rewrite the buffer management here >.<
        // will eventually want to / need to support more pixel formats (at least for HDR).... thankfully I have an hdr monitor :)
        // for now.... rgba8. that's fine.
        let img = self.pandora.get_image(cmd.image).unwrap();
        let bytes_per_row: i32 = img.width() as i32 * 4;
        let total_bytes: i32 = bytes_per_row * img.height() as i32;
        let file = tempfile::tempfile().expect("creating tempfile for shared mem failed");
        
        let state = self.state.as_mut().unwrap();

        // TODO: scale image down to fit if mode static? :&
        // generally need to figure out some scaling logic here prior to scrolling

        img_into_buffer(&img, &file);

        let pool = state.shm.create_pool(&mut state.conn, OwnedFd::from(file), total_bytes);
        let buf = pool.create_buffer(&mut state.conn, 0, img.width() as i32, img.height() as i32, bytes_per_row, Format::Argb8888 );
        state.conn.blocking_roundtrip().unwrap();
        state.surface.attach(&mut state.conn, Some(buf), 0, 0); //hardcoded 0s l0l
        //state.surface.offset(&mut state.conn, 0, 0);
        state.surface.commit(&mut state.conn);
        state.conn.blocking_roundtrip().unwrap();
        // todo: enter into draw loop after this probably?
        loop {}
    }

    fn scroll(&self, _cmd: ScrollCommand) {
        // just scroll the canvas 4head
        todo!();
    }
}

fn img_into_buffer(img: &RgbaImage, f: &File) {
    let mut buf = std::io::BufWriter::new(f);
    for pixel in img.pixels() {
        let (r, g, b, a) = (pixel.0[0], pixel.0[1],pixel.0[2],pixel.0[3]);
        buf.write_all(&[b as u8, g as u8, r as u8, a as u8]).unwrap();
    }
}