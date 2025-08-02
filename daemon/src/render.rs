use image::imageops::FilterType;
use image::RgbaImage;
use pithos::commands::{RenderCommand, RenderMode, ScrollCommand, ThreadCommand};
use crate::pandora::Pandora;
use crate::wl_session::{initialize_wayland_handles, Mode, WaylandGlobals, WaylandState};

use std::io::Write;
use std::sync::mpsc::{Sender, Receiver};
use std::sync::Arc;
use std::thread::sleep;
use std::time;
use std::{fs::File, os::fd::OwnedFd};

use wayrs_client::{Connection, EventCtx, IoMode};
use wayrs_client::protocol::wl_shm::Format;
use wayrs_client::protocol::WlSurface;

// note: output resize/mode-setting changes are currently not handled here
// the "best" solution will probably involve a new command from the compositor agent
// that will be sent upon an output mode/resolution change
// currently, when my desktop sleeps for a while, it looks like my main monitor 'disconnects'
// and then reconnects - and can't be recovered without restarting the daemon presently.
// definitely work to be done ! 

pub struct RenderThread {
    name: String,
    receiver: Receiver<ThreadCommand>,
    _sender: Sender<String>, 
    pandora: Arc<Pandora>,
    conn: Connection<WaylandState>,
    globals: Option<WaylandGlobals>,
    // state below, ough
    //state: Option<WaylandState>,
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

        let globals = self.globals.as_mut().unwrap();

        let scaled_img = scale_img_for_mode(
            &self.pandora.get_image(cmd.image).unwrap(),
            cmd.mode, globals.output_info);

        println!("> {}: scaled image to {} x {}", self.name, scaled_img.width(), scaled_img.height());

        let bytes_per_row: i32 = scaled_img.width() as i32 * 4;
        let total_bytes: i32 = bytes_per_row * scaled_img.height() as i32;

        // note: need to preserve some of this so that subsequent Render commands can properly swap things out

        let file = tempfile::tempfile().expect("creating tempfile for shared mem failed");

        img_into_buffer(&scaled_img, &file);

        let pool = globals.shm.create_pool(&mut self.conn, OwnedFd::from(file), total_bytes);
        let buf = pool.create_buffer(&mut self.conn, 0, scaled_img.width() as i32, scaled_img.height() as i32, bytes_per_row, Format::Argb8888 );
        globals.surface.attach(&mut self.conn, Some(buf), 0, 0); //hardcoded 0s l0l
        globals.surface.commit(&mut self.conn);
        self.conn.blocking_roundtrip().unwrap();
        match cmd.mode {
            RenderMode::Static => (),
            RenderMode::ScrollingVertical(offset) => {
                globals.surface.offset(&mut self.conn, 0, offset.position)
            },
            RenderMode::ScrollingLateral(offset) => {
                globals.surface.offset(&mut self.conn, offset.position, 0)
            },
        };
        globals.surface.commit(&mut self.conn);
        self.conn.blocking_roundtrip().unwrap();
        return;
        unreachable!();
        loop {
            // for the life of me, I cannot figure out why adjusting the offset
            // doesn't actually move the buffer/surface around. i'm doing *something* wrong, most definitely....            
            println!("0 25");
            globals.surface.offset(&mut self.conn, 0, 25);
            globals.surface.damage_buffer(&mut self.conn, 0, 0, scaled_img.width() as i32, scaled_img.height() as i32);
            globals.surface.commit(&mut self.conn);
            self.conn.blocking_roundtrip().unwrap();
            sleep(time::Duration::from_secs(2));

            println!("0 -25");
            globals.surface.offset(&mut self.conn, 0, -25);
            globals.surface.damage_buffer(&mut self.conn, 0, 0, scaled_img.width() as i32, scaled_img.height() as i32);
            globals.surface.commit(&mut self.conn);
            self.conn.blocking_roundtrip().unwrap();
            sleep(time::Duration::from_secs(2));
        }
    }

    fn _surface_frame_tick_callback(_ctx: EventCtx<WaylandState, WlSurface>) {
        // register this when we have updating to be doing / are Animating
        // need to request another frame callback if we are still animating/scrolling!
        todo!();
    }

    fn draw_loop(&mut self) {
        loop {
            self.conn.flush(IoMode::Blocking).unwrap();
            let received_events = self.conn.recv_events(IoMode::NonBlocking);
            // todo clone state into dispatch
            self.conn.dispatch_events(&mut WaylandState::default());
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

    fn scroll(&self, _cmd: ScrollCommand) {
        // just scroll the canvas 4head
        // UUUUUUGH
        todo!();
    }
}

fn scale_img_for_mode(img: &RgbaImage, mode: RenderMode, output_info: Mode) -> RgbaImage {
    /*
        thinking for a moment: let's say we have a 1920x1080 output, and a 2560x1440 img
        we want to downscale the image, so output/img gives us 0.75, 0.75 => we scale the img by that
        now, if the image is, say, 3000x1440.... we get (0.64, 0.75) => we pick the bigger of the two?
     */
    let width_ratio = output_info.width as f64 / img.width() as f64;
    let height_ratio = output_info.height as f64 / img.height() as f64;
    let scale_factor = match mode {
        RenderMode::Static => f64::max(width_ratio, height_ratio),
        RenderMode::ScrollingLateral(_) => height_ratio,
        RenderMode::ScrollingVertical(_) => width_ratio,
    };

    let new_width: u32 = (img.width() as f64 * scale_factor).round() as u32;
    let new_height: u32 = (img.height() as f64 * scale_factor).round() as u32;

    return image::imageops::resize(
        img,
        new_width,
        new_height,
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