use std::sync::Arc;

use pithos::config::load_config;

use crate::agents::outputs::OutputHandler;

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
    let outputs = OutputHandler::new(config, pandora.clone());
    
    let pandora = Arc::make_mut(&mut pandora);
    pandora.bind_threads(ipc.clone(), outputs.clone());
    // yeehaw
    pandora.start()
}