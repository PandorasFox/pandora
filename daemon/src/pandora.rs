use crate::ipc::IpcHandler;
use crate::render::{RenderThread};
use pithos::commands::{CommandType, ConfigReloadCommand, DaemonCommand, InfoCommand, LoadImageCommand, ThreadCommand};
use pithos::error::{CommandError, DaemonError};
use pithos::sockets::write_response_to_client_socket;

use std::collections::HashMap;
use std::os::unix::net::{UnixStream};
use std::sync::{Arc, RwLock};
use std::sync::mpsc::{channel, Sender};
use std::thread::{self, JoinHandle};

use image::{RgbaImage, ImageReader};

// daemon utility struct(s)
pub struct ThreadHandle {
    sender: Sender<ThreadCommand>,
    thread: JoinHandle<()>,
}

impl ThreadHandle {
    fn new(pandora: Arc<Pandora>) -> ThreadHandle {
        let (sender, receiver) = channel::<ThreadCommand>();
        let thread = thread::spawn(move || {
            RenderThread::new(receiver, pandora).start();
        });
        return ThreadHandle {
            sender: sender,
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

    fn handle_daemon_command(&self, dc: DaemonCommand) -> Result<&str, DaemonError> {
        return match dc {
            DaemonCommand::Info(c) => self.info(c),
            DaemonCommand::LoadImage(c) => self.load_image(c),
            DaemonCommand::ConfigReload(c) => self.reload_config(c),
        };
    }

    fn handle_thread_command(&self, tc: ThreadCommand) -> Result<&str, DaemonError> {
        let output: String;
        let mut can_spawn = false;
        let mut join_after = false;
        match tc.clone() {
            ThreadCommand::Render(c) => {
                output = c.output;
                can_spawn = true;
            }
            ThreadCommand::Stop(c) => {
                output = c.output;
                join_after = true;
            }
            ThreadCommand::Scroll(c) => {
                output = c.output;
            }
        };
        let ret = self.dispatch_thread_command(output.clone(), tc, can_spawn);
        if join_after {
            return self.cleanup_thread(output);
        } else {
            return ret;
        }
    }

    fn dispatch_thread_command(&self, output: String, c: ThreadCommand, spawn: bool) -> Result<&str, DaemonError> {
        // read lock:
        // check if thread exists, dispatch and return if it does.
        {
            let read_threads = self.threads.read()?;
            match read_threads.get(&output) {
                Some(thread) => {
                    thread.sender.send(c).expect("error when sending command over thread channel");
                    return Ok("dispatched command to thread");
                },
                None => {} // do nothing and release the lock!
            }
        }
        if spawn {
            return self.spawn_thread(output, c);
        } else {
            return Err(CommandError::new("invalid thread command: no existing render thread for this output name (Render command can spawn new threads)"));
        }
    }

    fn spawn_thread(&self, output: String, c: ThreadCommand) -> Result<&str, DaemonError> {
        let thread = ThreadHandle::new(Arc::new(self.clone()));
        thread.sender.send(c).expect("could not send initial command to thread after spawning");
        {
            let mut write_threads = self.threads.write()?;
            write_threads.insert(output, thread);
        }
        return Ok("spawned thread and dispatched render command");
    }

    fn info(&self, _cmd: InfoCommand) -> Result<&str, DaemonError> {
        todo!();
    }

    fn load_image(&self, cmd: LoadImageCommand) -> Result<&str, DaemonError>  {
        let img= ImageReader::open(cmd.image.clone())?.decode()?;
        {
            let images_lock = self.images.write();
            match images_lock {
                Ok(mut images_table) => {
                    if images_table.contains_key(&cmd.image.clone()) {
                        return Err(CommandError::new("file already loaded/present in image table"));
                    }
                    images_table.insert(cmd.image, img.into_rgba8());
                    return Ok("image loaded successfully");
                }
                Err(e) => Err(DaemonError::from(e)),
            }
        }
    }

    // TODO: maybe figure out a way to in-place read-only copy/reference pass to the threads? would be nice..
    pub fn get_image (&self, img: String) -> Result<RgbaImage, DaemonError> {
        {
            let images = self.images.read()?;
            if images.contains_key(&img) {
                return Ok(images.get(&img).unwrap().clone());
            }
        }
        // image not preloaded.... sigh.....
        self.load_image(LoadImageCommand {image: img.clone()})?;
        // self.get_image(img) would be funny, but could recurse infinitely. lazy copy/paste instead.
        {
            let images = self.images.read()?;
            if images.contains_key(&img) {
                return Ok(images.get(&img).unwrap().clone());
            } else {
                return Err(CommandError::new("???")) // should be unreachable if the load_image didn't error out on us.
            }
        }
    }

    fn reload_config(&self, _cmd: ConfigReloadCommand) -> Result<&str, DaemonError> {
        todo!();
    }

    fn cleanup_thread(&self, output: String) -> Result<&str, DaemonError> {
        {
            let mut write_threads = self.threads.write().expect("could not acquire read lock for dispatching command");
            return match write_threads.remove(&output) {
                Some(thread) => {
                    thread.thread.join().expect("failed to join thread handle after stopping?");
                    Ok("thread stopped successfully")
                },
                None => Err(CommandError::new("named thread already stopped or otherwise didn't exist")),
            }
        }
    }

    pub fn start(&self) {
        self.ipc.as_ref().expect("ipc handler should be bound before start").start_listen();
    }

    pub fn process_ipc(&self, socket: UnixStream) -> () {
        let cmd = pithos::sockets::read_command_from_client_socket(socket.try_clone().expect("couldn't clone socket"));
        let response = match self.handle_cmd(cmd) {
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

    fn handle_cmd(&self, cmd: CommandType) -> Result<&str, DaemonError> {
        dbg!(cmd.clone());
        return match cmd {
            CommandType::Dc(dc) => self.handle_daemon_command(dc),
            // CommandType::Ac(ac) => {}
            CommandType::Tc(tc) => self.handle_thread_command(tc),
        };
    }
}