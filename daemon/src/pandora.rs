use crate::ipc::IpcHandler;
use crate::render::{RenderThread};
use image::imageops::FilterType;
use pithos::misc_helpers::get_new_image_dimensions;
use pithos::wayland::render_helpers::RenderThreadWaylandState;
use pithos::commands::{CommandType, ConfigReloadCommand, DaemonCommand, InfoCommand, LoadImageCommand, ThreadCommand};
use pithos::error::{CommandError, DaemonError};
use pithos::sockets::write_response_to_client_socket;

use std::collections::HashMap;
use std::fs::File;
use std::os::unix::net::{UnixStream};
use std::sync::{Arc, Mutex, RwLock};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread::{self, JoinHandle};

use image::{RgbaImage, ImageReader};
use wayrs_client::Connection;

// daemon utility struct(s)
pub struct ThreadHandle {
    sender: Sender<ThreadCommand>,
    _receiver: Arc<Mutex<Receiver<String>>>,
    thread: JoinHandle<()>,
}

impl ThreadHandle {
    fn new(output: String, pandora: Arc<Pandora>) -> ThreadHandle {
        let (host_sender, thread_receiver) = channel::<ThreadCommand>();
        let (thread_sender, host_receiver) = channel::<String>();
        let conn = Connection::<RenderThreadWaylandState>::connect().unwrap();

        let thread = thread::spawn(move || {
            RenderThread::new(
                output,
                thread_receiver,
                thread_sender,
                pandora,
                conn,
            ).start();
        });

        return ThreadHandle {
            sender: host_sender,
            _receiver: Arc::new(Mutex::new(host_receiver)),
            thread: thread,
        };
    }
}

// daemon
#[derive(Clone)]
pub struct Pandora {
    pub ipc: Option<Arc<IpcHandler>>,
    // key: output name
    threads: Arc<RwLock<HashMap<String, ThreadHandle>>>,
    // key: file path
    // could maybe just get rid of this table entirely tbh? it's starting to feel like uncessary overhead at this point....
    // but i imagine it's maybe useful if you want to script switching between images frequently..... idk.
    images: Arc<RwLock<HashMap<String, RgbaImage>>>,
}

impl Pandora {
    pub fn new() -> Arc<Pandora> {
        return Arc::new(Pandora {
            ipc: None,
            threads: Arc::new(RwLock::new(HashMap::<String, ThreadHandle>::new())),
            images: Arc::new(RwLock::new(HashMap::<String, RgbaImage>::new())),
        });
    }

    pub fn bind_ipc(&mut self, ipc: Arc<IpcHandler>) {
        self.ipc = Some(ipc);
    }

    fn handle_daemon_command(&self, dc: &DaemonCommand) -> Result<&str, DaemonError> {
        return match dc {
            DaemonCommand::Info(c) => self.info(c),
            DaemonCommand::LoadImage(c) => self.load_image(c),
            DaemonCommand::ConfigReload(c) => self.reload_config(c),
            DaemonCommand::Stop => std::process::exit(0),
        };
    }

    fn handle_thread_command(&self, tc: &ThreadCommand) -> Result<&str, DaemonError> {
        let output: String;
        let mut can_spawn = false;
        let mut join_after = false;
        let mut image_to_preload: Option<String> = None;
        match tc.clone() {
            ThreadCommand::Render(c) => {
                output = c.output;
                can_spawn = true;
                image_to_preload = Some(c.image);
            }
            ThreadCommand::Stop(c) => {
                output = c.output;
                join_after = true;
            }
            ThreadCommand::Scroll(c) => {
                output = c.output;
            }
        };
        if image_to_preload.is_some() {
            self.handle_daemon_command(&DaemonCommand::LoadImage(LoadImageCommand { image:image_to_preload.unwrap() }))?;
        }
        let ret = self.dispatch_thread_command(output.clone(), &tc, can_spawn);
        if join_after && ret.is_ok() { // if a stop command error'd in dispatch, it either crashed or didn't exist; no need to clean up
            return self.cleanup_thread(&output);
        } else {
            return ret;
        }
    }

    fn dispatch_thread_command(&self, output: String, c: &ThreadCommand, spawn: bool) -> Result<&str, DaemonError> {
        // read lock:
        // check if thread exists, dispatch and return if it does.
        {
            let read_threads = self.threads.read()?;
            match read_threads.get(&output) {
                Some(thread) => {
                    if thread.sender.send(c.clone()).is_ok() {
                        return Ok("dispatched command to thread");
                    }
                    drop(read_threads); // kinda gross
                    match self.cleanup_thread(&output) {
                        Ok(_) => {
                            return Err(CommandError::new("could not send command to thread; cleaned it up"))
                        },
                        Err(e) => {
                            return Err(CommandError::new(format!("could not send command to thread; failed to clean it up: [{e:?}]").as_str()))
                        }
                    }
                },
                None => {} // doesn't exist; release lock and then re-enter outside to try to spawn a thread
            }
        }
        if spawn {
            return self.spawn_thread(output, c);
        } else {
            return Err(CommandError::new("invalid thread command: no existing render thread for this output name (Render command can spawn new threads)"));
        }
    }

    fn spawn_thread(&self, output: String, c: &ThreadCommand) -> Result<&str, DaemonError> {
        let thread = ThreadHandle::new(output.clone(), Arc::new(self.clone()));
        thread.sender.send(c.clone()).expect("could not send initial command to thread after spawning");
        {
            let mut write_threads = self.threads.write()?;
            write_threads.insert(output, thread);
        }
        return Ok("spawned thread and dispatched render command");
    }

    fn info(&self, _cmd: &InfoCommand) -> Result<&str, DaemonError> {
        let mut answer = String::new();

        {
            let images = self.images.read()?;
            answer += format!("items in images table: {}\n", images.len()).as_str();
        }
        {
            let threads = self.threads.read()?;
            answer += format!("items in threads table: {}\n", threads.len()).as_str();
        }

        println!("{answer}");

        return Ok("daemon dumped debug info to logs");
    }

    fn load_image(&self, cmd: &LoadImageCommand) -> Result<&str, DaemonError>  {
        let img= ImageReader::open(cmd.image.clone())?.decode()?;
        {
            let images_lock = self.images.write();
            match images_lock {
                Ok(mut images_table) => {
                    if images_table.contains_key(&cmd.image.clone()) {
                        return Ok("file already loaded/present in image table");
                    }
                    images_table.insert(cmd.image.clone(), img.into_rgba8());
                    return Ok("image loaded successfully");
                }
                Err(e) => Err(DaemonError::from(e)),
            }
        }
    }

    // if scale_to is provided, uses the provided width/height dimensions of the output to scale image appropriately
    // if only one dimension is provided, scales to that one and keeps aspect ratio.
    pub fn read_img_to_file(&self, img: &String, f: &File, scale_to: Option<(Option<u32>, Option<u32>)>) -> Result<(u32, u32), DaemonError> {
        let mut image = None;
        {
            let images = self.images.read()?;
            if images.contains_key(img) {
                image = Some(images.get(img).unwrap());
            }
            if image.is_none() {
                return Err(CommandError::new("invalid image (not loaded)"));
            }

            let image = image.unwrap();
            match scale_to {
                Some((maybe_width, maybe_height)) => {
                    let (new_width, new_height) = get_new_image_dimensions(image.width(), image.height(), maybe_width, maybe_height);
                    pithos::misc_helpers::img_into_buffer(
                        &image::imageops::resize(
                        image,
                        new_width as u32,
                        new_height as u32,
                        FilterType::Lanczos3,
                        ),
                        &f
                    );
                    return Ok((new_width, new_height));
                },
                None => {
                    pithos::misc_helpers::img_into_buffer(image, &f);
                    return Ok((image.width(), image.height()));
                }
            };
        }
    }

    pub fn drop_img_from_cache(&self, img: &String) -> Result<(), DaemonError> {
        {
            let mut images = self.images.write()?;
            images.remove(img);
            images.shrink_to_fit();
        }
        Ok(())
    }

    fn reload_config(&self, _cmd: &ConfigReloadCommand) -> Result<&str, DaemonError> {
        todo!();
    }

    fn cleanup_thread(&self, output: &String) -> Result<&str, DaemonError> {
        {
            let mut write_threads = self.threads.write().expect("could not acquire read lock for dispatching command");
            return match write_threads.remove(output) {
                Some(thread) => {
                    write_threads.shrink_to_fit();
                    if thread.thread.is_finished() {
                        let _ = thread.thread.join();
                        Ok("thread stopped successfully")
                    } else {
                        Err(CommandError::new("thread not stopped (wedged?)"))
                    }
                },
                None => Err(CommandError::new("named thread already stopped or otherwise didn't exist")),
            }
        }
    }

    pub fn start(&self) {
        self.ipc.as_ref().expect("ipc handler should be bound before start").start_listen();
    }

    pub fn process_ipc(&self, socket: &UnixStream) -> () {
        let cmd = pithos::sockets::read_command_from_client_socket(&socket.try_clone().expect("couldn't clone socket"));
        let response = match self.handle_cmd(&cmd) {
            Ok(s) => s.to_string(),
            Err(e) => {
                match e {
                    DaemonError::IoError(err) => {
                        format!("i/o error: {err}")
                    },
                    DaemonError::ImageError(err) => {
                        format!("i/o error: {err}")
                    },
                    DaemonError::PoisonError => {
                        format!("lock poison error (uhhh)")
                    },
                    DaemonError::CommandError(err) => err.response,
                }
            }
        };
        write_response_to_client_socket(response.as_str(), socket).expect("failed to write response to inbound ipc");
    }

    fn handle_cmd(&self, cmd: &CommandType) -> Result<&str, DaemonError> {
        return match cmd {
            CommandType::Dc(dc) => self.handle_daemon_command(&dc),
            // CommandType::Ac(ac) => {}
            CommandType::Tc(tc) => self.handle_thread_command(&tc),
        };
    }
}