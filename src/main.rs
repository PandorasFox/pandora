mod cli;
mod pandora;
mod threads;
use ::pandora::pithos::config::load_config;
use std::sync::Arc;

fn main() -> miette::Result<()> {
    let cli_verbosity = cli::cli();
    let config = load_config()?;

    let verbosity = match cli_verbosity {
        Some(level) => level,
        None => config.log_level,
    };

    // initialize daemon & ipc handlers, and glue them together.
    let mut pandora = crate::pandora::Pandora::new(config.clone(), verbosity);
    // we initialize pandora mutably so that logging can be started
    //  => other threads have The Logging Abstraction available for the entirety of their runtime
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
    Ok(pandora.start(Arc::downgrade(&pandora)))
}