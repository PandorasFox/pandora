use std::{env, path::PathBuf};
use miette::{IntoDiagnostic, Context};
/*
    My personal config wants to look something like:
output "DP-5" {
    image "~/pictures/wallpapers/plushie hoard.png"
    mode "vertical"
}

output "DP-6" {
    workspace "🗨️" {
        image "~/pictures/wallpapers/plushie hoard.png"
        mode static
        trigger "workspace name"
    }

    lockscreen {
        image "~/pictures/wallpapers/pandora (neon white).png"
        mode "static"
    }

    image "~/pictures/wallpapers/pawdi.png"
    mode "static"
}
*/

#[derive(Debug, knuffel::Decode)]
pub enum ConfigNode {
    Output(OutputConfig),
}

#[derive(Debug, knuffel::DecodeScalar)]
pub enum ConfigTriggers {
    Locked,
    WorkspaceName,
}

#[derive(Debug, knuffel::DecodeScalar)]
pub enum RenderMode {
    Static,
    ScrollVertical,
    ScrollLateral,
}

#[derive(Debug, knuffel::DecodeScalar)]
pub enum LockRenderMode {
    Static,
}

#[derive(Debug, Default, knuffel::Decode)]
pub struct OutputConfig {
    #[knuffel(argument)]
    pub name: String,
    #[knuffel(child, unwrap(argument))]
    pub image: String,
    #[knuffel(child, unwrap(argument))]
    pub mode: Option<RenderMode>,
    // sub-items
    #[knuffel(child)]
    pub lockscreen: Option<LockConfig>,
    #[knuffel(children(name="workspace"))]
    pub workspaces: Option<Vec<WorkspaceConfig>>,
}

#[derive(Debug, knuffel::Decode)]
pub struct LockConfig {
    #[knuffel(child, unwrap(argument))]
    pub image: String,
    #[knuffel(child, unwrap(argument))]
    pub mode: Option<LockRenderMode>, // just 'static' for now, but I want to figure out some other funny eyecandy later
}

/// workspace "name" {
///     image "~/path/to/img.png"
///     mode static
///     trigger "workspace name"
/// }
#[derive(Debug, knuffel::Decode)]
pub struct WorkspaceConfig {
    #[knuffel(argument)]
    pub name: String,
    #[knuffel(child, unwrap(argument))]
    pub image: String,
    #[knuffel(child, unwrap(argument))]
    pub mode: Option<RenderMode>,
    #[knuffel(child, unwrap(arguments))]
    pub trigger: Vec<ConfigTriggers>,
}

pub fn load_config() -> miette::Result<Vec<ConfigNode>> {
    let mut config_path = PathBuf::new();
    match env::var("XDG_CONFIG_HOME") {
        Ok(s) => config_path.push(s),
        Err(_) => config_path.push("~/.config"),
    };

    config_path.push("pandora.kdl");
    let processed_path = shellexpand::full(config_path.to_str().unwrap()).unwrap();

    let config = knuffel::parse::<Vec<ConfigNode>>(&processed_path, r#"
output "DP-5" {
    image "~/pictures/wallpapers/plushie hoard.png"
    mode "scroll-vertical"
}

output "DP-6" {
    workspace "🗨️" {
        image "~/pictures/wallpapers/plushie hoard.png"
        mode "static"
        trigger "workspace-name"
    }

    lockscreen {
        image "~/pictures/wallpapers/pandora (neon white).png"
        mode "static"
    }

    image "~/pictures/wallpapers/pawdi.png"
    mode "static"
}   
"#)?;

    return Ok(config);
}