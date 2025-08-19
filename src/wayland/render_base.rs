use crate::daemon::Daemon;
use crate::pithos::anims::spring::{Spring, SpringParams};
use crate::pithos::commands::{RenderMode};
use crate::pithos::config::DaemonConfig;


use std::fs::File;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use std::os::fd::OwnedFd;

use wayrs_client::global::GlobalExt;
use wayrs_client::{Connection, EventCtx, IoMode};
use wayrs_client::protocol::{WlBuffer, WlShm, wl_shm::Format, WlSurface, WlCallback, wl_output, WlOutput, WlCompositor, WlSubcompositor};
use wayrs_protocols::viewporter::{WpViewport, WpViewporter};
use wayrs_protocols::wlr_layer_shell_unstable_v1::{ZwlrLayerSurfaceV1};

// note: output resize/mode-setting changes are not handled here
// generic output plug/unplug thread handles start/stops for plug events

// maybe i do refactor all this first after all. appears to maybe be leaking buffers (?) during extended uptime
// i believe the refactor would allow for much better handling of buffer state and would prevent a lot of issues

pub struct RenderThreadState {
    pub outputs: Vec<(WlOutput, OutputState)>,
    pub globals: WaylandGlobals,
    pub pandora: Weak<dyn Daemon>,
}

impl RenderThreadState {
    pub fn get_outputs(&mut self, conn: &mut Connection<RenderThreadState>) {
        conn.blocking_roundtrip().unwrap();
        self.outputs = conn
                .globals()
                .iter()
                .filter(|g| g.is::<WlOutput>())
                .map(|g| g.clone())
                .collect::<Vec<_>>()
                .into_iter()
                .map(|g| g.bind_with_cb(conn, 2..=4, wl_output_cb).unwrap())
                .map(|output| (output, OutputState::default()))
                .collect();

        conn.flush(IoMode::Blocking).unwrap();

        while !self.outputs.iter().all(|x| x.1.done) {
            conn.recv_events(IoMode::Blocking).unwrap();
            conn.dispatch_events(self);
        }
    }

    pub fn is_animating(&self) -> bool {
        for (_, output_state) in &self.outputs {
            if let Some(render_state) = &output_state.render_state {
                match render_state {
                    OutputRenderStateVariety::Wallpaper(wallpaper_render_state) => {
                        if wallpaper_render_state.scroll_state.is_some() {
                            return true;
                        }
                    },
                    OutputRenderStateVariety::Lockscreen => (),
                }
            }
        }
        return false;
    }
}

#[derive(Default)]
pub struct OutputState {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub done: bool,
    pub render_state: Option<OutputRenderStateVariety>,
}

pub enum OutputRenderStateVariety {
    Wallpaper(WallpaperRenderState),
    Lockscreen,
}

pub struct WallpaperRenderState {
    pub surface: WlSurface,
    pub viewport: WpViewport,
    pub output_width: i32,
    pub output_height: i32,
    pub image: String,
    pub file: File,
    pub buffer: WlBuffer,
    pub image_width: i32,
    pub image_height: i32,
    pub mode: RenderMode,
    pub position_x: i32,
    pub position_y: i32,
    pub scroll_state: Option<ScrollState>, // should be None'd once scroll is finished
    pub slowdown: f64,
}

impl WallpaperRenderState {
    pub fn new(conn: &mut Connection<RenderThreadState>, globals: &WaylandGlobals, surface: WlSurface, image_path: &String, image_buf: File, image_width: i32, image_height: i32, mode: RenderMode, output_width: i32, output_height: i32, slowdown: f64) -> Self {
        let viewport = globals.viewporter.get_viewport(conn, surface);
        let bytes_per_row: i32 = image_width * 4;
        let total_bytes: i32 = bytes_per_row * image_height;
        let pool = globals.shm.create_pool(conn, OwnedFd::from(image_buf.try_clone().unwrap()), total_bytes);
        let buf = pool.create_buffer(conn, 0, image_width, image_height, bytes_per_row, Format::Argb8888 );
        surface.attach(conn, Some(buf), 0, 0);
        viewport.set_source(conn, 0.into(), 0.into(), output_width.into(), output_height.into());
        surface.commit(conn);
        conn.blocking_roundtrip().unwrap();

        return WallpaperRenderState {
            surface,
            viewport,
            output_width,
            output_height,
            image: image_path.clone(),
            image_width, image_height,
            file: image_buf, buffer: buf,
            position_x: 0, position_y: 0,
            scroll_state: None,
            mode: mode,
            slowdown,
        };
    }

    pub fn scroll(&mut self, conn: &mut Connection<RenderThreadState>, pos: i32) {
        let is_already_scrolling = self.scroll_state.is_some();
        let current_pos = match self.mode {
            RenderMode::Static => return,
            RenderMode::ScrollVertical => {
                if self.position_y == pos  {
                    return;
                }
                let end_bound = self.output_height + pos;
                if end_bound > self.image_height {
                    // would scroll past end and explode - invalid scroll command
                    // TODO: replumb so log functions are accessible here lol
                    return;
                }
                self.position_y
            },
            RenderMode::ScrollLateral => {
                if self.position_x == pos {
                    return;
                }
                let end_bound = self.output_width + pos;
                if end_bound <= self.image_width {
                    // would scroll past end and explode, do not scroll
                    return;
                }
                self.position_x
            }
        };

        let spring = Spring {
            from: current_pos as f64,
            to: pos as f64,
            initial_velocity: 0.0, // seed with initial velocity if interrupting an existing animation?
            params: SpringParams::default(),
        };

        self.scroll_state = Some(ScrollState {
            start_pos: current_pos,
            current_pos: current_pos,
            end_pos: pos,
            anim_start: Instant::now(),
            anim_duration: spring.duration(),
            anim: spring,
            slowdown: self.slowdown,
        });

        if !is_already_scrolling {
            self.start_scroll_anim(conn);
        }
    }

    fn start_scroll_anim(&mut self, conn: &mut Connection<RenderThreadState>) {
        self.surface.frame_with_cb(conn, frame_callback);
        self.do_scroll_tick(conn);
    }

    fn calc_next_pos(&self) -> i32 {
        let scroll_state = self.scroll_state.as_ref().unwrap();
        let eclipsed_duration = Instant::now() - scroll_state.anim_start;
        let seconds = eclipsed_duration.as_secs_f64() / scroll_state.slowdown;
        let scaled_duration = Duration::from_secs_f64(seconds);
        return scroll_state.anim.value_at(scaled_duration).round() as i32;
    }

    fn do_scroll_tick(&mut self, conn: &mut Connection<RenderThreadState>) {
        if self.scroll_state.is_none() {
            return; // probably an opportunistic dispatch for a surface w/o scroll state
        }
        let next_pos = self.calc_next_pos();
        self.scroll_to(conn, next_pos);
        let scroll_state = self.scroll_state.as_ref().unwrap();
        if scroll_state.is_animating() {
            self.surface.frame_with_cb(conn, frame_callback);
        } else {
            eprintln!("[render] dropping scroll state: done animating");
            self.scroll_state = None;
        }
    }

    fn scroll_to(
        &mut self,
        conn: &mut Connection<RenderThreadState>,
        new_pos: i32,
    ) {
        match self.mode {
            RenderMode::Static => {
                return; // ???
            }
            RenderMode::ScrollVertical => {
                self.position_x = 0;
                self.position_y = new_pos;
            },
            RenderMode::ScrollLateral => {
                self.position_x = new_pos;
                self.position_y = 0;
            },
        };
        self.scroll_state.as_mut().unwrap().current_pos = new_pos;
        self.viewport.set_source(conn, self.position_x.into(), self.position_y.into(), self.output_width.into(), self.output_height.into());
        self.surface.commit(conn);
        conn.blocking_roundtrip().unwrap();
    }
}

fn frame_callback(ctx: EventCtx<RenderThreadState, WlCallback>) {
    // fucking annoying: apparently no way to tell which surface the frame calback is for?
    // need to figure out a bool for is_waiting_for_next_frame or something
    // where we update that in the draw loop -> dispatch stuff
    // rly goofy ngl........
    for (_, os) in &mut ctx.state.outputs {
        if let Some(render_state) = &mut os.render_state {
            match render_state {
                OutputRenderStateVariety::Wallpaper(wallpaper_render_state) => {
                    wallpaper_render_state.do_scroll_tick(ctx.conn);
                },
                OutputRenderStateVariety::Lockscreen => (),
            }
        }
    }
}

pub struct ScrollState {
    pub start_pos: i32,
    pub current_pos: i32,
    pub end_pos: i32,
    pub anim_start: Instant,
    pub anim_duration: Duration, // only needed for LERP'd animations with fixed durations
    pub anim: Spring,
    pub slowdown: f64,
}

impl ScrollState {
    pub fn is_animating(&self) -> bool {
        // return (Instant::now() - self.anim_start) < self.anim_duration; -> sometimes drops early
        return self.current_pos != self.end_pos;
    }
}

pub struct WaylandGlobals {
    pub shm: WlShm,
    pub compositor: WlCompositor,
    pub subcompositor: WlSubcompositor,
    pub viewporter: WpViewporter,
}

impl WaylandGlobals {
    pub fn new(conn: &mut Connection<RenderThreadState>) -> WaylandGlobals {
        conn.blocking_roundtrip().unwrap();
        let shm = conn.bind_singleton::<WlShm>(2..=2).unwrap();
        //let dma = conn.bind_singleton::<ZwpLinuxDmabufV1>(4..=5).unwrap();
        let compositor = conn.bind_singleton::<WlCompositor>(1..=6).unwrap();
        let subcompositor = conn.bind_singleton::<WlSubcompositor>(1..=1).unwrap();
        let viewporter = conn.bind_singleton::<WpViewporter>(1..=1).unwrap();
        WaylandGlobals {
            shm,
            compositor,
            subcompositor,
            viewporter
        }
    }
}

pub fn initialize_wallpaper_outputs(conn: &mut Connection<RenderThreadState>, config: &DaemonConfig, state: &mut RenderThreadState) {
    // create main surface for each output, load image(s) onto subsurfaces
    // initial positions are (0,0)
    for (wl_output, output_state) in &mut state.outputs {
        let pandora = state.pandora.upgrade().unwrap();
        let output_config = match config.outputs.iter().find(|oc| oc.name == output_state.name) {
            Some(c) => c,
            None => {
                println!("output {} not found in configs; skipping", output_state.name);
                continue;
            }
        };
        let wallpaper_surface = state.globals.compositor.create_surface(conn);
        pandora.clone().apply_role_to_surface(conn, &wallpaper_surface, &wl_output, output_state);
        let file = tempfile::tempfile().expect("creating tempfile for shared mem failed");
        let mode = match output_config.mode {
            Some(mode) => mode,
            None => RenderMode::Static,
        };
        let (scaled_width, scaled_height) = image_to_file(
            pandora.clone(), &mode,
            &file, &output_config.image,
            output_state.width as u32, output_state.height as u32,
        );

        let wallpaper_state = WallpaperRenderState::new(conn, &state.globals,
            wallpaper_surface, &output_config.image, file,
            scaled_width as i32, scaled_height as i32, mode,
            output_state.width, output_state.height,
            config.animation.slowdown.max(0.001),
        );

        wallpaper_state.surface.commit(conn);
        conn.blocking_roundtrip().unwrap();
        conn.flush(IoMode::Blocking).unwrap();
        output_state.render_state = Some(crate::wayland::render_base::OutputRenderStateVariety::Wallpaper(wallpaper_state));
    }
}

fn image_to_file(pandora: Arc<dyn Daemon>, mode: &RenderMode, f: &File, path: &String,  width: u32, height: u32) -> (u32, u32) {
    // i decided that downscaling to minimize resource footprint while maximizing quality is mandatory
    // easier to reason about
    let scale_to = match mode {
        RenderMode::Static => Some((Some(width), Some(height))),
        RenderMode::ScrollVertical => Some((Some(width), None)),
        RenderMode::ScrollLateral => Some((None, Some(height))),
    };

    pandora.clone().load_image(path).unwrap();
    let (img_width, img_height) = pandora.clone().read_img_to_file(path, f, scale_to).unwrap();

    if img_width < width || img_height < height {
        // TODO: better handling of this case
        // idealy coerce to static at runtime and just log
        // im lazy for now tho
        panic!("image scaled to {img_width} x {img_height}, but output is {width} by {height}.\n   Try static mode for this image, as it's maybe insufficient for the desired mode :(")
    }
    pandora.clone().verbose("wallpaper", format!("file loaded and scaled to {img_width} x {img_height}"));
    return (img_width, img_height);
}

fn wl_output_cb(ctx: EventCtx<RenderThreadState, WlOutput>) {
    let outputs = &mut ctx.state.outputs;

    let output_state = &mut outputs.iter_mut()
        .find(|o| o.0 == ctx.proxy).unwrap().1;

    if output_state.done {
        println!("received event {:?} for output that's already .done - reseat?", ctx.event);
        return;
    }
    match ctx.event {
        wl_output::Event::Name(name) => output_state.name = name.into_string().unwrap(),
        wl_output::Event::Mode(mode) => {
            output_state.width = mode.width;
            output_state.height = mode.height;
        },
        // wl_output::Event::Scale(scale) => output.scale = Some(scale), // maybe track this for lockscreen element scaling?
        wl_output::Event::Done => output_state.done = true,
        _ => (),
    }
}

pub fn layer_shell_callback(mut ctx: EventCtx<RenderThreadState, ZwlrLayerSurfaceV1>) {
    let layer: ZwlrLayerSurfaceV1 = ctx.proxy;
    match ctx.event {
        wayrs_protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::Event::Configure(args) => {
            layer.ack_configure(&mut ctx.conn, args.serial);
        },
        _ => (),
    }
}