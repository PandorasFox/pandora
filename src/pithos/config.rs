use std::{env, fs, path::{Path, PathBuf}, thread, time::Duration};

use super::commands::RenderMode;

#[derive(Clone, Debug, knuffel::Decode, serde::Serialize, serde::Deserialize)]
pub enum ConfigNode {
    Output(OutputConfig),
}

#[derive(Clone, Debug, knuffel::DecodeScalar, serde::Serialize, serde::Deserialize)]
pub enum ConfigTriggers {
    Locked,
    WorkspaceName,
}

#[derive(Clone, Debug, knuffel::DecodeScalar, serde::Serialize, serde::Deserialize)]
pub enum LockRenderMode {
    Static,
}

#[derive(Clone, Debug, Default, knuffel::Decode, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, knuffel::Decode, serde::Serialize, serde::Deserialize)]
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
#[derive(Clone, Debug, knuffel::Decode, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct DaemonConfig {
    pub outputs: Vec<OutputConfig>,
    // lockscreen: LockscreenConfig,
}

pub fn get_config_dir() -> PathBuf {
    let base_dir = match env::var("XDG_CONFIG_HOME") {
        Ok(s) => shellexpand::full(&s).unwrap().into_owned(),
        Err(_) => shellexpand::full("~/.config").unwrap().into_owned(),
    };
    return Path::new(&base_dir).join("pandora");
}

fn try_load_file(path: &PathBuf) -> Option<String> {
    // vim and some other editors will write to a swap buffer, then upon save, copy the swap over the original
    // this lead to weird race conditions. we fight this by doing a few fs::exists and and read attempts in a row
    // with some micro sleeps between attempts. miette! if we fail a few times in a row.
    for _ in 1..3 {
        if path.exists() {
            match fs::read_to_string(path) {
                Ok(s) => return Some(s),
                Err(_) => ()
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    None
}

static mut LAST_CONFIG_FILE_CONTENTS: String = String::new();

pub fn load_config() -> miette::Result<DaemonConfig> {
    let config_dir = get_config_dir();
    let config_path = config_dir.join("pandora.kdl");
    let config_file_contents = try_load_file(&config_path);
    if config_file_contents.is_none() {
        return Err(miette::miette!("Could not load config file from fs (if editing with vim, try backupcopy yes)"));
    }

    unsafe { // hot reloading config files while also debouncing the reloads is fucking annoying :/ 
        if LAST_CONFIG_FILE_CONTENTS == config_file_contents.clone().unwrap() {
            return Err(miette::miette!("config file contents unchanged since last reload"));
        }
    }

    let config_nodes = knuffel::parse::<Vec<ConfigNode>>(config_path.to_str().unwrap(), config_file_contents.clone().unwrap().as_str())?;
    
    let mut config = DaemonConfig { outputs: Vec::new() };
    for node in config_nodes {
        match node {
            ConfigNode::Output(mut n) => {
                n.image = shellexpand::full(&n.image).unwrap().to_string();
                if n.workspaces.is_some() {
                    for wsc in n.workspaces.as_mut().unwrap() {
                       wsc.image = shellexpand::full(&wsc.image).unwrap().to_string();
                    }
                }
                if n.lockscreen.is_some() {
                    n.lockscreen.as_mut().unwrap().image = shellexpand::full(&n.lockscreen.as_ref().unwrap().image).unwrap().to_string();
                }
                config.outputs.push(n)
            }
        }
    }

    unsafe { // lol
        LAST_CONFIG_FILE_CONTENTS = config_file_contents.unwrap();
    }
    return Ok(config);
}