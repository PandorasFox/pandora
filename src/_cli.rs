use ::pandora::commands::{CommandType, DaemonCommand, LoadImageCommand, StopCommand, RenderThreadCommand};

// TODO factor the still relevant bits of this back into main binary

use clap::{arg, Command, ArgMatches};
fn cli() -> Command {
    Command::new("pandora")
    .about("a parallax wallpaper-and-lockscreen daemon ")
    .subcommand_required(true)
    .subcommand(Command::new("stop-daemon")
        .about("TODO")
    )
    .subcommand(Command::new("loadimg")
        .about("TODO")
        .arg(arg!(<image> "path of the image to preload into memory"))
        .arg_required_else_help(true)
    )
    // thread commands
    .subcommand(Command::new("stop")
        .about("TODO")
        .arg(arg!(<output> "the output name for the render thread to stop"))
        .arg_required_else_help(true)
    )
}

fn extract_str(matches: &ArgMatches, key: &str, msg: &str) -> String {
    return matches.get_one::<String>(key).expect(msg).to_owned();
}

fn _main() {
    let matches = cli()
        .get_matches();

    let cmd: Option<CommandType>;
    // handling human input is always so much more annoying than computer input :(
    match matches.subcommand() {
        // daemon-level commands first
        Some(("loadimg", sub_matches)) => {
            let image = extract_str(sub_matches,"image", "image path is required");
            cmd = Some(CommandType::Dc(DaemonCommand::LoadImage(
                LoadImageCommand { image: image })));
        }
        Some(("stop-daemon", _)) => {
            cmd = Some(CommandType::Dc(DaemonCommand::Stop));
        }

        // thread-level commands
        Some(("stop", sub_matches)) => {
            let output = extract_str(sub_matches, "output", "output name is required");
            cmd = Some(CommandType::Tc(RenderThreadCommand::Stop(StopCommand {
                output: output,
            })));
        }

        _ => unreachable!(),
    }
    let command = cmd.expect("clap should prevent this in a prettier way?");
    println!("{}", pithos::sockets::write_command_to_daemon_socket(&command).expect("could not send command (is the daemon running?)"));
}
