mod cli;
mod pandora;
mod threads;
use ::pandora::pithos::config::load_config;
use std::sync::Arc;

fn main() -> miette::Result<()> {
    cli::cli();
    let config = load_config()?;
    // initialize daemon & ipc handlers, and glue them together.
    let mut pandora = crate::pandora::Pandora::new();
    let ipc = crate::threads::ipc::InboundCommandHandler::new();
    let outputs = crate::threads::outputs::OutputHandler::new(config.clone());
    let niri = crate::threads::niri::NiriAgent::new(config.clone());
    let config_watcher = crate::threads::config::ConfigWatcher::new();

    Arc::make_mut(&mut pandora).bind_threads(
        ipc.clone(),
        outputs.clone(),
        niri.clone(),
        config_watcher.clone(),
    );
    // give the subthreads a weak pointer now that we're done mutating pandora into some sort of daemon
    let weak = Arc::downgrade(&pandora);
    pandora.start(weak);
    Ok(())
}