use super::config::DaemonConfig;
use serde::{Deserialize, Serialize};
// ===== TRAITS AND MISC DATA STRUCTS =====
#[derive(knuffel::DecodeScalar, Serialize, Deserialize, PartialEq, Copy, Clone, Debug)]
pub enum RenderMode {
    // single image
    Static, // will scale up/down to fill
    ScrollVertical,
    ScrollLateral,
    // scrolling both directions will be trickier to implement. later problem.
    // hello from later me: honestly it's probably easier than I thought:
    // the agent can enforce positional state well, & correcting-on-the-fly looks better than expected
}

// ===== COMMAND STRUCTS =====
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RenderCommand {
    pub output: String,
    pub image: String,
    pub mode: RenderMode,
    pub position: (f64, f64),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScrollCommand {
    pub output: String,
    // scroll position tracks the center of the viewport into the canvas
    // e.g. 0% is top edge at top of screen, 100% is bottom edge at bottom of screen,
    // 50% is center of screen is center of image
    pub position_x: f64, // 0.0 => 100.0
    pub position_y: f64, // 0.0 => 100.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum CommandType {
    // commands for the daemon & other Forever Threads (outputs watcher, compositor agent)
    Dc(DaemonCommand),
    // commands for a specific render thread, dispatched by output name
    Tc(RenderThreadCommand),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum DaemonCommand {
    Lock,
    ReloadConfig(DaemonConfig),
    Stop,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum RenderThreadCommand {
    Render(RenderCommand),
    Scroll(ScrollCommand),
    ConfigReload(DaemonConfig),
}
