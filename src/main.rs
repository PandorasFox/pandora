mod pandora;
mod ipc;
mod render;
mod agents;
use ::pandora::config::load_config;
use std::sync::Arc;

fn main() -> miette::Result<()> {
    // TODO factor cli::main into here, do IPC if any args
    let config = load_config()?;
    // initialize daemon & ipc handlers, and glue them together.
    let mut pandora = crate::pandora::Pandora::new();
    let ipc = crate::ipc::InboundCommandHandler::new();
    let outputs = crate::agents::outputs::OutputHandler::new(config.clone());
    let niri = crate::agents::niri::NiriAgent::new(config.clone());
    // config_watcher thread next!

    Arc::make_mut(&mut pandora).bind_threads(
        ipc.clone(),
        outputs.clone(),
        niri.clone(),
    );
    // give the subthreads a weak pointer now that we're done mutating pandora into some sort of daemon
    let weak = Arc::downgrade(&pandora);
    pandora.start(weak);
    Ok(())
}