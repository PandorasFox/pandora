use pithos::agents::niri;
fn main() {
    // stub for using pithis::agents::niri outside of the main daemon
    // mostly so that I can iterate on this without having to repeatedly reload the daemon during development
    // will need a flag or dep-inject to send commands over socket instead of to an in-process channel
    // this stub will be dropped.... soon.....
    let agent = niri::NiriAgent::new(None);
    if agent.is_ok() {
        agent.unwrap().start();
    } else {
        println!("> agent failed to spawn, continuing");
    }
}
