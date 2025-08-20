use crate::daemon::Daemon;
use crate::pithos::anims::spring::{Spring, SpringParams};
use crate::pithos::commands::RenderMode;
use crate::pithos::config::DaemonConfig;
use crate::wayland::render_base::OutputRenderStateVariety::Wallpaper;

use std::fs::File;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use wayrs_client::global::GlobalExt;
use wayrs_client::protocol::wl_registry::{self, GlobalArgs};
use wayrs_client::protocol::{
    WlBuffer, WlCallback, WlCompositor, WlOutput, WlShm, WlSubcompositor, WlSurface, wl_output,
    wl_shm::Format,
};
use wayrs_client::{Connection, EventCtx, IoMode};
use wayrs_protocols::viewporter::{WpViewport, WpViewporter};
use wayrs_protocols::wlr_layer_shell_unstable_v1::ZwlrLayerSurfaceV1;

// todo: split up into some smaller files perhaps

#[derive(Debug)]
pub struct Output {
    registry_name: u32,
    wl_output: WlOutput,
    name: Option<String>,
    done: bool,
}

impl Output {
    fn bind(conn: &mut Connection<RenderThreadState>, global: &GlobalArgs) -> Self {
        Self {
            registry_name: global.name,
            wl_output: global.bind_with_cb(conn, 3..=4, wl_output_cb).unwrap(),
            name: None,
            done: false,
        }
    }
}

pub struct RenderThreadState {
    pub outputs: Vec<(Output, OutputState)>,
    pub detached_outputs: Vec<OutputState>,
    pub globals: WaylandGlobals,
    pub pandora: Weak<dyn Daemon>,
}

impl RenderThreadState {
    pub fn get_outputs(&mut self, conn: &mut Connection<RenderThreadState>) {
        let num_outputs = conn
            .globals()
            .iter()
            .filter(|g| g.is::<WlOutput>())
            .map(|g| g.clone())
            .collect::<Vec<_>>()
            .len();

        conn.add_registry_cb(wl_registry_cb);
        conn.blocking_roundtrip().unwrap();
        conn.flush(IoMode::Blocking).unwrap();

        // kinda gross hack: first we block until the vec is appropriately sized
        while self.outputs.len() != num_outputs {
            conn.flush(IoMode::Blocking).unwrap();
            conn.recv_events(IoMode::Blocking).unwrap();
            conn.dispatch_events(self);
        }
        // then we make sure they're all .done
        while !self.outputs.iter().all(|x| x.1.done) {
            conn.flush(IoMode::Blocking).unwrap();
            conn.recv_events(IoMode::Blocking).unwrap();
            conn.dispatch_events(self);
        }
    }

    pub fn is_animating(&self) -> bool {
        for (_, output_state) in &self.outputs {
            if let Some(render_state) = &output_state.render_state {
                match render_state {
                    Wallpaper(wallpaper_render_state) => {
                        if wallpaper_render_state.scroll_state.is_some() {
                            return true;
                        }
                    }
                    //Lockscreen => (),
                }
            }
        }
        return false;
    }

    pub fn try_reseat_outputs(&mut self, conn: &mut Connection<RenderThreadState>) {
        // check for previous state in detached_outputs
        // match against existing outputs
        // update their output state with the reusable unplugged state fields (image path, mode, ?position?, scroll state)
        for (output, output_state) in &mut self.outputs {
            if let Some(idx) = self
                .detached_outputs
                .iter()
                .position(|prior| prior.name == output_state.name)
            {
                let mut prior_state = self.detached_outputs.swap_remove(idx);
                output_state.reseat(
                    conn,
                    output,
                    &mut prior_state,
                    self.pandora.clone(),
                    self.globals,
                );
            }
        }
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

impl OutputState {
    pub fn reseat(
        &mut self,
        conn: &mut Connection<RenderThreadState>,
        new_output: &mut Output,
        prior_state: &mut OutputState,
        weak: Weak<dyn Daemon>,
        globals: WaylandGlobals,
    ) {
        if let Some(kind) = &mut prior_state.render_state {
            let new_state = match kind {
                Wallpaper(wp_state) => Wallpaper(WallpaperRenderState::from_prior(
                    conn, weak, wp_state, new_output, self, globals,
                )),
            };
            self.render_state = Some(new_state);
        }
    }

    pub fn yeet(&mut self, conn: &mut Connection<RenderThreadState>) {
        if let Some(kind) = &self.render_state {
            match kind {
                Wallpaper(wp_state) => wp_state.yeet(conn),
            };
        }
    }
}

pub enum OutputRenderStateVariety {
    Wallpaper(WallpaperRenderState),
    //Lockscreen,
}

pub struct WallpaperRenderState {
    // un-rust-y: Wayland-y objects can be destroyed & be invalid references for use after output removal => buffer freeing
    // should refactor them to be Option<>s, .take() on destroy?
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
    pub slowdown: f64, // must be updated whenever config reload percolates to render thread
}

impl WallpaperRenderState {
    pub fn new(
        conn: &mut Connection<RenderThreadState>,
        globals: WaylandGlobals,
        weak: Weak<dyn Daemon>,
        image_path: &String,
        mode: RenderMode,
        wl_output: WlOutput,
        output_state: &OutputState,
        initial_pos: Option<(i32, i32)>,
        slowdown: f64,
    ) -> Self {
        // create new buffer, load image into it, attach it to surface, apply role through daemon
        let pandora = weak.upgrade().unwrap();
        let surface = globals.compositor.create_surface(conn);
        pandora
            .clone()
            .apply_role_to_surface(conn, &surface, &wl_output, output_state);
        let file = tempfile::tempfile().expect("creating tempfile for shared mem failed");

        let (scaled_width, scaled_height) = image_to_file(
            pandora.clone(),
            &mode,
            &file,
            image_path,
            output_state.width as u32,
            output_state.height as u32,
        );
        let bytes_per_row = scaled_width * 4;
        let total_bytes = bytes_per_row * scaled_height;

        // make a pool that consists of a single image. map that onto a single buffer.
        // not bothering to do one big pool with one big map and keeping track of byte offsets.
        let pool =
            globals
                .shm
                .create_pool(conn, OwnedFd::from(file.try_clone().unwrap()), total_bytes);
        let buf = pool.create_buffer(
            conn,
            0,
            scaled_width,
            scaled_height,
            bytes_per_row,
            Format::Argb8888,
        );
        let viewport = globals.viewporter.get_viewport(conn, surface);
        let (position_x, position_y) = match initial_pos {
            Some((x, y)) => {
                match mode {
                    RenderMode::Static => (0, 0),
                    RenderMode::ScrollVertical => {
                        if y + output_state.height <= scaled_height {
                            (0, y)
                        } else {
                            eprintln!("foo {y} {} {scaled_height}", output_state.height);
                            // debug log here
                            (0, 0)
                        }
                    }
                    RenderMode::ScrollLateral => {
                        if x + output_state.width <= scaled_width {
                            (x, 0)
                        } else {
                            // debug log here
                            (0, 0)
                        }
                    }
                }
            }
            None => (0, 0),
        };

        surface.attach(conn, Some(buf), 0, 0);
        viewport.set_source(
            conn,
            position_x.into(),
            position_y.into(),
            output_state.width.into(),
            output_state.height.into(),
        );
        surface.commit(conn);
        conn.blocking_roundtrip().unwrap();
        conn.flush(IoMode::Blocking).unwrap(); // can comment this out perhaps?

        return WallpaperRenderState {
            surface,
            viewport,
            output_width: output_state.width,
            output_height: output_state.height,
            image: image_path.clone(),
            image_width: scaled_width,
            image_height: scaled_height,
            file,
            buffer: buf,
            position_x,
            position_y,
            scroll_state: None,
            mode: mode,
            slowdown,
        };
    }

    pub fn yeet(&self, conn: &mut Connection<RenderThreadState>) {
        self.buffer.destroy(conn);
        self.viewport.destroy(conn);
        self.surface.destroy(conn);
        conn.blocking_roundtrip().unwrap();
    }

    pub fn from_prior(
        conn: &mut Connection<RenderThreadState>,
        weak: Weak<dyn Daemon>,
        unplugged: &mut WallpaperRenderState,
        output: &mut Output,
        output_state: &OutputState,
        globals: WaylandGlobals,
    ) -> Self {
        return WallpaperRenderState::new(
            conn,
            globals,
            weak,
            &unplugged.image,
            unplugged.mode,
            output.wl_output,
            output_state,
            Some((unplugged.position_x, unplugged.position_y)),
            unplugged.slowdown,
        );
    }

    pub fn scroll(&mut self, conn: &mut Connection<RenderThreadState>, pos: i32) {
        let is_already_scrolling = self.scroll_state.is_some();
        let current_pos = match self.mode {
            RenderMode::Static => return,
            RenderMode::ScrollVertical => {
                if self.position_y == pos {
                    return;
                }
                let end_bound = self.output_height + pos;
                if end_bound > self.image_height {
                    // would scroll past end and explode - invalid scroll command
                    // TODO: replumb so log functions are accessible here lol
                    return;
                }
                self.position_y
            }
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

    fn scroll_to(&mut self, conn: &mut Connection<RenderThreadState>, new_pos: i32) {
        match self.mode {
            RenderMode::Static => {
                return; // ???
            }
            RenderMode::ScrollVertical => {
                self.position_x = 0;
                self.position_y = new_pos;
            }
            RenderMode::ScrollLateral => {
                self.position_x = new_pos;
                self.position_y = 0;
            }
        };
        self.scroll_state.as_mut().unwrap().current_pos = new_pos;
        self.viewport.set_source(
            conn,
            self.position_x.into(),
            self.position_y.into(),
            self.output_width.into(),
            self.output_height.into(),
        );
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
                }
                //OutputRenderStateVariety::Lockscreen => (),
            }
        }
    }
}

#[derive(Clone, Copy)]
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

#[derive(Copy, Clone)]
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
            viewporter,
        }
    }
}

pub fn initialize_wallpaper_outputs(
    conn: &mut Connection<RenderThreadState>,
    config: &DaemonConfig,
    state: &mut RenderThreadState,
) {
    for (output, output_state) in &mut state.outputs {
        let output_config = match config
            .outputs
            .iter()
            .find(|oc| oc.name == output_state.name)
        {
            Some(c) => c,
            None => {
                println!(
                    "output {} not found in configs; skipping",
                    output_state.name
                );
                continue;
            }
        };
        let mode = match output_config.mode {
            Some(mode) => mode,
            None => RenderMode::Static,
        };

        let wallpaper_state = WallpaperRenderState::new(
            conn,
            state.globals,
            state.pandora.clone(),
            &output_config.image,
            mode,
            output.wl_output,
            output_state,
            None,
            config.animation.slowdown.max(0.001),
        );
        output_state.render_state =
            Some(crate::wayland::render_base::OutputRenderStateVariety::Wallpaper(wallpaper_state));
    }
}

fn image_to_file(
    pandora: Arc<dyn Daemon>,
    mode: &RenderMode,
    f: &File,
    path: &String,
    width: u32,
    height: u32,
) -> (i32, i32) {
    // enforcing a downscale to output_width at the 'load to buffer' stage is currently mandatory
    // so that the niri agent can reason better about viewport position upon scrolls and not the viewport scale
    // however, this makes re-initializing surfaces after an output mode change or output disconnect/reconnect
    // a bit costly as we have to re-scale the image and write it out to another buffer again
    // need to address this at some point, I think
    // the 'best' option I can presently think of would be to give the render thread more control of viewport positioning
    // by reworking the scroll command to just be float percentile based for (x,y) (start, end) ?
    // i am looping back to "how i approach the scroll command is gonna be weird and dependent on where i want to draw lines in the sand about state/ownership"
    // and re-evaluating some tradeoffs from the other end of implementation
    //
    // the niri window-geometry ipc PR was merged though, which means i can take stabs at the lateral and Both parallax
    // implementations now - i suspect a scroll percentage is gonna be the way we wanna go
    // (with maybe an optional hint for # of total 'slots' we have if we want to do smooth stepping e.g. full screen at a time)
    // yeah that sounds like the best of both worlds?
    // will mean offloading a good bit of state out of the niri agent too
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
        panic!(
            "image scaled to {img_width} x {img_height}, but output is {width} by {height}.\n   Try static mode for this image, as it's maybe insufficient for the desired mode :("
        )
    }
    pandora.clone().verbose(
        "wallpaper",
        format!("file loaded and scaled to {img_width} x {img_height}"),
    );
    return (img_width as i32, img_height as i32);
}

fn wl_registry_cb(
    conn: &mut Connection<RenderThreadState>,
    state: &mut RenderThreadState,
    event: &wl_registry::Event,
) {
    match event {
        wl_registry::Event::Global(global) if global.is::<WlOutput>() => {
            let output = Output::bind(conn, global);
            match state
                .outputs
                .iter_mut()
                .find(|(o, _)| o.wl_output == output.wl_output)
            {
                Some(_) => {
                    println!("BUG: output already in stack?");
                }
                None => {
                    //eprintln!("plug event: [{output:?}] [{event:?}]",);
                    state.outputs.push((output, OutputState::default()));
                    state.try_reseat_outputs(conn);
                    // todo: investigate hitch? might be from image load?
                }
            };
        }
        wl_registry::Event::GlobalRemove(name) => {
            //eprintln!("remove event [{event:?}]");
            if let Some(i) = state
                .outputs
                .iter()
                .position(|(o, _)| o.registry_name == *name)
            {
                let (output, mut output_state) = state.outputs.swap_remove(i);
                output.wl_output.release(conn);
                output_state.yeet(conn);
                state.detached_outputs.push(output_state);
            }
        }
        _ => (),
    }
}

fn wl_output_cb(ctx: EventCtx<RenderThreadState, WlOutput>) {
    let outputs = &mut ctx.state.outputs;
    let (output, output_state) = &mut outputs
        .iter_mut()
        .find(|o| o.0.wl_output == ctx.proxy)
        .unwrap();

    if output.done {
        println!(
            "received event {:?} for output that's already .done - reseat?",
            ctx.event
        );
        return;
    }
    match ctx.event {
        wl_output::Event::Name(name) => {
            output_state.name = name.clone().into_string().unwrap();
            output.name = Some(name.into_string().unwrap());
        }
        wl_output::Event::Mode(mode) => {
            output_state.width = mode.width;
            output_state.height = mode.height;
        }
        // wl_output::Event::Scale(scale) => output.scale = Some(scale), // maybe track this for lockscreen element scaling?
        wl_output::Event::Done => {
            output_state.done = true;
            output.done = true;
            ctx.state.try_reseat_outputs(ctx.conn);
        }
        _ => (),
    }
}

pub fn layer_shell_callback(mut ctx: EventCtx<RenderThreadState, ZwlrLayerSurfaceV1>) {
    let layer: ZwlrLayerSurfaceV1 = ctx.proxy;
    match ctx.event {
        wayrs_protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::Event::Configure(
            args,
        ) => {
            layer.ack_configure(&mut ctx.conn, args.serial);
        }
        _ => (),
    }
}
