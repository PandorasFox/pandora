use pithos::commands::{CommandType, ConfigReloadCommand, DaemonCommand, InfoCommand, LoadImageCommand, RenderCommand, RenderMode, ScrollCommand, StopCommand, ThreadCommand};

use clap::{arg, Command, ArgMatches};

fn cli() -> Command {
    // todo: clean this up into a more modular builder-class when this gets refactored back into the main binary
    Command::new("pandora")
    .about("a panning/scrolling wallpaper daemon (and its cli, TBD)")
    .subcommand_required(true)
    .subcommand(Command::new("info")
        .about("should return some info about the current state")
        .arg(arg!(-v --verbose "set verbose=true"))
    )
     .subcommand(Command::new("reloadcfg")
        .about("make the daemon live-reload the config file - this might be removed?")
        .arg(arg!([path] "path to the config file to (re)load"))
    )
    .subcommand(Command::new("loadimg")
        .about("TODO")
        .arg(arg!(<image> "path of the image to preload into memory"))
        .arg_required_else_help(true)
    )
    // thread commands
    .subcommand(Command::new("render")
        .about("If <output> does not have an existing render thread, one will be started. Switches the relevant render thread to the given mode and image.")
        .arg(arg!(<output> "the output name to start a render thread for"))
        .arg(arg!(<image> "path of the image to use for wallpaper canvas. should be loaded with LoadImage first"))
        .arg(arg!(<mode> "render mode to use for the given image (static, scroll_vertical, scroll_lateral)")
                .value_parser(["static", "scroll_vertical", "scroll_lateral"])
            )
        .arg(arg!([position] "initial scroll position when mode is a scrolling mode").value_parser(clap::value_parser!(u32)))
//        .arg(arg!([end] "initial end scroll value when mode is a scroll mode").value_parser(clap::value_parser!(i32)))
        .arg_required_else_help(true)
    )
    
    .subcommand(Command::new("stop")
        .about("TODO")
        .arg(arg!(<output> "the output name for the render thread to stop"))
        .arg_required_else_help(true)
    )
    .subcommand(Command::new("scroll")
        .about("set the scrolling position of the given output along the configured dimension")
        .arg(arg!(<output> "output name of the canvas to scroll"))
        .arg(arg!(<position> "scroll position in pixels").value_parser(clap::value_parser!(u32)))
//        .arg(arg!(<end> "end of the scroll position").value_parser(clap::value_parser!(i32)))
    )
}

fn extract_str(matches: &ArgMatches, key: &str, msg: &str) -> String {
    return matches.get_one::<String>(key).expect(msg).to_owned();
}

fn extract_int(matches: &ArgMatches, key: &str, msg: &str) -> u32 {
    return matches.get_one::<u32>(key).expect(msg).to_owned();
}

/*
fn is_verbose() -> bool {
    // TODO lol
    return false;
}
*/

fn main() {
    let matches = cli()
        .get_matches();

    let cmd: Option<CommandType>;
    // handling human input is always so much more annoying than computer input :(
    match matches.subcommand() {

        // daemon-level commands first
        Some(("info", sub_matches)) => {
            // TODO: refactor verbose arg to a global arg, default false, always a bool? seems easier maybe
            let info: InfoCommand;
            if let Some(verbose) = sub_matches.get_one::<bool>("verbose") {
                info = InfoCommand { verbose: verbose.to_owned() };
            } else {
                info = InfoCommand { verbose: false };
            }
            cmd = Some(CommandType::Dc(DaemonCommand::Info(info)))
        }
        Some(("loadimg", sub_matches)) => {
            let image = extract_str(sub_matches,"image", "image path is required");
            cmd = Some(CommandType::Dc(DaemonCommand::LoadImage(
                LoadImageCommand { image: image })));
        }
        Some(("reloadcfg", sub_matches)) => {
            let path = extract_str(sub_matches, "path", "config file path is required");
            cmd = Some(CommandType::Dc(DaemonCommand::ConfigReload(
                ConfigReloadCommand { file: path })));
        }

        // thread-level commands
        Some(("render", sub_matches)) => {
            let output = extract_str(sub_matches, "output", "output name is required");
            let image = extract_str(sub_matches, "image", "image name is required");
            let mode = match extract_str(sub_matches, "mode", "render mode is required").as_str() {
                "static" => RenderMode::Static,
                "scroll_vertical" => {
                    let position = extract_int(sub_matches, "position", "scroll value should not be empty");
                    //let end = extract_int(sub_matches, "end", "end scroll value should not be empty");
                    RenderMode::ScrollingVertical(position)
                }
                "scroll_lateral" => {
                    let position = extract_int(sub_matches, "position", "scroll value should not be empty");
                    //let end = extract_int(sub_matches, "end", "end scroll value should not be empty");
                    RenderMode::ScrollingLateral(position)
                },
                _ => unreachable!(),
            };
            cmd = Some(CommandType::Tc(ThreadCommand::Render(RenderCommand {
                    output: output,
                    image: image,
                    mode: mode,
                }))); // im so glad my editor colorcodes each parens differently
        }
        Some(("stop", sub_matches)) => {
            let output = extract_str(sub_matches, "output", "output name is required");
            cmd = Some(CommandType::Tc(ThreadCommand::Stop(StopCommand {
                output: output,
            })));
        }
        Some(("scroll", sub_matches)) => {
            let output = extract_str(sub_matches, "output", "output name is required");
            let position = extract_int(sub_matches, "position", "scroll value should not be empty");
            //let end = extract_int(sub_matches, "end", "end scroll value should not be empty");
            //assert!(start < end);

            cmd = Some(CommandType::Tc(ThreadCommand::Scroll(
                ScrollCommand {
                    output: output,
                    position: position,
            })));
        }

        _ => unreachable!(),
    }
    let command = cmd.expect("clap should prevent this in a prettier way?");
    println!("{}", pithos::sockets::write_command_to_daemon_socket(command).expect("could not send command (is the daemon running?)"));
}
