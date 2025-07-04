use serde::{Serialize, Deserialize};
// ===== TRAITS AND MISC DATA STRUCTS =====
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScrollPosition {
    pub start: i32,
    pub end: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum RenderMode {
    // single image
    Static, // will scale up/down to fill
    ScrollingVertical(ScrollPosition),
    ScrollingLateral(ScrollPosition),
    // scrolling both directions will be trickier to implement. later problem.
}

// ===== COMMAND STRUCTS =====
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct InfoCommand {
    pub verbose: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ConfigReloadCommand {
    pub file: String,
}

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
    pub position: ScrollPosition,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum CommandType {
    Dc(DaemonCommand),
    Tc(ThreadCommand),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum DaemonCommand {
    Info(InfoCommand),
    ConfigReload(ConfigReloadCommand),
    LoadImage(LoadImageCommand),
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum ThreadCommand {
    Render(RenderCommand),
    Stop(StopCommand),
    Scroll(ScrollCommand),
}

// TODO TESTS =)
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let result = 2 + 2;
        assert_eq!(result, 4);
    }
}
