use std::sync::Arc;

use pithos::config::load_config;

mod pandora;
mod ipc;
mod render;
mod agents;

fn main() {
    // TODO refactor cli::main into here, do IPC if any args
    let config = load_config().unwrap();
    // initialize daemon & ipc handlers, and glue them together.
    let mut pandora = crate::pandora::Pandora::new();
    
    let ipc = crate::ipc::InboundCommandHandler::new(pandora.clone());
    let outputs = crate::agents::outputs::OutputHandler::new(config.clone(), pandora.clone());
    let niri = crate::agents::niri::NiriAgent::new(config.clone(), pandora.clone());
    // config_watcher thread next!

    let pandora = Arc::make_mut(&mut pandora);
    pandora.bind_threads(ipc.clone(), outputs.clone(), niri).start();
}