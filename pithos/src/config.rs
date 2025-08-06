use std::{env, fs, path::Path};

#[derive(Clone, Debug, knuffel::Decode)]
pub enum ConfigNode {
    Output(OutputConfig),
}

#[derive(Clone, Debug, knuffel::DecodeScalar)]
pub enum ConfigTriggers {
    Locked,
    WorkspaceName,
}

#[derive(Clone, Debug, knuffel::DecodeScalar)]
pub enum RenderModeConfig {
    Static,
    ScrollVertical,
    ScrollLateral,
}

#[derive(Clone, Debug, knuffel::DecodeScalar)]
pub enum LockRenderMode {
    Static,
}

#[derive(Clone, Debug, Default, knuffel::Decode)]
pub struct OutputConfig {
    #[knuffel(argument)]
    pub name: String,
    #[knuffel(child, unwrap(argument))]
    pub image: String,
    #[knuffel(child, unwrap(argument))]
    pub mode: Option<RenderModeConfig>,
    // sub-items
    #[knuffel(child)]
    pub lockscreen: Option<LockConfig>,
    #[knuffel(children(name="workspace"))]
    pub workspaces: Option<Vec<WorkspaceConfig>>,
}

#[derive(Clone, Debug, knuffel::Decode)]
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
#[derive(Clone, Debug, knuffel::Decode)]
pub struct WorkspaceConfig {
    #[knuffel(argument)]
    pub name: String,
    #[knuffel(child, unwrap(argument))]
    pub image: String,
    #[knuffel(child, unwrap(argument))]
    pub mode: Option<RenderModeConfig>,
    #[knuffel(child, unwrap(arguments))]
    pub trigger: Vec<ConfigTriggers>,
}

#[derive(Clone, Default, Debug)]
pub struct DaemonConfig {
    pub outputs: Vec<OutputConfig>,
    // lockscreen: LockscreenConfig,
}

pub fn load_config() -> miette::Result<DaemonConfig> {
    //let mut config_path = PathBuf::new();
    let base_dir = match env::var("XDG_CONFIG_HOME") {
        Ok(s) => shellexpand::full(&s).unwrap().into_owned(),
        Err(_) => shellexpand::full("~/.config").unwrap().into_owned(),
    };
    let config_path = Path::new(&base_dir).join("pandora.kdl");
    let config_file_contents = fs::read_to_string(config_path.clone()).unwrap();
    let config_nodes = knuffel::parse::<Vec<ConfigNode>>(config_path.to_str().unwrap(), config_file_contents.as_str())?;
    
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

    return Ok(config);
}