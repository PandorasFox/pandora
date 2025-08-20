use std::sync::{Arc, Weak};

use crate::{daemon::Daemon, pithos::config::DaemonConfig};

pub mod config;
pub mod ipc;
pub mod lockscreen;
pub mod logger; // logChamp
pub mod niri;
pub mod render;

pub trait Thread {
    fn new() -> Arc<Self>;
    fn start(&self, daemon: Weak<dyn Daemon + Sync + Send>, config: DaemonConfig);
}
