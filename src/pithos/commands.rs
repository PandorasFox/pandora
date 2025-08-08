use serde::{Serialize, Deserialize};

use super::config::DaemonConfig;
// ===== TRAITS AND MISC DATA STRUCTS =====
#[derive(knuffel::DecodeScalar, Serialize, Deserialize, Copy, Clone, Debug)]
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
pub struct LoadImageCommand {
    pub image: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RenderCommand {
    pub output: String,
    pub image: String,
    pub mode: RenderMode,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StopCommand {
    pub output: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScrollCommand {
    pub output: String,
    pub position: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ModeCommand {
    pub output: String,
    pub new_width: i32,
    pub new_height: i32,
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
    LoadImage(LoadImageCommand),
    ReloadConfig(DaemonConfig),
    OutputModeChange(ModeCommand),
    Stop,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum RenderThreadCommand {
    Render(RenderCommand),
    Stop(StopCommand),
    Scroll(ScrollCommand),
}
