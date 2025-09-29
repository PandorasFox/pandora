use crate::daemon::Daemon;
use crate::pithos::anims::spring::Spring;
use crate::pithos::commands::RenderMode;
use crate::pithos::config::DaemonConfig;
use crate::pithos::misc::get_viewport_dimensions;

use std::collections::HashMap;
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

fn verbose(pandora: Arc<dyn Daemon>, msg: &str) {
    pandora.verbose("render", msg.to_string())
}

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

    pub fn wl_output(&self) -> WlOutput {
        self.wl_output
    }
}

pub struct RenderThreadState {
    pub outputs: Vec<(Output, OutputState)>,
    pub detached_outputs: Vec<OutputState>,
    pub globals: WaylandGlobals,
    pub pandora: Weak<dyn Daemon>,
    pub reseat_needed: bool,
}

impl RenderThreadState {
    pub fn get_outputs(&mut self, conn: &mut Connection<RenderThreadState>) {
        let num_outputs = conn
            .globals()
            .iter()
            .filter(|g| g.is::<WlOutput>())
            .cloned()
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
            match &output_state.render_state {
                OutputRenderStateVariety::Wallpaper(wallpaper_render_state) => {
                        if wallpaper_render_state.width.scroll_state.is_some() {
                            return true;
                        }
                        if wallpaper_render_state.height.scroll_state.is_some() {
                            return true;
                        }
                } //OutputRenderStateVariety::Lockscreen => (),
                OutputRenderStateVariety::None => {}
            }
        }
        false
    }

    pub fn try_reseat_outputs(&mut self, conn: &mut Connection<RenderThreadState>) {
        // check for previous state in detached_outputs
        // match against existing outputs
        // update their output state with the reusable unplugged state fields (image path, mode, ?position?, scroll state)

        // Process each output individually by temporarily removing it to avoid borrowing conflicts
        let mut output_indices_to_process = Vec::new();

        // Collect outputs that need reseating
        for (i, (_, output_state)) in self.outputs.iter().enumerate() {
            if self.detached_outputs.iter().any(|prior| prior.name == output_state.name) {
                output_indices_to_process.push(i);
            }
        }

        // Process each output by temporarily removing it from the state
        for &output_idx in &output_indices_to_process {
            if let Some(detached_idx) = self.detached_outputs.iter().position(|prior| {
                if let Some((_, os)) = self.outputs.get(output_idx) {
                    prior.name == os.name
                } else {
                    false
                }
            }) {
                let mut prior_state = self.detached_outputs.swap_remove(detached_idx);
                let (mut output, mut output_state) = self.outputs.swap_remove(output_idx);

                output_state.reseat(
                    conn,
                    &mut output,
                    &mut prior_state,
                    self,
                );

                // Put the output back
                self.outputs.insert(output_idx, (output, output_state));
            }
        }
    }

}

pub struct OutputState {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub done: bool,
    pub transform: wl_output::Transform,
    pub render_state: OutputRenderStateVariety,
    pub needs_reinit: bool,
}

impl Default for OutputState {
    fn default() -> Self {
        Self {
            name: String::default(),
            width: 0,
            height: 0,
            done: false,
            transform: wl_output::Transform::Normal,
            render_state: OutputRenderStateVariety::None,
            needs_reinit: false,
        }
    }
}

impl OutputState {
    pub fn reseat(
        &mut self,
        conn: &mut Connection<RenderThreadState>,
        new_output: &mut Output,
        prior_state: &mut OutputState,
        render_state: &mut RenderThreadState,
    ) {
        match &mut prior_state.render_state {
            OutputRenderStateVariety::Wallpaper(wp_state) => {
                let new_state = OutputRenderStateVariety::Wallpaper(WallpaperRenderState::from_prior(
                    conn, render_state, wp_state, new_output, self,
                ));
                self.render_state = new_state;
            }
            OutputRenderStateVariety::None => {}
        }
    }

    pub fn yeet(&mut self, conn: &mut Connection<RenderThreadState>) {
        match &self.render_state {
            OutputRenderStateVariety::Wallpaper(wp_state) => wp_state.yeet(conn),
            OutputRenderStateVariety::None => {}
        }
    }

    pub fn reinit(&mut self) {
        todo!()
    }
}

pub enum OutputRenderStateVariety {
    None,
    Wallpaper(WallpaperRenderState),
    //Lockscreen,
}

#[derive(Copy, Clone)]
pub struct ScrollDimension {
    pub output_dim: i32,                   // output width or height
    pub image_dim: i32,                    // image width or height
    pub viewport_dim: f64,                 // calculated viewport width or height
    pub scroll_percent: f64,               // 0.0 => 100.0
    pub scroll_state: Option<ScrollState>, // should be None'd once scroll is finished
}

impl ScrollDimension {
    pub fn new(output_dim: i32, image_dim: i32, viewport_dim: i32, scroll_percent: f64) -> Self {
        Self {
            output_dim,
            image_dim,
            viewport_dim: viewport_dim as f64,
            scroll_percent,
            scroll_state: None,
        }
    }

    pub fn compute_viewport_range(&self) -> (f64, f64) {
        crate::pithos::misc::compute_viewport_range(
            self.image_dim,
            self.viewport_dim as i32,
            self.scroll_percent,
        )
    }

    pub fn compute_viewport_range_with_scroll(&self, scroll_percent: f64) -> (f64, f64) {
        crate::pithos::misc::compute_viewport_range(
            self.image_dim,
            self.viewport_dim as i32,
            scroll_percent,
        )
    }

    pub fn compute_geometry(&self) -> (f64, f64) {
        let (viewport_start, viewport_end) = self.compute_viewport_range();
        (viewport_start, viewport_end - viewport_start)
    }

    pub fn compute_geometry_with_scroll(&self, scroll_percent: f64) -> (f64, f64) {
        let (viewport_start, viewport_end) =
            self.compute_viewport_range_with_scroll(scroll_percent);
        (viewport_start, viewport_end - viewport_start)
    }

    pub fn calc_current_position(&self) -> f64 {
        if let Some(scroll_state) = self.scroll_state {
            let eclipsed_duration = std::time::Instant::now() - scroll_state.anim_start;
            let seconds = eclipsed_duration.as_secs_f64() / scroll_state.slowdown;
            let scaled_duration = std::time::Duration::from_secs_f64(seconds);
            scroll_state.anim.value_at(scaled_duration).round()
        } else {
            let (viewport_start, _) = self.compute_geometry();
            viewport_start
        }
    }

    pub fn scroll(&mut self, scroll_percent: f64, slowdown: f64) {
        let (start_position, _) = self.compute_geometry_with_scroll(scroll_percent);
        self.update_scroll_animation(start_position, scroll_percent, slowdown);
    }

    pub fn update_scroll_animation(&mut self, target_pos: f64, scroll_percent: f64, slowdown: f64) {
        let current_pos = self.calc_current_position();
        self.scroll_percent = scroll_percent;

        match &mut self.scroll_state {
            Some(state) => {
                state.start_pos = current_pos;
                state.anim.from = current_pos;
                state.end_pos = target_pos;
                state.anim.to = target_pos;
                state.anim_start = std::time::Instant::now();
                state.anim_duration = state.anim.duration();
            }
            None => {
                let spring = crate::pithos::anims::spring::Spring {
                    from: current_pos,
                    to: target_pos,
                    initial_velocity: 0.0,
                    params: crate::pithos::anims::spring::SpringParams::default(),
                };

                self.scroll_state = Some(ScrollState {
                    start_pos: current_pos,
                    current_pos,
                    end_pos: target_pos,
                    anim_start: std::time::Instant::now(),
                    anim_duration: spring.duration(),
                    anim: spring,
                    slowdown,
                });
            }
        }
    }
}

pub struct WallpaperRenderState {
    // un-rust-y: Wayland-y objects can be destroyed & be invalid references for use after output removal => buffer freeing
    // should refactor them to be Option<>s, .take() on destroy?
    pub surface: WlSurface,
    pub viewport: WpViewport,
    pub width: ScrollDimension,
    pub height: ScrollDimension,
    pub image: String,
    pub file: File,
    pub buffer: WlBuffer,
    pub mode: RenderMode,
    pub slowdown: f64, // must be updated whenever config reload percolates to render thread
}

impl WallpaperRenderState {
    pub fn new(
        conn: &mut Connection<RenderThreadState>,
        render_state: &mut RenderThreadState,
        image_path: &str,
        mode: RenderMode,
        wl_output: WlOutput,
        output_state: &OutputState,
        scroll_percents: Option<(f64, f64)>,
        slowdown: f64,
    ) -> Self {
        // create new buffer, load image into it, attach it to surface, apply role through daemon
        let pandora = render_state.pandora.upgrade().unwrap();
        let surface = render_state.globals.compositor.create_surface(conn);
        pandora
            .clone()
            .apply_role_to_surface(conn, &surface, &wl_output, output_state);

        // Dispatch events after applying layer role to ensure configure event is handled
        conn.dispatch_events(render_state);
        // note: during runtime, the above dispatch (on an output reseat after wake, for example)
        // could happen when other displays are having similar events
        // maybe not entirely thread-safe if the outputs vector is mutating but uhhhhhhhhhhhh :) later problem :)
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
            render_state.globals
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
        let viewport = render_state.globals.viewporter.get_viewport(conn, surface);
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

        let width = ScrollDimension::new(
            output_state.width,
            image_width,
            viewport_width,
            scroll_percent_x,
        );
        let height = ScrollDimension::new(
            output_state.height,
            image_height,
            viewport_height,
            scroll_percent_y,
        );

        let (viewport_x_start, viewport_width) = width.compute_geometry();
        let (viewport_y_start, viewport_height) = height.compute_geometry();

        surface.attach(conn, Some(buf), 0, 0);
        surface.damage(conn, 0, 0, width.output_dim, height.output_dim);
        viewport.set_destination(conn, output_state.width, output_state.height);

        viewport.set_source(
            conn,
            viewport_x_start.into(),
            viewport_y_start.into(),
            viewport_width.into(),
            viewport_height.into(),
        );
        surface.commit(conn);
        surface.frame_with_cb(conn, frame_callback);
        conn.blocking_roundtrip().unwrap();

        WallpaperRenderState {
            surface,
            viewport,
            width,
            height,
            image: image_path.to_string(),
            file,
            buffer: buf,
            mode,
            slowdown,
        }
    }

    pub fn yeet(&self, conn: &mut Connection<RenderThreadState>) {
        self.buffer.destroy(conn);
        self.viewport.destroy(conn);
        self.surface.destroy(conn);
        conn.blocking_roundtrip().unwrap();
    }

    pub fn from_prior(
        conn: &mut Connection<RenderThreadState>,
        render_state: &mut RenderThreadState,
        unplugged: &mut WallpaperRenderState,
        output: &mut Output,
        output_state: &OutputState,
    ) -> Self {
        // todo: file reuse from unplugged
        let new_state = WallpaperRenderState::new(
            conn,
            render_state,
            &unplugged.image,
            unplugged.mode,
            output.wl_output,
            output_state,
            Some((
                unplugged.width.scroll_percent,
                unplugged.height.scroll_percent,
            )),
            unplugged.slowdown,
        );
        return new_state;
    }

    pub fn scroll(
        &mut self,
        conn: &mut Connection<RenderThreadState>,
        _scroll_x: f64,
        scroll_y: f64,
    ) {
        // scroll (x|y) track the % (0.0 => 100.0) of the center of the viewport along each dimension
        // e.g. 0.0 scroll_x is left-aligned, 100.0 is right aligned, and 50.0 has the viewport centered
        let is_already_scrolling =
            self.width.scroll_state.is_some() || self.height.scroll_state.is_some();

        match self.mode {
            RenderMode::Static => return,
            RenderMode::ScrollVertical => self.height.scroll(scroll_y, self.slowdown),
        };

        if !is_already_scrolling {
            self.start_scroll_anim(conn);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn update_image(
        &mut self,
        conn: &mut Connection<RenderThreadState>,
        globals: WaylandGlobals,
        weak: Weak<dyn Daemon>,
        new_image_path: &str,
        new_mode: RenderMode,
        output_state: &OutputState,
        scroll_position: Option<(f64, f64)>,
        slowdown: f64,
    ) {
        // Create new file and load new image
        let new_file = tempfile::tempfile().expect("creating tempfile for shared mem failed");
        let pandora = weak.upgrade().unwrap();
        verbose(pandora.clone(), "starting swap of image buffers");

        let (image_width, image_height) = image_to_file(
            pandora.clone(),
            &new_mode,
            &new_file,
            new_image_path,
            output_state.width as u32,
            output_state.height as u32,
        );

        let bytes_per_row = image_width * 4;
        let total_bytes = bytes_per_row * image_height;

        // Create new pool and buffer
        let pool = globals.shm.create_pool(
            conn,
            OwnedFd::from(new_file.try_clone().unwrap()),
            total_bytes,
        );
        let new_buffer = pool.create_buffer(
            conn,
            0,
            image_width,
            image_height,
            bytes_per_row,
            Format::Argb8888,
        );

        // Calculate scroll positions
        let (scroll_percent_x, scroll_percent_y) = match scroll_position {
            Some((x, y)) => (x, y),
            None => (self.width.scroll_percent, self.height.scroll_percent),
        };

        let (viewport_width, viewport_height) = get_viewport_dimensions(
            image_width,
            image_height,
            output_state.width,
            output_state.height,
            new_mode,
        );

        // Create new dimensions
        let new_width_dim = ScrollDimension::new(
            output_state.width,
            image_width,
            viewport_width,
            scroll_percent_x,
        );
        let new_height_dim = ScrollDimension::new(
            output_state.height,
            image_height,
            viewport_height,
            scroll_percent_y,
        );

        // Calculate new viewport geometry
        let (viewport_x_start, viewport_width) = new_width_dim.compute_geometry();
        let (viewport_y_start, viewport_height) = new_height_dim.compute_geometry();

        // Save reference to old buffer for cleanup
        let old_buffer = self.buffer;

        // Atomically swap to new buffer and update state
        verbose(pandora.clone(), "attaching new buffer");
        self.surface.attach(conn, Some(new_buffer), 0, 0);
        self.viewport
            .set_destination(conn, output_state.width, output_state.height);
        self.viewport.set_source(
            conn,
            viewport_x_start.into(),
            viewport_y_start.into(),
            viewport_width.into(),
            viewport_height.into(),
        );

        // Update all state fields
        self.buffer = new_buffer;
        self.file = new_file;
        self.image = new_image_path.to_string();
        self.mode = new_mode;
        self.slowdown = slowdown;
        self.width = new_width_dim;
        self.height = new_height_dim;

        // Mark the entire surface as damaged so compositor knows to redraw
        self.surface
            .damage(conn, 0, 0, output_state.width, output_state.height);

        // Commit the changes
        self.surface.commit(conn);
        conn.blocking_roundtrip().unwrap();

        // Now that the new buffer is committed, destroy the old one
        old_buffer.destroy(conn);
    }

    fn start_scroll_anim(&mut self, conn: &mut Connection<RenderThreadState>) {
        self.surface.frame_with_cb(conn, frame_callback);
        self.do_scroll_tick(conn);
    }

    fn calc_pos_x(&self) -> f64 {
        self.width.calc_current_position()
    }

    fn calc_pos_y(&self) -> f64 {
        self.height.calc_current_position()
    }

    fn do_scroll_tick(&mut self, conn: &mut Connection<RenderThreadState>) {
        if self.width.scroll_state.is_none() && self.height.scroll_state.is_none() {
            return; // probably an opportunistic dispatch for a surface w/o scroll state
        }

        let next_pos_x = self.calc_pos_x();
        let next_pos_y = self.calc_pos_y();
        //println!("> [render] scrolling to {next_pos_x},{next_pos_y}");
        self.scroll_to(conn, next_pos_x, next_pos_y);

        if self.width.scroll_state.is_some_and(|s| s.is_animating())
            || self.height.scroll_state.is_some_and(|s| s.is_animating())
        {
            self.surface.frame_with_cb(conn, frame_callback);
        }
        if let Some(mut state) = self.width.scroll_state {
            if !state.is_animating() {
                self.width.scroll_state = None
            } else {
                state.current_pos = next_pos_x;
            }
        }
        if let Some(mut state) = self.height.scroll_state {
            if !state.is_animating() {
                self.height.scroll_state = None
            } else {
                state.current_pos = next_pos_y;
            }
        }
    }

    fn scroll_to(&mut self, conn: &mut Connection<RenderThreadState>, pos_x: f64, pos_y: f64) {
        self.viewport.set_source(
            conn,
            pos_x.into(),
            pos_y.into(),
            self.width.viewport_dim.into(),
            self.height.viewport_dim.into(),
        );

        self.surface.commit(conn);
        conn.flush(IoMode::Blocking).unwrap();
    }
}

fn frame_callback(ctx: EventCtx<RenderThreadState, WlCallback>) {
    // fucking annoying: apparently no way to tell which surface the frame calback is for?
    // need to figure out a bool for is_waiting_for_next_frame or something
    // where we update that in the draw loop -> dispatch stuff
    // rly goofy ngl........
    for (_, os) in &mut ctx.state.outputs {
        match &mut os.render_state {
            OutputRenderStateVariety::Wallpaper(wallpaper_render_state) => {
                if wallpaper_render_state.width.scroll_state.is_some()
                    || wallpaper_render_state.height.scroll_state.is_some()
                {
                    wallpaper_render_state.do_scroll_tick(ctx.conn);
                }
            } //OutputRenderStateVariety::Lockscreen => (),
            OutputRenderStateVariety::None => {}
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
        (Instant::now() - self.anim_start) < self.anim_duration
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
    initial_positions: &HashMap<String, (f64, f64)>,
) {
    // Collect the data we need first to avoid borrowing conflicts
    let mut output_configs = Vec::new();
    for (output, output_state) in &state.outputs {
        let maybe_positions = initial_positions.get(&output_state.name);
        if let Some(output_config) = config.outputs.iter().find(|oc| oc.name == output_state.name) {
            let mode = output_config.mode.unwrap_or(RenderMode::Static);
            output_configs.push((
                output.wl_output,
                output_state.name.clone(),
                output_config.image.clone(),
                mode,
                maybe_positions.copied(),
                output_state.width,
                output_state.height,
                output_state.done,
            ));
        } else {
            println!("output {} not found in configs; skipping", output_state.name);
        }
    }


    // Now process each output, creating render states one by one
    for (wl_output, output_name, image_path, mode, scroll_percents, width, height, done) in output_configs {
        // Create a temporary OutputState for the constructor
        let temp_output_state = OutputState {
            name: output_name.clone(),
            width,
            height,
            done,
            transform: wl_output::Transform::Normal,
            render_state: OutputRenderStateVariety::None,
        };

        let wallpaper_state = WallpaperRenderState::new(
            conn,
            state,
            &image_path,
            mode,
            wl_output,
            &temp_output_state,
            scroll_percents,
            config.animation.slowdown.max(0.001),
        );

        // Find the actual output state and update it
        if let Some((_, output_state)) = state.outputs.iter_mut().find(|(_, os)| os.name == output_name) {
            output_state.render_state = OutputRenderStateVariety::Wallpaper(wallpaper_state);
        }
    }
}

fn image_to_file(
    pandora: Arc<dyn Daemon>,
    mode: &RenderMode,
    f: &File,
    path: &str,
    width: u32,
    height: u32,
) -> (i32, i32) {
    // note: should wrap this and fall back to static if vert won't work, or infer it ourselves
    let scale_to = match mode {
        RenderMode::Static => (Some(width), Some(height)),
        RenderMode::ScrollVertical => (Some(width), None),
    };

    pandora.clone().load_image(path).unwrap();
    let (img_width, img_height) = pandora.clone().read_img_to_file(path, f, scale_to).unwrap();

    if img_width < width || img_height < height {
        panic!(
            "INVALID CONFIG COMBINATION: image scaled to {img_width} x {img_height}, but output is {width} by {height}.\nSwitch to STATIC mode for this image or try scrolling in the other direction."
        )
    }
    pandora.clone().verbose(
        "wallpaper",
        format!("file loaded and scaled to {img_width} x {img_height}"),
    );
    (img_width as i32, img_height as i32)
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
                    state.reseat_needed = true;
                }
            };
        }
        wl_registry::Event::GlobalRemove(name) => {
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

    match ctx.event {
        wl_output::Event::Name(name) => {
            output_state.name = name.clone().into_string().unwrap();
            output.name = Some(name.into_string().unwrap());
        }
        wl_output::Event::Mode(mode) => {
            output_state.width = mode.width;
            output_state.height = mode.height;
        }
        wl_output::Event::Geometry(geometry) => {
            let prev_transform = output_state.transform;
            let new_transform = geometry.transform;

            // Check if transform changed from/to a 90° or 270° rotation
            let prev_is_rotated = matches!(prev_transform,
                wl_output::Transform::_90 | wl_output::Transform::_270 |
                wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270);
            let new_is_rotated = matches!(new_transform,
                wl_output::Transform::_90 | wl_output::Transform::_270 |
                wl_output::Transform::Flipped90 | wl_output::Transform::Flipped270);

            // Update the transform
            output_state.transform = new_transform;

            // If rotation state changed, swap width/height and flag for reseat
            if prev_is_rotated != new_is_rotated {
                let old_width = output_state.width;
                output_state.width = output_state.height;
                output_state.height = old_width;
                // Flag this output for re-initialization after rotation
                output_state.needs_reinit = true;
            }
        }
        // wl_output::Event::Scale(scale) => output.scale = Some(scale), // maybe track this for lockscreen element scaling?
        wl_output::Event::Done => {
            output_state.done = true;
            output.done = true;
        }
        _ => (),
    }
}

pub fn layer_shell_callback(ctx: EventCtx<RenderThreadState, ZwlrLayerSurfaceV1>) {
    let layer: ZwlrLayerSurfaceV1 = ctx.proxy;
    if let wayrs_protocols::wlr_layer_shell_unstable_v1::zwlr_layer_surface_v1::Event::Configure(
        args,
    ) = ctx.event
    {
        layer.ack_configure(ctx.conn, args.serial);
    }
}
