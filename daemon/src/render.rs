use pithos::anims::spring::{Spring, SpringParams};
use pithos::commands::{RenderCommand, RenderMode, ScrollCommand, ThreadCommand};
use pithos::error::DaemonError;
use pithos::wayland::render_helpers::{get_wloutput_by_name, OutputMode, RenderState, RenderThreadWaylandState, ScrollState};

use crate::pandora::Pandora;

use std::ffi::CString;
use std::sync::mpsc::{Sender, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};
use std::os::fd::OwnedFd;

use wayrs_client::{Connection, EventCtx, IoMode};
use wayrs_client::protocol::{WlShm, wl_shm::Format, WlSurface, WlCallback, WlOutput, WlCompositor};

use wayrs_protocols::linux_dmabuf_v1::ZwpLinuxDmabufV1;
use wayrs_protocols::viewporter::{WpViewport, WpViewporter};
use wayrs_protocols::wlr_layer_shell_unstable_v1::{ZwlrLayerShellV1, ZwlrLayerSurfaceV1, zwlr_layer_surface_v1::Anchor, zwlr_layer_shell_v1::Layer};
// note: output resize/mode-setting changes are currently not handled here
// the "best" solution will probably involve a new command from the compositor agent
// that will be sent upon an output mode/resolution change
// currently, when my desktop sleeps for a while, it looks like my main monitor 'disconnects'
// and then reconnects - and can't be recovered without restarting the daemon presently.
// definitely work to be done ! 

#[derive(Copy, Clone)]
pub struct RenderThreadWaylandGlobals {
    _output: WlOutput,
    output_info: OutputMode,
    shm: WlShm, // shared mem singleton
    _dma: ZwpLinuxDmabufV1,
    _compositor: WlCompositor,
    _layer_shell: ZwlrLayerShellV1,
    surface: WlSurface,
    _viewporter: WpViewporter,
    viewport: WpViewport,
}

pub enum RenderVariant {
    Wallpaper,
    Lockscreen,
}

pub struct RenderThread {
    name: String,
    kind: RenderVariant,
    receiver: Receiver<ThreadCommand>,
    _sender: Sender<String>, 
    pandora: Arc<Pandora>,
    conn: Connection<RenderThreadWaylandState>,
    globals: Option<RenderThreadWaylandGlobals>,
    // state below, ough
    render_state: Option<RenderState>,
}

fn layer_callback(mut ctx: EventCtx<RenderThreadWaylandState, ZwlrLayerSurfaceV1>) {
    let layer: ZwlrLayerSurfaceV1 = ctx.proxy;
    match ctx.event {
        wayrs_protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::Event::Configure(args) => {
            layer.ack_configure(&mut ctx.conn, args.serial);
        },
        _ => (),
    }
}

fn initialize_wayland_handles(conn: &mut Connection<RenderThreadWaylandState>, output: String, variant: &RenderVariant) -> RenderThreadWaylandGlobals {
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
    match variant {
        RenderVariant::Wallpaper => {
            let layer_surface = layer_shell.get_layer_surface(conn, surface, Some(wl_output), Layer::Background, CString::new("pandora").unwrap());
            
            layer_surface.set_size(conn, width as u32, height as u32);
            layer_surface.set_anchor(conn, Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right );
            layer_surface.set_exclusive_zone(conn, -1);
            
            // set callback handler for 'layer_surface.configure' event
            conn.set_callback_for(layer_surface, layer_callback);
        },
        RenderVariant::Lockscreen => {
            // need to get_lock_surface from ExtSessionLockV1, need to share wayland connection between threads. hmmm. uh oh.
            // might be time to refactor this all to be Arc<>s, have one global wayland connection that display....
            // - oh god, the render state in the connection for the frame callbacks makes this kinda annoying
            // alternatively, i guess i can go cleanly implement a lock thread that handles all this in-house, and then
            // refactor the wallpaper threads to be.... better, so that it's able to just pull the buffers & viewport state out
            // that sounds better, I guess. hmm.
            // will need to delete this bit of code later bc it's not gonna be used here lol
        },
    }

    surface.commit(conn);
    conn.blocking_roundtrip().unwrap();

    return RenderThreadWaylandGlobals {
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

impl RenderThread {
    pub fn new(output: String, recv: Receiver<ThreadCommand>, send: Sender<String>, pandora: Arc<Pandora>, conn: Connection<RenderThreadWaylandState>, variant: RenderVariant) -> RenderThread {
        return RenderThread {
            name: output,
            kind: variant,
            receiver: recv,
            _sender: send,
            pandora: pandora,
            conn: conn,
            globals: None,
            render_state: None,
        }
    }

    fn log(&self, s: String) {
        println!("> {}: {s}", self.name);
    }

    fn respond(&self, s: String) {
        self._sender.send(s).expect("exploded when sending response to daemon");
    }

    pub fn start(&mut self) {
        let cmd = self.receiver.recv().expect("thread exploded while waiting on first command recv");
        match cmd {
            ThreadCommand::Render(c) => {
                self.render(&c).expect("Error initializing render thread");
            }
            _ => {
                panic!("invalid initial command received (should be Render");
            }
        }
        self.draw_loop();
    }

    fn end(&mut self) {
        let globals = self.globals.take().unwrap();
        let render_state = self.render_state.take().unwrap();
        render_state.buffer.destroy(&mut self.conn);
        render_state.bufpool.destroy(&mut self.conn);
        globals.viewport.destroy(&mut self.conn);
        globals._viewporter.destroy(&mut self.conn);
        globals._layer_shell.destroy(&mut self.conn);
        globals._dma.destroy(&mut self.conn);
        globals.surface.destroy(&mut self.conn);
        self.log("goodbye!".to_string());
    }

    // todo: generally rewrite the buffer management >.<
    // will eventually want to / need to support more pixel formats (at least for HDR)....
    // thankfully I have an hdr monitor :) ... but for now.... rgba8. that's fine.

    fn render(&mut self, cmd: &RenderCommand) -> Result<(), DaemonError> {
        if self.globals.is_none() {
            let globals = initialize_wayland_handles(&mut self.conn, cmd.output.clone(), &self.kind);
            self.globals = Some(globals);
        }
        if self.render_state.is_some() { // could try to transition old state to new state/animate, maybe.
            let render_state = self.render_state.take().unwrap();
            render_state.buffer.destroy(&mut self.conn);
            render_state.bufpool.destroy(&mut self.conn);
        }
        let globals = self.globals.take().unwrap();
        let file = tempfile::tempfile().expect("creating tempfile for shared mem failed");
        let (output_width, output_height) = (globals.output_info.width as u32, globals.output_info.height as u32);

        // TODO: have an option for to omit this
        // might "want" bigger images as it will give more positional granularity for scroll spring nonsense
        let scale_to = match cmd.mode {
            RenderMode::Static => Some((Some(output_width), Some(output_height))),
            RenderMode::ScrollingVertical(_) => Some((Some(output_width), None)),
            RenderMode::ScrollingLateral(_) => Some((None, Some(output_height)))
        };

        let (img_width, img_height) = self.pandora.read_img_to_file(&cmd.image, &file, scale_to)?;
        self.log(format!("file loaded and scaled to {img_width} x {img_height}"));

        // self.pandora.drop_img_from_cache(&cmd.image).expect("error dropping image from cache, somehow");

        self.log(format!("loaded image w/ dims {} x {}", img_width, img_height));
        let bytes_per_row: i32 = img_width as i32 * 4;
        let total_bytes: i32 = bytes_per_row * img_height as i32;

        let pool = globals.shm.create_pool(&mut self.conn, OwnedFd::from(file.try_clone().unwrap()), total_bytes);
        let buf = pool.create_buffer(&mut self.conn, 0, img_width as i32, img_height as i32, bytes_per_row, Format::Argb8888 );
        globals.surface.attach(&mut self.conn, Some(buf), 0, 0); //hardcoded 0s l0l
        

        self.log(format!("cropping surface view to {output_width} x {output_height}"));

        let scroll_state = match cmd.mode {
            RenderMode::Static => {
                globals.viewport.set_destination(&mut self.conn,
                    globals.output_info.width, globals.output_info.height,
                );

                // center image
                let width_offset = (img_width - output_width) / 2;
                let height_offset = (img_height - output_height) / 2;

                self.log(format!("static mode: offset[{} x {}] geom[{} x {}]", width_offset, height_offset, output_width, output_height));

                globals.viewport.set_source(&mut self.conn,
                    width_offset.into(), height_offset.into(),
                    output_width.into(), output_height.into(),
                );
                None
            }
            RenderMode::ScrollingVertical(offset) => {
                Some(ScrollState {
                    start_pos: offset,
                    current_pos: offset,
                    end_pos: offset,
                    anim_start: Instant::now(),
                    anim_duration: Duration::ZERO,
                    anim: Spring {
                        from: offset as f64,
                        to: offset as f64,
                        initial_velocity: 0.0,
                        params: SpringParams::default(),
                    },
                    _frame_count: 0,
                })
            },
            RenderMode::ScrollingLateral(offset) => {
                Some(ScrollState {
                    start_pos: offset,
                    current_pos: offset,
                    end_pos: offset,
                    anim_start: Instant::now(),
                    anim_duration: Duration::ZERO,
                    anim: Spring {
                        from: offset as f64,
                        to: offset as f64,
                        initial_velocity: 0.0,
                        params: SpringParams::default(),
                    },
                    _frame_count: 0,
                })
            },
        };

        self.globals = Some(globals);
        self.render_state = Some(RenderState {
            mode: cmd.mode,
            _img_path: cmd.image.clone(),
            _buf_file: file,
            buffer: buf,
            bufpool: pool,
            scrolling: scroll_state,
            crop_width: output_width,
            crop_height: output_height,
            orig_width: img_width,
            orig_height: img_height,
        });

        if scroll_state.is_some() {
            self.scroll_surface_to(scroll_state.unwrap().current_pos);
        } 

        globals.surface.commit(&mut self.conn);
        self.conn.blocking_roundtrip().unwrap();
        Ok(())
    }

    fn draw_loop(&mut self) {
        loop {
            // this is still kinda gross. needs rewriting still.
            self.conn.flush(IoMode::Blocking).unwrap();
            let received_events = self.conn.recv_events(IoMode::NonBlocking);

            // set up dispatch state. animation_state is the only mut, rest are just refs we need.
            let mut dispatch_state = RenderThreadWaylandState::default();
            dispatch_state.render_state = self.render_state.take(); // must be put back!!
            dispatch_state.viewport = Some(self.globals.as_ref().unwrap().viewport);
            dispatch_state.surface = Some(self.globals.as_ref().unwrap().surface);
            dispatch_state.output_info = Some(self.globals.as_ref().unwrap().output_info);

            self.conn.dispatch_events(&mut dispatch_state);
            self.render_state = dispatch_state.render_state;

            self.handle_inbound_commands();

            if received_events.is_err() { // did not process any animation commands this tick; block on command queue lazy style
                let scroll_state = self.render_state.as_ref().unwrap().scrolling.as_ref();
                if scroll_state.is_none() || !is_animating(scroll_state.unwrap()) {
                    // not animating currently - BLOCK AND WAIT HERE
                    self.handle_cmd(&self.receiver.recv().expect("thread exploded during blocking read on inbound commands"));
                }
            }
        }
    }

    // returns true if it's time to exit e.g. received stop
    fn handle_inbound_commands(&mut self) {
        loop {
            match self.receiver.try_recv() {
                Ok(cmd) => self.handle_cmd(&cmd),
                Err(_) => break,
            }
        }
    }

    fn handle_cmd(&mut self, cmd: &ThreadCommand) {
        match cmd {
            ThreadCommand::Render(c) => {
                self.render(c).expect("error handling render command");
            }
            ThreadCommand::Stop(_) => {
                self.end();
            }
            ThreadCommand::Scroll(c) => {
                self.scroll(c);
            }
        }
    }

    fn scroll_surface_to(&mut self, pos: u32) {
        let globals = self.globals.as_mut().unwrap();
        let mut state = self.render_state.take().unwrap();
        do_scroll_step(&mut self.conn, &mut state,
            &globals.viewport, &globals.output_info, &globals.surface, pos);
        self.render_state = Some(state);
    }

    fn scroll(&mut self, cmd: &ScrollCommand) {
        let is_already_scrolling  = self.is_scrolling();
        let mut render_state = self.render_state.take().unwrap();
        let mut scroll_state = render_state.scrolling.take().unwrap();
        // validate command/position before we commit to scrolling
        let valid = match render_state.mode {
            RenderMode::Static => false, // nothing to do here!
            RenderMode::ScrollingVertical(_) => {
                let end_bound = render_state.crop_height + cmd.position;
                end_bound <= render_state.orig_height
            },
            RenderMode::ScrollingLateral(_) => {
                let end_bound = render_state.crop_width + cmd.position;
                end_bound <= render_state.orig_width
            }
        };
        if !valid {
            self.log("would scroll past end and explode".to_string());
            render_state.scrolling = Some(scroll_state);
            self.render_state = Some(render_state);
            return;
        }

        scroll_state.start_pos = scroll_state.current_pos; // current pos should always be updated in scroll_to
        scroll_state.end_pos = cmd.position;
        scroll_state.anim_start = Instant::now();

        scroll_state.anim.from = scroll_state.current_pos as f64;
        scroll_state.anim.to = cmd.position as f64;
        scroll_state.anim.initial_velocity = 0.0; // TODO: figure out how to determine initial velocity if is_already_scrolling!
        scroll_state._frame_count = 0;

        scroll_state.anim_duration = scroll_state.anim.duration();

        render_state.scrolling = Some(scroll_state);
        self.render_state = Some(render_state);

        self.respond(format!("scrolling from {} to {}, expected duration {:?} (was already scrolling: {})", scroll_state.start_pos, scroll_state.end_pos, scroll_state.anim_duration, is_already_scrolling));

        if !is_already_scrolling {
            self.start_scroll_anim();
        }
    }

    fn start_scroll_anim(&mut self) {
        let mut render_state = self.render_state.take().unwrap();
        let globals = self.globals.as_ref().unwrap();

        self.log(format!("animation starting. start_pos: {}, end_pos: {}",
            render_state.scrolling.unwrap().start_pos,
            render_state.scrolling.unwrap().end_pos));
    
        globals.surface.frame_with_cb(&mut self.conn, frame_callback);
        let next_pos = calc_next_pos(&render_state);
        do_scroll_step(&mut self.conn, &mut render_state,
            &globals.viewport,
            &globals.output_info,
            &globals.surface,
            next_pos,
        );
        self.render_state = Some(render_state);
    }

    fn is_scrolling(&self) -> bool {
        let render_state = self.render_state.as_ref().unwrap();
        let scroll_state = render_state.scrolling.as_ref();
        if scroll_state.is_some() {
            return is_animating(scroll_state.unwrap());
        } else {
            return false;
        }
    }
}

fn calc_next_pos(render_state: &RenderState) -> u32 {
    let scroll_state = render_state.scrolling.as_ref().unwrap();
    let eclipsed_duration = Instant::now() - scroll_state.anim_start;
    let ret = scroll_state.anim.value_at(eclipsed_duration).round() as u32;
    return ret;
}

fn do_scroll_step(conn: &mut Connection<RenderThreadWaylandState>,
    render_state: &mut RenderState,
    viewport: &WpViewport,
    output_info: &OutputMode,
    surface: &WlSurface,
    next_pos: u32,
) {
    match render_state.mode {
        RenderMode::Static => {
            // THIS SHOULD BE A NOP / INVALID COMMAND IDK
        }
        RenderMode::ScrollingVertical(_) => {
            viewport.set_destination(conn,
                output_info.width, output_info.height,
            );
            viewport.set_source(conn,
                wayrs_client::Fixed::ZERO, next_pos.into(),
                render_state.crop_width.into(), render_state.crop_height.into(),
            );
        },
        RenderMode::ScrollingLateral(_) => {
            viewport.set_destination(conn,
                output_info.width, output_info.height,
            );
            viewport.set_source(conn,
                next_pos.into(), wayrs_client::Fixed::ZERO,
                render_state.crop_width.into(), render_state.crop_height.into(),
            );
        },
    };
    let mut scroll_state = render_state.scrolling.take().unwrap();
    scroll_state.current_pos = next_pos;
    render_state.scrolling = Some(scroll_state);
    surface.commit(conn);
    conn.blocking_roundtrip().unwrap();
}

fn frame_callback(ctx: EventCtx<RenderThreadWaylandState, WlCallback>) {
    let wl_state = ctx.state;
    
    let mut render_state = wl_state.render_state.take().unwrap();
    let new_pos = calc_next_pos(&render_state);
    render_state.scrolling.as_mut().unwrap()._frame_count += 1;

    if is_animating(render_state.scrolling.as_ref().unwrap()) {
        wl_state.surface.unwrap().frame_with_cb(ctx.conn, frame_callback);
        do_scroll_step(ctx.conn, &mut render_state,
            &wl_state.viewport.unwrap(), &wl_state.output_info.unwrap(), &wl_state.surface.unwrap(), new_pos
        );
    }
    wl_state.render_state = Some(render_state);
}

fn is_animating(state: &ScrollState) -> bool {
    return (Instant::now() - state.anim_start) < state.anim_duration;
}