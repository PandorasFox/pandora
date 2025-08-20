mod cli;
mod pandora;
use ::pandora::pithos::config::load_config;
use std::sync::Arc;

fn main() -> miette::Result<()> {
    let cli_verbosity = cli::cli();
    let config = load_config()?;

    let verbosity = match cli_verbosity {
        Some(level) => level,
        None => config.log_level,
    };

    let pandora = crate::pandora::Pandora::new(config.clone(), verbosity);
    let weak = Arc::downgrade(&pandora);
    pandora.start(weak, config.clone())
}
