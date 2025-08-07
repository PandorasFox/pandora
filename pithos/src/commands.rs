use serde::{Serialize, Deserialize};
// ===== TRAITS AND MISC DATA STRUCTS =====
#[derive(Serialize, Deserialize, Copy, Clone, Debug)]
pub enum RenderMode {
    // single image
    Static, // will scale up/down to fill
    ScrollingVertical(u32),
    ScrollingLateral(u32),
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
pub enum CommandType {
    Dc(DaemonCommand),
    Tc(ThreadCommand),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum DaemonCommand {
    LoadImage(LoadImageCommand),
    Stop,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ThreadCommand {
    Render(RenderCommand),
    Stop(StopCommand),
    Scroll(ScrollCommand),
}
