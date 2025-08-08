use crate::threads::config::ConfigWatcher;
use crate::threads::niri::NiriAgent;
use crate::threads::outputs::OutputHandler;
use crate::threads::ipc::InboundCommandHandler;
use crate::threads::render::{RenderThread};
use ::pandora::pithos::misc::get_new_image_dimensions;
use ::pandora::pithos::commands::{CommandType, DaemonCommand, LoadImageCommand, RenderThreadCommand};
use ::pandora::pithos::error::{CommandError, DaemonError};
use ::pandora::pithos::sockets::write_response_to_client_socket;
use ::pandora::wayland::render_helpers::RenderThreadWaylandState;

use std::collections::HashMap;
use std::fs::File;
use std::os::unix::net::{UnixStream};
use std::sync::{Arc, RwLock, Weak};
use std::sync::mpsc::{channel, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use image::imageops::FilterType;
use image::{RgbaImage, ImageReader};
use wayrs_client::Connection;

// daemon utility struct(s)
pub struct ThreadHandle {
    sender: Sender<RenderThreadCommand>,
    thread: JoinHandle<()>,
}

impl ThreadHandle {
    fn new(output: String, pandora: Arc<Pandora>) -> ThreadHandle {
        let (host_sender, thread_receiver) = channel::<RenderThreadCommand>();
        let conn = Connection::<RenderThreadWaylandState>::connect().unwrap();

        let thread = thread::spawn(move || {
            RenderThread::new(
                output,
                thread_receiver,
                pandora,
                conn,
            ).start();
        });

        return ThreadHandle {
            sender: host_sender,
            thread: thread,
        };
    }
}

#[derive(Clone)]
pub struct Pandora {
    cmd_ipc_thread: Option<Arc<InboundCommandHandler>>,
    outputs_thread: Option<Arc<OutputHandler>>,
    niri_ag_thread: Option<Arc<NiriAgent>>,
    configw_thread: Option<Arc<ConfigWatcher>>,
    // key: output name
    threads: Arc<RwLock<HashMap<String, ThreadHandle>>>,
    // key: file path
    // useful central cache of loaded images for lockscreen etc
    images: Arc<RwLock<HashMap<String, RgbaImage>>>,
    // agent: Arc<AgentHandler>,
}

impl Pandora {
    pub fn log(&self, s: String) {
        // need to evaluate logging libraries & integrate clap/ipc back into main binary
        println!("> pandora: {s}");
    }

    pub fn new() -> Arc<Pandora> {
        return Arc::new(Pandora {
            cmd_ipc_thread: None,
            outputs_thread: None,
            niri_ag_thread: None,
            configw_thread: None,
            threads: Arc::new(RwLock::new(HashMap::<String, ThreadHandle>::new())),
            images: Arc::new(RwLock::new(HashMap::<String, RgbaImage>::new())),
        });
    }

    pub fn bind_threads(&mut self,
        ipc: Arc<InboundCommandHandler>,
        outputs: Arc<OutputHandler>,
        niri: Arc<NiriAgent>,
        config: Arc<ConfigWatcher>,
    ) -> &mut Self {
        self.cmd_ipc_thread = Some(ipc);
        self.outputs_thread = Some(outputs);
        self.niri_ag_thread = Some(niri);
        self.configw_thread = Some(config);
        return self;
    }

    fn reload_config(&self, cmd: &DaemonCommand) {
        // if sending to the other perpetual-threads fails i am assuming shit's fucked for other reasons
        let _ = self.outputs_thread.as_ref().unwrap().queue.send(cmd.clone());
        let _ = self.niri_ag_thread.as_ref().unwrap().queue.send(cmd.clone());
    }

    pub fn start(&self, weak: Weak<Pandora> ) {
        self.outputs_thread.as_ref().unwrap().start(weak.clone());
        self.niri_ag_thread.as_ref().unwrap().start(weak.clone());
        self.configw_thread.as_ref().unwrap().start(weak.clone());
        // main thread control flow loop
        self.log("startup completed; entering into ipc listen loop! :3".to_string());
        self.cmd_ipc_thread.as_ref().unwrap().start(weak);
    }

    pub fn handle_cmd(&self, cmd: &CommandType){
        match cmd {
            CommandType::Dc(dc) => self.handle_daemon_command(&dc),
            // CommandType::Ac(ac) => {}
            CommandType::Tc(tc) => self.handle_thread_command(&tc),
        };
    }

    fn handle_daemon_command(&self, dc: &DaemonCommand) {
        match dc {
            DaemonCommand::LoadImage(c) => _ = self.load_image(&c.image.clone()),
            DaemonCommand::ReloadConfig(_) => {
                self.reload_config(dc);
            },
            DaemonCommand::Stop => {
                self.log("goodbye!".to_string());
                std::process::exit(0);
            },
            DaemonCommand::OutputModeChange(_) => {
                let _ = self.niri_ag_thread.as_ref().unwrap().queue.send(dc.clone());
            }
        };
    }

    fn handle_thread_command(&self, tc: &RenderThreadCommand) {
        let output: String;
        let mut can_spawn = false;
        let mut join_after = false;
        let mut image_to_preload: Option<String> = None;
        match tc.clone() {
            RenderThreadCommand::Render(c) => {
                output = c.output;
                can_spawn = true;
                image_to_preload = Some(c.image);
            }
            RenderThreadCommand::Stop(c) => {
                output = c.output;
                join_after = true;
            }
            RenderThreadCommand::Scroll(c) => {
                output = c.output;
            },
        };
        if image_to_preload.is_some() {
            self.handle_daemon_command(&DaemonCommand::LoadImage(LoadImageCommand { image:image_to_preload.unwrap() }));
        }
        let ret = self.dispatch_thread_command(output.clone(), &tc, can_spawn);
        if join_after && ret.is_ok() { // if a stop command error'd in dispatch, it either crashed or didn't exist; no need to clean up
            // if we full-steam ahead, we will get to .is_finished before the thread might be finished
            thread::sleep(Duration::from_millis(1)); // seems to be sufficient for letting the thread exit before we clean it up
            self.cleanup_thread(&output);
        }
    }

    fn dispatch_thread_command(&self, output: String, c: &RenderThreadCommand, spawn: bool) -> Result<(), DaemonError> {
        // read lock:
        // check if thread exists, dispatch and return if it does.
        {
            let read_threads = self.threads.read()?;
            match read_threads.get(&output) {
                Some(thread) => {
                    if thread.sender.send(c.clone()).is_ok() {
                        return Ok(());
                    }
                    drop(read_threads); // drop read thread bc we're done reading & need to re-enter to clean up thread
                    self.cleanup_thread(&output);
                    return Err(CommandError::new("could not send command to thread; cleaned it up"));
                },
                None => {
                    drop(read_threads); // release lock, result not found.
                    if spawn {
                       return self.spawn_thread(output, c);
                    } else {
                        return Err(CommandError::new("invalid thread command: thread does not exist"));
                    }
                },
            }
        }
        
    }

    fn spawn_thread(&self, output: String, c: &RenderThreadCommand) -> Result<(), DaemonError> {
        let thread = ThreadHandle::new(output.clone(), Arc::new(self.clone()));
        thread.sender.send(c.clone()).expect("could not send initial command to thread after spawning");
        {
            let mut write_threads = self.threads.write()?;
            write_threads.insert(output, thread);
        }
        return Ok(());
    }

    pub fn load_image(&self, path: &String) -> Result<(), DaemonError>  {
        let img= ImageReader::open(path.clone())?.decode()?;
        {
            let images_lock = self.images.write();
            match images_lock {
                Ok(mut images_table) => {
                    if images_table.contains_key(&path.clone()) {
                        self.log(format!("file {} already loaded", path.clone()));
                        return Ok(());
                    }
                    images_table.insert(path.clone(), img.into_rgba8());
                    self.log(format!("file {} loaded", path.clone()));
                    return Ok(());
                }
                Err(e) => panic!("{e:?}"),
            }
        }
    }

    pub fn get_image_dimensions(&self, img: String) -> Result<(u32, u32), ()> {
        {
            let images_lock = self.images.read();
            match images_lock {
                Ok(images_table) => {
                    if images_table.contains_key(&img) {
                        let image = images_table.get(&img).unwrap();
                        return Ok((image.width(), image.height()));
                    } else {
                        return Err(());
                    }
                },
                Err(e) => panic!("{e:?}"),
            };
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
                    ::pandora::pithos::misc::img_into_buffer(
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
                    ::pandora::pithos::misc::img_into_buffer(image, &f);
                    return Ok((image.width(), image.height()));
                }
            };
        }
    }

    fn cleanup_thread(&self, output: &String) {
        {
            let mut write_threads = self.threads.write().expect("could not acquire read lock for dispatching command");
            match write_threads.remove(output) {
                Some(thread) => {
                    write_threads.shrink_to_fit();
                    if thread.thread.is_finished() {
                        let _ = thread.thread.join();
                        return;
                    } else {
                        // can happen when a stop is issued -> daemon gets here before the thread stops
                        // .... but also could happen on a genuine wedge, so we don't want to lie about that.
                        self.log(format!("could not join to thread for {output} (wedged?)"));
                    }
                },
                None => self.log(format!("named thread for {output} already stopped or doesn't exist")),
            }
        }
    }

    pub fn process_ipc(&self, socket: &UnixStream) {
        let cmd = ::pandora::pithos::sockets::read_command_from_client_socket(&socket.try_clone().expect("couldn't clone socket"));
        self.handle_cmd(&cmd);
        write_response_to_client_socket("command dispatched", socket).expect("failed to write response to inbound ipc");
    }
}