use ::pandora::pithos::config::LogLevel;
use ::pandora::pithos::commands::{CommandType, DaemonCommand, StopCommand, RenderThreadCommand};
use clap::{arg, Parser};
use std::process;

#[derive(Parser)]
#[command(name = "pandora")]
#[command(about = "a parallax wallpaper and lockscreen daemon for Wayland")]
#[command(version)]
struct Interface {
    #[arg(long="log-level")]
    log_level: Option<LogLevel>,
    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Clone, clap::Subcommand)]
enum CliCommand {
    StopDaemon,
    StopThread(StopCommand),
    Lock,
}

pub fn cli() -> Option<LogLevel> { // the only config pass-able to the daemon via cli
    let cli = Interface::parse();
    if let Some(command) = cli.command {
        let cmd = match command {
            CliCommand::StopDaemon => CommandType::Dc(DaemonCommand::Stop),
            CliCommand::StopThread(c) => CommandType::Tc(RenderThreadCommand::Stop(c)),
            CliCommand::Lock => CommandType::Dc(DaemonCommand::Lock),
        };
        println!("{}", ::pandora::pithos::sockets::write_command_to_daemon_socket(&cmd).expect("could not send command (is the daemon running?)"));
        process::exit(0);
    }
    return cli.log_level;
}