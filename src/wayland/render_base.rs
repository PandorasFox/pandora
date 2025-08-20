use crate::daemon::Daemon;
use crate::pithos::anims::spring::{Spring, SpringParams};
use crate::pithos::commands::RenderMode;
use crate::pithos::config::DaemonConfig;
use crate::pithos::misc::{compute_viewport_range, get_viewport_dimensions};
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
                        if wallpaper_render_state.scroll_state_x.is_some() {
                            return true;
                        }
                        if wallpaper_render_state.scroll_state_y.is_some() {
                            return true;
                        }
                    } //Lockscreen => (),
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
    // scroll state is getting gross, but it needs.... a lot of these values, so, idk.
    // ScrollDimension(output, image, scroll_percent, viewport_x, viewport_size) ?
    // hmmmm.
    // Would Be Good lol
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
    pub scroll_percent_x: f64, // 0.0 => 100.0
    pub scroll_percent_y: f64,
    pub scroll_state_x: Option<ScrollState>, // should be None'd once scroll is finished
    pub scroll_state_y: Option<ScrollState>, // should be None'd once scroll is finished
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
        scroll_percents: Option<(f64, f64)>,
        slowdown: f64,
    ) -> Self {
        // create new buffer, load image into it, attach it to surface, apply role through daemon
        let pandora = weak.upgrade().unwrap();
        let surface = globals.compositor.create_surface(conn);
        pandora
            .clone()
            .apply_role_to_surface(conn, &surface, &wl_output, output_state);
        let file = tempfile::tempfile().expect("creating tempfile for shared mem failed");

        let (image_width, image_height) = image_to_file(
            pandora.clone(),
            &mode,
            &file,
            image_path,
            output_state.width as u32,
            output_state.height as u32,
        );
        let bytes_per_row = image_width * 4;
        let total_bytes = bytes_per_row * image_height;

        // make a pool that consists of a single image; map that onto a single buffer.
        // not bothering to do one big pool with one big map and keeping track of byte offsets.
        let pool =
            globals
                .shm
                .create_pool(conn, OwnedFd::from(file.try_clone().unwrap()), total_bytes);
        let buf = pool.create_buffer(
            conn,
            0,
            image_width,
            image_height,
            bytes_per_row,
            Format::Argb8888,
        );
        let viewport = globals.viewporter.get_viewport(conn, surface);
        let (scroll_percent_x, scroll_percent_y) = match scroll_percents {
            Some((x, y)) => (x, y),
            None => (0.0, 0.0),
        };

        let (viewport_width, viewport_height) = get_viewport_dimensions(
            image_width,
            image_height,
            output_state.width,
            output_state.height,
            mode,
        );
        let (viewport_x_start, viewport_x_end) =
            compute_viewport_range(image_width, viewport_width, scroll_percent_x);
        let (viewport_y_start, viewport_y_end) =
            compute_viewport_range(image_height, viewport_height, scroll_percent_y);
        let viewport_width = viewport_x_end - viewport_x_start;
        let viewport_height = viewport_y_end - viewport_y_start;

        surface.attach(conn, Some(buf), 0, 0);
        viewport.set_destination(conn, output_state.width, output_state.height);

        viewport.set_source(
            conn,
            viewport_x_start.into(),
            viewport_y_start.into(),
            viewport_width.into(),
            viewport_height.into(),
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
            image_width: image_width,
            image_height: image_height,
            file,
            buffer: buf,
            scroll_percent_x,
            scroll_percent_y,
            scroll_state_x: None,
            scroll_state_y: None,
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
        // todo: file reuse from unplugged
        return WallpaperRenderState::new(
            conn,
            globals,
            weak,
            &unplugged.image,
            unplugged.mode,
            output.wl_output,
            output_state,
            Some((unplugged.scroll_percent_x, unplugged.scroll_percent_y)),
            unplugged.slowdown,
        );
    }

    pub fn scroll(
        &mut self,
        conn: &mut Connection<RenderThreadState>,
        scroll_x: f64,
        scroll_y: f64,
    ) {
        // scroll (x|y) track the % (0.0 => 100.0) of the center of the viewport along each dimension
        // e.g. 0.0 scroll_x is left-aligned, 100.0 is right aligned, and 50.0 has the viewport centered
        let (viewport_width, viewport_height) = get_viewport_dimensions(
            self.image_width,
            self.image_height,
            self.output_width,
            self.output_height,
            self.mode,
        );
        let is_already_scrolling = self.scroll_state_x.is_some() || self.scroll_state_y.is_some();

        // match on mode:
        // - if vertical, we use scroll_y, scroll_x is ignored (50.0 because centered & fill)
        // - if lateral, use scroll_x, scroll_y is ignored
        // - if static, RETURN!
        // - if full scroll, TODO :)
        //
        // we need to convert the given scroll % into a viewport start value, then:
        // update existing scroll state if we have one,
        // or calc_scroll_pos for that dim => compare => return if NOP => set state and finish otherwise

        let (viewport_x, viewport_y) = match self.mode {
            RenderMode::Static => return,
            RenderMode::ScrollVertical => {
                let (start_y, _end_y) =
                    compute_viewport_range(self.image_height, viewport_height, scroll_y);
                (0.0, start_y)
            }
            RenderMode::ScrollLateral => {
                let (start_x, _end_x) =
                    compute_viewport_range(self.image_width, viewport_width, scroll_x);
                (start_x, 0.0)
            }
        };

        // TODO: determine why multi-scroll interrupts weird style

        match self.mode {
            RenderMode::Static => (),
            RenderMode::ScrollVertical => {
                let current_pos: f64 = self.calc_pos_y(); // :)
                match &mut self.scroll_state_y {
                    Some(state) => {
                        state.start_pos = current_pos;
                        state.anim.from = current_pos;
                        state.end_pos = viewport_y;
                        state.anim.to = viewport_y;

                        state.anim_start = Instant::now();
                        state.anim_duration = state.anim.duration();
                    }
                    None => {
                        let spring = Spring {
                            from: current_pos,
                            to: viewport_y,
                            initial_velocity: 0.0, // seed with initial velocity if interrupting an existing animation?
                            params: SpringParams::default(),
                        };

                        self.scroll_state_y = Some(ScrollState {
                            start_pos: current_pos,
                            current_pos: current_pos,
                            end_pos: viewport_y,
                            anim_start: Instant::now(),
                            anim_duration: spring.duration(),
                            anim: spring,
                            slowdown: self.slowdown,
                        });
                    }
                }
            }
            RenderMode::ScrollLateral => {
                let current_pos = self.calc_pos_x();
                match &mut self.scroll_state_x {
                    Some(state) => {
                        state.start_pos = current_pos;
                        state.anim.from = current_pos;
                        state.end_pos = viewport_x;
                        state.anim.to = viewport_x;
                        state.anim_start = Instant::now();
                        state.anim_duration = state.anim.duration();
                    }
                    None => {
                        let current_pos = self.calc_pos_x(); // :)
                        let spring = Spring {
                            from: current_pos as f64,
                            to: viewport_x,
                            initial_velocity: 0.0, // seed with initial velocity if interrupting an existing animation?
                            params: SpringParams::default(),
                        };

                        self.scroll_state_x = Some(ScrollState {
                            start_pos: current_pos,
                            current_pos: current_pos,
                            end_pos: viewport_x,
                            anim_start: Instant::now(),
                            anim_duration: spring.duration(),
                            anim: spring,
                            slowdown: self.slowdown,
                        });
                    }
                }
            }
        }

        self.scroll_percent_x = scroll_x;
        self.scroll_percent_y = scroll_y;

        if !is_already_scrolling {
            self.start_scroll_anim(conn);
        }
    }

    fn start_scroll_anim(&mut self, conn: &mut Connection<RenderThreadState>) {
        self.surface.frame_with_cb(conn, frame_callback);
        self.do_scroll_tick(conn);
    }

    fn calc_pos_x(&self) -> f64 {
        if let Some(scroll_state) = self.scroll_state_x {
            let eclipsed_duration = Instant::now() - scroll_state.anim_start;
            let seconds = eclipsed_duration.as_secs_f64() / scroll_state.slowdown;
            let scaled_duration = Duration::from_secs_f64(seconds);
            return scroll_state.anim.value_at(scaled_duration).round();
        } else {
            let (viewport_width, _) = get_viewport_dimensions(
                self.image_width,
                self.image_height,
                self.output_width,
                self.output_height,
                self.mode,
            );
            let (viewport_x_start, _) =
                compute_viewport_range(self.image_width, viewport_width, self.scroll_percent_x);
            return viewport_x_start;
        }
    }

    fn calc_pos_y(&self) -> f64 {
        if let Some(scroll_state) = self.scroll_state_y {
            let eclipsed_duration = Instant::now() - scroll_state.anim_start;
            let seconds = eclipsed_duration.as_secs_f64() / scroll_state.slowdown;
            let scaled_duration = Duration::from_secs_f64(seconds);
            return scroll_state.anim.value_at(scaled_duration).round();
        } else {
            let (_, viewport_height) = get_viewport_dimensions(
                self.image_width,
                self.image_height,
                self.output_width,
                self.output_height,
                self.mode,
            );
            let (viewport_y_start, _) =
                compute_viewport_range(self.image_height, viewport_height, self.scroll_percent_y);
            return viewport_y_start;
        }
    }

    fn do_scroll_tick(&mut self, conn: &mut Connection<RenderThreadState>) {
        if self.scroll_state_x.is_none() && self.scroll_state_y.is_none() {
            return; // probably an opportunistic dispatch for a surface w/o scroll state
        }

        let next_pos_x = self.calc_pos_x();
        let next_pos_y = self.calc_pos_y();
        //println!("> [render] scrolling to {next_pos_x},{next_pos_y}");
        self.scroll_to(conn, next_pos_x, next_pos_y);

        if self.scroll_state_x.is_some_and(|s| s.is_animating())
            || self.scroll_state_y.is_some_and(|s| s.is_animating())
        {
            self.surface.frame_with_cb(conn, frame_callback);
        }
        match self.scroll_state_x {
            Some(mut state) => {
                if !state.is_animating() {
                    self.scroll_state_x = None
                } else {
                    state.current_pos = next_pos_x;
                }
            }
            None => (),
        }
        match self.scroll_state_y {
            Some(mut state) => {
                if !state.is_animating() {
                    self.scroll_state_y = None
                } else {
                    state.current_pos = next_pos_y;
                }
            }
            None => (),
        }
    }

    fn scroll_to(&mut self, conn: &mut Connection<RenderThreadState>, pos_x: f64, pos_y: f64) {
        let (viewport_width, viewport_height) = get_viewport_dimensions(
            self.image_width,
            self.image_height,
            self.output_width,
            self.output_height,
            self.mode,
        );
        let (viewport_x_start, viewport_x_end) =
            compute_viewport_range(self.image_width, viewport_width, self.scroll_percent_x);
        let (viewport_y_start, viewport_y_end) =
            compute_viewport_range(self.image_height, viewport_height, self.scroll_percent_y);
        let viewport_width = viewport_x_end - viewport_x_start;
        let viewport_height = viewport_y_end - viewport_y_start;

        self.viewport.set_source(
            conn,
            pos_x.into(),
            pos_y.into(),
            viewport_width.into(),
            viewport_height.into(),
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
                    if wallpaper_render_state.scroll_state_x.is_some()
                        || wallpaper_render_state.scroll_state_y.is_some()
                    {
                        wallpaper_render_state.do_scroll_tick(ctx.conn);
                    }
                } //OutputRenderStateVariety::Lockscreen => (),
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct ScrollState {
    pub start_pos: f64,
    pub current_pos: f64,
    pub end_pos: f64,
    pub anim_start: Instant,
    pub anim_duration: Duration, // only needed for LERP'd animations with fixed durations
    pub anim: Spring,
    pub slowdown: f64,
}

impl ScrollState {
    pub fn is_animating(&self) -> bool {
        return (Instant::now() - self.anim_start) < self.anim_duration;
        //return self.current_pos != self.end_pos;
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
                    state.outputs.push((output, OutputState::default()));
                    // todo: file reuse when constructing new state
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
