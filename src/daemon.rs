use crate::pithos::error::DaemonError;
use crate::wayland::render_base::OutputState;
use crate::{pithos::commands::CommandType, wayland::render_base::RenderThreadState};

use std::fs::File;
use std::sync::Arc;

use wayrs_client::Connection;
use wayrs_client::protocol::{WlOutput, WlSurface};

pub trait Daemon {
    fn log(&self, name: &str, msg: String);
    fn debug(&self, name: &str, msg: String);
    fn verbose(&self, name: &str, msg: String);

    fn handle_cmd(self: Arc<Self>, cmd: &CommandType);

    fn load_image(self: Arc<Self>, path: &String) -> Result<(), DaemonError>;
    fn get_image_dimensions(self: Arc<Self>, img: String) -> Result<(u32, u32), ()>;
    fn read_img_to_file(
        self: Arc<Self>,
        img: &String,
        f: &File,
        scale_to: Option<(Option<u32>, Option<u32>)>,
    ) -> Result<(u32, u32), DaemonError>;

    fn apply_role_to_surface(
        self: Arc<Self>,
        conn: &mut Connection<RenderThreadState>,
        wl_surface: &WlSurface,
        wl_output: &WlOutput,
        output_state: &OutputState,
    );
}
