use image::imageops::FilterType;
use image::RgbaImage;
use pithos::commands::{RenderCommand, RenderMode, ScrollCommand, ThreadCommand};
use crate::pandora::Pandora;
use crate::wl_session::{initialize_wayland_handles, OutputMode, WaylandGlobals, WaylandState};

use std::io::Write;
use std::sync::mpsc::{Sender, Receiver};
use std::sync::Arc;
use std::time::Duration;
use std::{fs::File, os::fd::OwnedFd};

use wayrs_client::{Connection, EventCtx, IoMode};
use wayrs_client::protocol::wl_shm::Format;
use wayrs_client::protocol::WlSurface;
use wayrs_client::protocol::WlCallback;
use wayrs_protocols::viewporter::WpViewport;

// note: output resize/mode-setting changes are currently not handled here
// the "best" solution will probably involve a new command from the compositor agent
// that will be sent upon an output mode/resolution change
// currently, when my desktop sleeps for a while, it looks like my main monitor 'disconnects'
// and then reconnects - and can't be recovered without restarting the daemon presently.
// definitely work to be done ! 

// also i really need to do a pass and make sure to properly use references on.... a lot of these functions, i think.....
// definitely just relied on derive(copy, clone) l0l

pub struct RenderThread {
    name: String,
    receiver: Receiver<ThreadCommand>,
    _sender: Sender<String>, 
    pandora: Arc<Pandora>,
    conn: Connection<WaylandState>,
    globals: Option<WaylandGlobals>,
    // state below, ough
    render_state: Option<RenderState>,
    raw_img: Option<RgbaImage>,
}

#[derive(Copy, Clone)]
pub struct ScrollState {
    start_pos: u32,
    current_pos: u32,
    end_pos: u32,
    step: u32,
    remaining_duration: Duration,
    _num_frames: u32,
}

pub struct RenderState {
    mode: RenderMode,
    _img_path: String,
    _buf_file: File,
    scrolling: Option<ScrollState>,
    crop_width: u32,
    crop_height: u32,
}

impl RenderThread {
    pub fn new(output: String, recv: Receiver<ThreadCommand>, send: Sender<String>, pandora: Arc<Pandora>, conn: Connection<WaylandState>) -> RenderThread {
        return RenderThread {
            name: output,
            receiver: recv,
            _sender: send,
            pandora: pandora,
            conn: conn,
            globals: None,
            render_state: None,
            raw_img: None,
        }
    }

    pub fn start(&mut self) {
        let cmd = self.receiver.recv().expect("thread exploded while waiting on first command recv");
        match cmd {
            ThreadCommand::Render(c) => {
                self.render(c);
            }
            ThreadCommand::Stop(_) => {
                return; // goodbye!
            }
            ThreadCommand::Scroll(_) => {
                panic!("invalid initial command received (should be Render");
            }
        }
        self.draw_loop();
        println!("> {}: goodbye!", self.name);
        // .destroy() every object?
    }

    // i think all the as i32/u32's sprinkled around are going to cause problems
    // one day when someone uses some really fuckin' big images. whatever.
    fn render(&mut self, cmd: RenderCommand) {
        // todo: generally rewrite the buffer management >.<
        // will eventually want to / need to support more pixel formats (at least for HDR)....
        // thankfully I have an hdr monitor :) ... but for now.... rgba8. that's fine.
        if self.globals.is_none() {
            let globals = initialize_wayland_handles(&mut self.conn, cmd.output.clone());
            self.globals = Some(globals);
        }
        // todo: attach file/buffer to self?
        // need to do more work here for when we're switching images or whatever. cus uh. don't think that works rn

        let globals = self.globals.take().unwrap();

        /* No longer scaling image with imageops - instead letting wp_viewport handle for us :)
        // however, WP_viewport does Not like it if the image is too small.... might try this as a convenient upscale if needed?
        let scaled_img = scale_img_for_mode(
            &self.pandora.get_image(cmd.image.clone()).unwrap(),
            cmd.mode, globals.output_info);
        */
        
        let img = self.pandora.get_image(cmd.image.clone()).unwrap();
        println!("> {}: loaded image w/ dims {} x {}", self.name, img.width(), img.height());

        let bytes_per_row: i32 = img.width() as i32 * 4;
        let total_bytes: i32 = bytes_per_row * img.height() as i32;

        let file = tempfile::tempfile().expect("creating tempfile for shared mem failed");
        img_into_buffer(&img, &file);

        let pool = globals.shm.create_pool(&mut self.conn, OwnedFd::from(file.try_clone().unwrap()), total_bytes);
        let buf = pool.create_buffer(&mut self.conn, 0, img.width() as i32, img.height() as i32, bytes_per_row, Format::Argb8888 );
        globals.surface.attach(&mut self.conn, Some(buf), 0, 0); //hardcoded 0s l0l
        
        let (crop_width, crop_height) = calculate_crop(cmd.mode, globals.output_info, img.clone());

        println!("> {}: cropping surface view to {crop_width} x {crop_height}", self.name);

        let scroll_state = match cmd.mode {
            RenderMode::Static => {
                globals.viewport.set_destination(&mut self.conn,
                    globals.output_info.width, globals.output_info.height,
                );

                // center image
                let width_offset = (img.width() - crop_width) / 2;
                let height_offset = (img.height() - crop_height) / 2;

                println!("static mode: offset[{} x {}] geom[{} x {}]", width_offset, height_offset, crop_width, crop_height);

                globals.viewport.set_source(&mut self.conn,
                    width_offset.into(), height_offset.into(),
                    crop_width.into(), crop_height.into(),
                );
                None
            }
            RenderMode::ScrollingVertical(offset) => {
                Some(ScrollState {
                    start_pos: offset,
                    current_pos: offset,
                    end_pos: offset,
                    step: 0,
                    remaining_duration: Duration::from_secs(0),
                    _num_frames: 0,
                })
            },
            RenderMode::ScrollingLateral(offset) => {
                Some(ScrollState {
                    start_pos: offset,
                    current_pos: offset,
                    end_pos: offset,
                    step: 0,
                    remaining_duration: Duration::from_secs(0),
                    _num_frames: 0,
                })
            },
        };
        self.raw_img = Some(img);
        self.render_state = Some(RenderState {
            mode: cmd.mode,
            _img_path: cmd.image.clone(),
            _buf_file: file,
            scrolling: scroll_state,
            crop_width: crop_width,
            crop_height: crop_height,
        });

        if scroll_state.is_some() {
            self.globals = Some(globals);
            self.scroll_surface_to(scroll_state.unwrap().current_pos);
        } 

        globals.surface.commit(&mut self.conn);
        self.conn.blocking_roundtrip().unwrap();
    }

    fn draw_loop(&mut self) {
        loop {
            self.conn.flush(IoMode::Blocking).unwrap();
            let received_events = self.conn.recv_events(IoMode::NonBlocking);
            // set up dispatch state. animation_state is the only mut, rest are just refs we need.
            let mut dispatch_state = WaylandState::default();
            dispatch_state.render_state = self.render_state.take(); // must be put back!!
            dispatch_state.viewport = Some(self.globals.as_ref().unwrap().viewport);
            dispatch_state.surface = Some(self.globals.as_ref().unwrap().surface);
            dispatch_state.output_info = Some(self.globals.as_ref().unwrap().output_info);

            self.conn.dispatch_events(&mut dispatch_state);
            // make sure to update state after callbacks
            self.render_state = dispatch_state.render_state;

            if self.handle_inbound_commands() {
                break;
            }
            if received_events.is_err() {
                // no events on last poll, should sleep if not animating - need to figure out a better way to
                // lazily block on both the receiver and wayland connection events
            }
        }
    }

    // returns true if it's time to exit e.g. received stop
    fn handle_inbound_commands(&mut self) -> bool {
        loop {
            match self.receiver.try_recv() {
                Ok(cmd) => return self.handle_cmd(cmd),
                Err(_) => break,
            }
        }
        return false;
    }

    fn handle_cmd(&mut self, cmd: ThreadCommand) -> bool {
        match cmd {
            ThreadCommand::Render(c) => {
                self.render(c);
                return false;
            }
            ThreadCommand::Stop(_) => {
                return true; // goodbye!
            }
            ThreadCommand::Scroll(c) => {
                self.scroll(c);
                return false;
            }
        }
    }

    fn scroll_surface_to(&mut self, pos: u32) {
        let globals = self.globals.as_mut().unwrap();
        let mut state = self.render_state.take().unwrap();
        do_scroll_step(&mut self.conn, &mut state,
            globals.viewport, globals.output_info, globals.surface, pos);
        self.render_state = Some(state);
    }

    fn scroll(&mut self, cmd: ScrollCommand) {
        let mut render_state = self.render_state.take().unwrap();
        let mut scroll_state = render_state.scrolling.take().unwrap();
        // validate command/position before we commit to scrolling
        let valid = match render_state.mode {
            RenderMode::Static => false, // nothing to do here!
            RenderMode::ScrollingVertical(_) => {
                let end_bound = render_state.crop_height + cmd.position;
                end_bound <= self.raw_img.as_ref().unwrap().height()
            },
            RenderMode::ScrollingLateral(_) => {
                let end_bound = render_state.crop_width + cmd.position;
                end_bound <= self.raw_img.as_ref().unwrap().width()
            }
        };
        if !valid {
            println!("would scroll past end and explode");
            render_state.scrolling = Some(scroll_state);
            self.render_state = Some(render_state);
            return;
        }

        scroll_state.start_pos = scroll_state.current_pos; // current pos should always be updated in scroll_to
        scroll_state.end_pos = cmd.position;
        scroll_state.remaining_duration = Duration::from_millis(1_500);
        // should maybe figure out "number of estimated frames based on refresh rate and duration"
        // and then step-per-frame based off end pos / start pos / etc
        // need to also maybe interp the step along a curve...... complicated!
        // will probably just need to go look at niri to figure out how the scroll anims are interp'd
        // since we want to mimic that for now, I guess
        // (might cause motion sickness otherwise, idk)
        scroll_state.step = 1; // TODO FIGURE OUT :')
        render_state.scrolling = Some(scroll_state);
        self.render_state = Some(render_state);

        self.start_scroll_anim();
    }

    fn start_scroll_anim(&mut self) {
        let mut render_state = self.render_state.take().unwrap();
        let globals = self.globals.as_ref().unwrap();

        println!("> {}: animation starting. start_pos: {}, end_pos: {}",
            self.name,
            render_state.scrolling.unwrap().start_pos,
            render_state.scrolling.unwrap().end_pos);
    
        globals.surface.frame_with_cb(&mut self.conn, frame_callback);
        let next_pos = calc_next_pos(&render_state);
        do_scroll_step(&mut self.conn, &mut render_state,
            globals.viewport,
            globals.output_info,
            globals.surface,
            next_pos,
        );
        self.render_state = Some(render_state);
    }
}

fn calc_next_pos(render_state: &RenderState) -> u32 {
    let scroll_state = render_state.scrolling.as_ref().unwrap();
    // need to handle overshoot when step is not 1 lol
    // also TODO: figure out how to interp from (start_pos, end_pos) based on duration?
    let mut maybe_pos = scroll_state.current_pos;
    if scroll_state.end_pos > scroll_state.current_pos {
        maybe_pos = scroll_state.current_pos + scroll_state.step;
        if maybe_pos > scroll_state.end_pos {
            maybe_pos = scroll_state.end_pos;
        }
    } else if scroll_state.end_pos < scroll_state.current_pos {
        maybe_pos = scroll_state.current_pos - scroll_state.step;
        if maybe_pos < scroll_state.end_pos {
            maybe_pos = scroll_state.end_pos;
        }
    }
    return maybe_pos;
}

fn do_scroll_step(conn: &mut Connection<WaylandState>,
    render_state: &mut RenderState,
    viewport: WpViewport,
    output_info: OutputMode,
    surface: WlSurface,
    next_pos: u32,
) {
    //println!("scrolling to {next_pos}");
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
    //globals.surface.damage(&mut self.conn, 0, 0, 3440, 1440); // <- not needed?? cool.
    surface.commit(conn);
    conn.blocking_roundtrip().unwrap();
}

fn frame_callback(ctx: EventCtx<WaylandState, WlCallback>) {
    let wl_state = ctx.state;
    
    let mut render_state = wl_state.render_state.take().unwrap();
    let next_pos = calc_next_pos(&render_state);
    if next_pos != render_state.scrolling.unwrap().current_pos {
        wl_state.surface.unwrap().frame_with_cb(ctx.conn, frame_callback);
        do_scroll_step(ctx.conn, &mut render_state,
            wl_state.viewport.unwrap(), wl_state.output_info.unwrap(), wl_state.surface.unwrap(), next_pos
        );
    }

    wl_state.render_state = Some(render_state);
}

// todo: genericize and move into a util file
// (agent will probably want to do these calculations later)
fn calculate_crop(mode: RenderMode, output_info: OutputMode, img: RgbaImage) -> ( u32,  u32) {
    // scale factor of image to output
    // image bigger than output => we scale up, vice versa we scale down
    let width_ratio = img.width() as f64 / output_info.width as f64;
    let height_ratio = img.height() as f64 / output_info.height as f64;

    let scale_factor = match mode {
        // min?
        RenderMode::Static => f64::min(width_ratio, height_ratio),
        RenderMode::ScrollingVertical(_) => width_ratio,
        RenderMode::ScrollingLateral(_) => height_ratio,
    };

    match mode {
        RenderMode::Static => {
            ((output_info.width as f64 * scale_factor).round() as  u32, (output_info.height as f64 * scale_factor).round() as  u32)
        },
        RenderMode::ScrollingVertical(_) => {
            // crop vertically, leaving full width
            (img.width() as  u32, (output_info.height as f64 * scale_factor).round() as  u32)
        },
        RenderMode::ScrollingLateral(_) => {
            // crop horizontally, leaving full height
            ((output_info.width as f64 * scale_factor).round() as  u32, img.height() as  u32)

        },
    }
}

fn _get_scaled_dimensions(mode: RenderMode, output_width: i32, output_height: i32, img_width: u32, img_height: u32) -> (i32, i32) {
    let width_ratio = output_width as f64 / img_width as f64;
    let height_ratio = output_height as f64 / img_height as f64;
    let scale_factor = match mode {
        RenderMode::Static => f64::max(width_ratio, height_ratio),
        RenderMode::ScrollingLateral(_) => height_ratio,
        RenderMode::ScrollingVertical(_) => width_ratio,
    };

    let new_width: i32 = (img_width as f64 * scale_factor).round() as i32;
    let new_height: i32 = (img_height as f64 * scale_factor).round() as i32;

    return (new_width, new_height);
}

fn _scale_img_for_mode(img: &RgbaImage, mode: RenderMode, output_info: OutputMode) -> RgbaImage {
    /*
        thinking for a moment: let's say we have a 1920x1080 output, and a 2560x1440 img
        we want to downscale the image, so output/img gives us 0.75, 0.75 => we scale the img by that
        now, if the image is, say, 3000x1440.... we get (0.64, 0.75) => we pick the bigger of the two?
     */
    let (new_width, new_height) = _get_scaled_dimensions(mode,
         output_info.width, output_info.height,
         img.width(), img.height());

    return image::imageops::resize(
        img,
        new_width as u32,
        new_height as u32,
        FilterType::Lanczos3, // TODO: expose this in config/cmd
    );
}

fn img_into_buffer(img: &RgbaImage, f: &File) {
    let mut buf = std::io::BufWriter::new(f);
    for pixel in img.pixels() {
        let (r, g, b, a) = (pixel.0[0], pixel.0[1],pixel.0[2],pixel.0[3]);
        buf.write_all(&[b as u8, g as u8, r as u8, a as u8]).unwrap();
    }
}