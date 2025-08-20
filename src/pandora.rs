use miette::Result;
use pandora::daemon::Daemon;
use pandora::pithos::commands::{CommandType, DaemonCommand, RenderThreadCommand};
use pandora::pithos::config::{DaemonConfig, LogLevel};
use pandora::pithos::error::{CommandError, DaemonError};
use pandora::pithos::misc::get_new_image_dimensions;
use pandora::threads::Thread;
use pandora::threads::config::ConfigWatcher;
use pandora::threads::logger::LogThread;
use pandora::threads::niri::NiriAgent;
use pandora::threads::render::WallpaperThreadHandle;
use pandora::wayland::render_base::{OutputState, RenderThreadState};

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::{self, File};
use std::sync::{Arc, RwLock, Weak};
use std::thread;

use image::imageops::FilterType;
use image::{ImageReader, RgbaImage};
use wayrs_client::Connection;
use wayrs_client::protocol::{WlOutput, WlSurface};
use wayrs_protocols::wlr_layer_shell_unstable_v1::{
    ZwlrLayerShellV1, zwlr_layer_shell_v1::Layer, zwlr_layer_surface_v1::Anchor,
};

#[derive(Clone)]
pub struct Pandora {
    logger: Arc<LogThread>,
    niri_ag_thread: Option<Arc<NiriAgent>>,
    configw_thread: Arc<ConfigWatcher>,
    // key: output name
    bgwallp_thread: Arc<WallpaperThreadHandle>,
    // key: file path
    // useful central cache of loaded images for lockscreen etc
    images: Arc<RwLock<HashMap<String, RgbaImage>>>,
    // agent: Arc<AgentHandler>,
    config: Arc<RwLock<DaemonConfig>>,
}

impl Daemon for Pandora {
    fn log(&self, name: &str, msg: String) {
        self.logger.log(LogLevel::DEFAULT, name, msg);
    }

    fn debug(&self, name: &str, msg: String) {
        self.logger.log(LogLevel::DEBUG, name, msg);
    }

    fn verbose(&self, name: &str, msg: String) {
        self.logger.log(LogLevel::VERBOSE, name, msg);
    }

    fn apply_role_to_surface(
        self: Arc<Self>,
        conn: &mut Connection<RenderThreadState>,
        wl_surface: &WlSurface,
        wl_output: &WlOutput,
        output_state: &OutputState,
    ) {
        let width = output_state.width;
        let height = output_state.height;
        let layer_shell = conn.bind_singleton::<ZwlrLayerShellV1>(4..=5).unwrap();
        let layer_surface = layer_shell.get_layer_surface(
            conn,
            *wl_surface,
            Some(*wl_output),
            Layer::Background,
            CString::new("pandora").unwrap(),
        );

        layer_surface.set_size(conn, width as u32, height as u32);
        layer_surface.set_anchor(
            conn,
            Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right,
        );
        layer_surface.set_exclusive_zone(conn, -1);

        conn.set_callback_for(
            layer_surface,
            pandora::wayland::render_base::layer_shell_callback,
        );
        wl_surface.commit(conn);
        conn.blocking_roundtrip().unwrap();
    }

    fn handle_cmd(self: Arc<Self>, cmd: &CommandType) {
        match cmd {
            CommandType::Dc(dc) => self.handle_daemon_command(&dc),
            // CommandType::Ac(ac) => {}
            CommandType::Tc(tc) => self.handle_thread_command(&tc),
        };
    }

    fn load_image(self: Arc<Self>, path: &String) -> Result<(), DaemonError> {
        {
            // read lock
            match self.images.read() {
                Ok(images_check) => {
                    if images_check.contains_key(&path.clone()) {
                        self.verbose("pandora", format!("file {} already loaded", path.clone()));
                        return Ok(());
                    }
                }
                Err(e) => panic!("{e:?}"),
            }
        }
        let img = ImageReader::open(path.clone())?.decode()?;
        {
            //write lock
            match self.images.write() {
                Ok(mut images_table) => {
                    if images_table.contains_key(&path.clone()) {
                        // could have been written while we loaded the image!
                        self.verbose("pandora", format!("file {} already loaded", path.clone()));
                        return Ok(());
                    }
                    images_table.insert(path.clone(), img.into_rgba8());
                    self.log("pandora", format!("file {} loaded", path.clone()));
                    return Ok(());
                }
                Err(e) => panic!("{e:?}"),
            }
        }
    }

    fn get_image_dimensions(self: Arc<Self>, img: String) -> Result<(u32, u32), ()> {
        match self.images.read() {
            Ok(images_table) => {
                if images_table.contains_key(&img) {
                    let image = images_table.get(&img).unwrap();
                    return Ok((image.width(), image.height()));
                } else {
                    return Err(());
                }
            }
            Err(e) => panic!("{e:?}"),
        };
    }

    // if scale_to is provided, uses the provided width/height dimensions of the output to scale image appropriately
    // if only one dimension is provided, scales to that one and keeps aspect ratio.
    fn read_img_to_file(
        self: Arc<Self>,
        img: &String,
        f: &File,
        scale_to: Option<(Option<u32>, Option<u32>)>,
    ) -> Result<(u32, u32), DaemonError> {
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
                    let (new_width, new_height) = get_new_image_dimensions(
                        image.width(),
                        image.height(),
                        maybe_width,
                        maybe_height,
                    );
                    ::pandora::pithos::misc::img_into_buffer(
                        &image::imageops::resize(
                            image,
                            new_width as u32,
                            new_height as u32,
                            FilterType::Lanczos3,
                        ),
                        &f,
                    );
                    return Ok((new_width, new_height));
                }
                None => {
                    ::pandora::pithos::misc::img_into_buffer(image, &f);
                    return Ok((image.width(), image.height()));
                }
            };
        }
    }
}

impl Pandora {
    pub fn new(config: DaemonConfig, verbosity: LogLevel) -> Arc<Pandora> {
        let logger = LogThread::new(verbosity);
        let config_watcher = ::pandora::threads::config::ConfigWatcher::new();
        let wallpaper = WallpaperThreadHandle::new();
        let niri = match ::pandora::threads::niri::NiriAgent::new(config.clone()) {
            Ok(agent) => Some(agent),
            Err(_e) =>
            /* log _e */
            {
                None
            }
        };

        return Arc::new(Pandora {
            logger: logger,
            niri_ag_thread: niri,
            configw_thread: config_watcher,
            bgwallp_thread: wallpaper,
            images: Arc::new(RwLock::new(HashMap::<String, RgbaImage>::new())),
            config: Arc::new(RwLock::new(config)),
        });
    }

    pub fn start(self: Arc<Self>, weak: Weak<Pandora>, config: DaemonConfig) -> miette::Result<()> {
        match self.clone().try_load_images(&config) {
            Err(msg) => return Err(miette::miette!(msg)),
            Ok(()) => (),
        };
        self.configw_thread.start(weak.clone());
        self.bgwallp_thread.start(weak.clone(), config);
        match &self.niri_ag_thread {
            Some(niri) => niri.start(weak.clone()),
            None => self.log("pandora", "niri agent thread could not spawn!!".to_string()),
        };

        // main thread control flow loop
        self.log(
            "pandora",
            "startup completed; entering into ipc listen loop! :3".to_string(),
        );
        ::pandora::threads::ipc::InboundCommandHandler::new().start(weak);
        Ok(())
    }

    fn reload_config(self: Arc<Self>, config: &DaemonConfig) {
        match self.config.write() {
            Ok(mut conf) => {
                conf.outputs = config.outputs.clone();
                // if we add more non-logging config nodes we handle them here
                // i guess we could actually update the verbosity level here and pull that out of the logger..... eh.
            }
            Err(e) => {
                self.log("pandora", format!("{e:?}"));
            }
        }
        match self.clone().try_load_images(config) {
            Err(msg) => return self.log("pandora", msg),
            Ok(()) => (),
        };
        let _ = self
            .niri_ag_thread
            .as_ref()
            .unwrap()
            .queue
            .send(DaemonCommand::ReloadConfig(config.clone()));
    }

    fn try_load_images(self: Arc<Pandora>, config: &DaemonConfig) -> Result<(), String> {
        for output in &config.outputs {
            self.clone().try_load_image(output.image.clone())?;
            if output.lockscreen.is_some() {
                let path = output.lockscreen.as_ref().unwrap().image.clone();
                self.clone().try_load_image(path)?;
            }
            if output.workspaces.is_some() {
                for workspace in output.workspaces.as_ref().unwrap() {
                    self.clone().try_load_image(workspace.image.clone())?;
                }
            }
        }
        return Ok(());
    }

    fn try_load_image(self: Arc<Pandora>, path: String) -> Result<(), String> {
        if fs::exists(&path.clone()).is_err() {
            return Err(format!("could not preload {path} during init"));
        }
        thread::spawn(move || self.load_image(&path));
        return Ok(());
    }

    fn handle_daemon_command(self: Arc<Pandora>, dc: &DaemonCommand) {
        match dc {
            DaemonCommand::ReloadConfig(config) => {
                self.reload_config(config);
            }
            DaemonCommand::Stop => {
                self.log("pandora", "goodbye!".to_string());
                std::process::exit(0);
            }
            DaemonCommand::OutputModeChange(_) => {
                let _ = self.niri_ag_thread.as_ref().unwrap().queue.send(dc.clone());
            }
            DaemonCommand::Lock => self.lock(),
        };
    }

    fn lock(self: Arc<Pandora>) {
        {
            match self.config.read() {
                Ok(conf) => {
                    pandora::threads::lockscreen::lock(self.logger.inbox.clone(), conf.clone())
                }
                Err(_) => self.log(
                    "pandora",
                    "locking screen failed: could not acquire config read-lock".to_string(),
                ),
            }
        }
    }

    fn handle_thread_command(self: Arc<Pandora>, tc: &RenderThreadCommand) {
        let _ = self.bgwallp_thread.inbox.send(tc.clone());
    }
}
