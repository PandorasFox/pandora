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

use fast_image_resize::images::Image;
use fast_image_resize::{IntoImageView, Resizer};
use image::{DynamicImage, ImageReader};
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
    images: Arc<RwLock<HashMap<String, DynamicImage>>>,
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
        conn.set_callback_for(
            layer_surface,
            pandora::wayland::render_base::layer_shell_callback,
        );

        layer_surface.set_size(conn, width as u32, height as u32);
        layer_surface.set_anchor(
            conn,
            Anchor::Top | Anchor::Left | Anchor::Bottom | Anchor::Right,
        );
        layer_surface.set_exclusive_zone(conn, -1);
        
        wl_surface.commit(conn);
        conn.blocking_roundtrip().unwrap();
    }

    fn handle_cmd(self: Arc<Self>, cmd: &CommandType) {
        match cmd {
            CommandType::Dc(dc) => self.handle_daemon_command(dc),
            // CommandType::Ac(ac) => {}
            CommandType::Tc(tc) => self.handle_thread_command(tc),
        };
    }

    fn load_image(self: Arc<Self>, path: &str) -> Result<(), DaemonError> {
        let start = std::time::Instant::now();
        {
            // read lock
            match self.images.read() {
                Ok(images_check) => {
                    if images_check.contains_key(&path.to_string()) {
                        self.debug("pandora", format!("file {} already loaded", path));
                        return Ok(());
                    }
                }
                Err(e) => panic!("{e:?}"),
            }
        }
        let img = ImageReader::open(path)?.decode()?;
        let img_loaded = std::time::Instant::now();
        self.verbose(
            "pandora",
            format!("image loaded in {:?}", img_loaded - start),
        );
        {
            //write lock
            match self.images.write() {
                Ok(mut images_table) => {
                    if images_table.contains_key(&path.to_string()) {
                        // written while we loaded the image! try to eliminate this case as much as we can.
                        self.verbose(
                            "pandora",
                            format!(
                                "file {} already loaded (during read, but beaten to write)",
                                path
                            ),
                        );
                        return Ok(());
                    }
                    images_table.insert(path.to_string(), img /*.into_rgba8() */);
                    let img_inserted = std::time::Instant::now();
                    self.debug("pandora", format!("file {} loaded", path));
                    self.verbose(
                        "pandora",
                        format!(
                            "image inserted into table in {:?} (cumulative for call: {:?}",
                            img_inserted - img_loaded,
                            img_inserted - start
                        ),
                    );
                    Ok(())
                }
                Err(e) => panic!("{e:?}"),
            }
        }
    }

    fn get_image_dimensions(self: Arc<Self>, img: &str) -> Result<(u32, u32), DaemonError> {
        match self.images.read() {
            Ok(images_table) => {
                if images_table.contains_key(&img.to_string()) {
                    let image = images_table.get(&img.to_string()).unwrap();
                    Ok((image.width(), image.height()))
                } else {
                    Err(CommandError::from_message("image not found in cache"))
                }
            }
            Err(_) => Err(DaemonError::PoisonError),
        }
    }

    fn read_img_to_file(
        self: Arc<Self>,
        img: &str,
        f: &File,
        scale_to: (Option<u32>, Option<u32>),
    ) -> Result<(u32, u32), DaemonError> {
        let start = std::time::Instant::now();
        {
            let images = self.images.read()?;
            if let Some(image) = images.get(img) {
                let fetched = std::time::Instant::now();
                let (new_width, new_height) =
                    get_new_image_dimensions(image.width(), image.height(), scale_to.0, scale_to.1);

                let resize_start = std::time::Instant::now();
                let mut dst_image = Image::new(new_width, new_height, image.pixel_type().unwrap());
                let mut resizer = Resizer::new();
                resizer.resize(image, &mut dst_image, None).unwrap();

                let scaletime = std::time::Instant::now();
                self.verbose(
                    "image",
                    format!(
                        "image resize took [{:?}] (cumulative: {:?})",
                        scaletime - resize_start,
                        scaletime - start
                    ),
                );
                let mut buf = std::io::BufWriter::new(f);
                ::pandora::pithos::misc::img_into_buffer(&dst_image, &mut buf);
                let put = std::time::Instant::now();
                self.verbose(
                    "image",
                    format!(
                        "image into buffer took [{:?}] (cumulative: {:?})",
                        put - fetched,
                        put - start
                    ),
                );
                Ok((new_width, new_height))
            } else {
                Err(CommandError::from_message("invalid image (not loaded)"))
            }
        }
    }
}

impl Pandora {
    pub fn new(config: DaemonConfig, verbosity: LogLevel) -> Arc<Pandora> {
        let logger = LogThread::new(verbosity);
        let config_watcher = ::pandora::threads::config::ConfigWatcher::new();
        let wallpaper = WallpaperThreadHandle::new();
        let niri = ::pandora::threads::niri::NiriAgent::new(config.clone()).ok();

        Arc::new(Pandora {
            logger,
            niri_ag_thread: niri,
            configw_thread: config_watcher,
            bgwallp_thread: wallpaper,
            images: Arc::new(RwLock::new(HashMap::<String, DynamicImage>::new())),
            config: Arc::new(RwLock::new(config)),
        })
    }

    pub fn start(self: Arc<Self>, weak: Weak<Pandora>, config: DaemonConfig) -> miette::Result<()> {
        self.bgwallp_thread.start(weak.clone(), config.clone());
        self.configw_thread.start(weak.clone());
        match &self.niri_ag_thread {
            Some(niri) => niri.start(weak.clone()),
            None => self.log("pandora", "niri agent thread could not spawn!!".to_string()),
        };

        // main thread control flow loop
        self.log(
            "pandora :3",
            "startup completed:".to_owned() /*lazy-loading remaining images in config & */ + "entering into ipc listen loop!",
        );
        //match self.clone().try_load_images(&config) {
        //    Err(msg) => return Err(miette::miette!(msg)),
        //    Ok(()) => (),
        //};
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
        if let Err(msg) = self.clone().try_load_images(config) {
            return self.log("pandora", msg);
        };
        let _ = self
            .niri_ag_thread
            .as_ref()
            .unwrap()
            .queue
            .send(DaemonCommand::ReloadConfig(config.clone()));

        // Send config reload to render thread as well
        let _ = self
            .bgwallp_thread
            .inbox
            .send(RenderThreadCommand::ConfigReload(config.clone()));
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
        Ok(())
    }

    fn try_load_image(self: Arc<Pandora>, path: String) -> Result<(), String> {
        if fs::exists(path.clone()).is_err() {
            return Err(format!("could not preload {path} during init"));
        }
        thread::spawn(move || self.load_image(&path));
        Ok(())
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
