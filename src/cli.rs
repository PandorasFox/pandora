use std::process;

use ::pandora::pithos::commands::{CommandType, DaemonCommand, StopCommand, RenderThreadCommand};
use clap::{arg, Command, ArgMatches};

use crate::threads::logger::LogLevel;

// TODO refactor to derive <3
// i have seen the light

fn interface() -> Command {
     return Command::new("pandora")
    .about("a parallax wallpaper-and-lockscreen daemon ")
    .subcommand(Command::new("stop-daemon")
        .about("tells the running daemon to stop, if it exists")
    )
    // thread commands
    .subcommand(Command::new("stop-thread")
        .about("stops a wallpaper render thread by output name")
        .arg(arg!(<output> "output to stop wallpaper on"))
        .arg_required_else_help(true)
    )
    .subcommand(Command::new("lock")
        .about("instructs the running daemon to lock the session")
    )
}

pub fn cli() -> LogLevel {
    let matches = interface()
        .get_matches();

    if let Some(cmd) = match matches.subcommand() {
        Some(("stop-daemon", _)) => Some(CommandType::Dc(DaemonCommand::Stop)),
        Some(("stop-thread", sub_matches)) => {
            let output = extract_str(sub_matches, "output", "output name is required");
            Some(CommandType::Tc(RenderThreadCommand::Stop(StopCommand {
                output: output,
            })))
        },
        Some(("lock", _)) => Some(CommandType::Dc(DaemonCommand::Lock)),
        _ => None,
    } {
        println!("{}", ::pandora::pithos::sockets::write_command_to_daemon_socket(&cmd).expect("could not send command (is the daemon running?)"));
        process::exit(0);
    }

    return LogLevel::DEFAULT;
}

fn extract_str(matches: &ArgMatches, key: &str, msg: &str) -> String {
    return matches.get_one::<String>(key).expect(msg).to_owned();
}
