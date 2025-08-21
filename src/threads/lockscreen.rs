use crate::pithos::config::{DaemonConfig, LogLevel};
use std::{
    sync::{Arc, mpsc::Sender},
    thread,
};

pub fn lock(log: Arc<Sender<(LogLevel, String)>>, config: DaemonConfig) {
    // the one public interface
    Cerberus::start(log, config);
}

struct Cerberus {
    _logger: Arc<Sender<(LogLevel, String)>>,
    _config: DaemonConfig,
    // more. moar.
}

impl Cerberus {
    /* state: AgentState - drop the config, just ask agent for InitialLockscreenState? */
    /* maybe LockScreenConfig from agent. idk. we can replumb that when it's time. */
    fn new(log: Arc<Sender<(LogLevel, String)>>, config: DaemonConfig) -> Cerberus {
        Cerberus {
            _logger: log,
            _config: config,
        }
    }

    fn start(log: Arc<Sender<(LogLevel, String)>>, config: DaemonConfig) {
        thread::spawn(|| {
            // set up lockscreen thread initial stuff. Arc<>s them. hands them to Cerberus once session is locked
            // cerberus then run()s and spawns the lockscreen/etc threads
            Cerberus::new(log, config).run();
        });
    }

    fn run(&self) {
        // these five easy steps, theoretically
        //self.init_wayland_connection(); // should return (mut conn, globals?)
        //self.acquire_lock();
        //self.listen_for_inputs();
        //self.raise_lockscreen();
        //self.listen_for_unlock()
    }
    /*
    get wayland globals:
        connection, compositor, shm, outputs, viewport. create globals struct, no optionals, just arcs.
        bind ext_session_lock_manager_v1 => acquire session lock - lives in-thread
        spawn render threads per output; their state lives in the vec<(output_state, wloutput)> hamburger. they only get connection<state> and path (evaluate cloning this ? it's mut ? fuck)
        primary lock thread continues on to input parsing, sending to.... oh god, the channels.

    what if we do this all in one thread? what if we just process input, then dispatch events for frame callbacks?
    => nothing can block ever

    so. we need to *also* put the receivers in the output state, and arc them, on output::done during the initial callback hell.

    we also need to make sure that we *cleanly* handle the callbacks for mode changes. We'll want to send that
    into its own callback, which can evaluate the proper actions to take - i believe we will want to destroy the active surface,
    and create a new lock_surface

    ... so we need a way to ask the input threa-.
    ..... so we need a main thread that holds the input thread, the ext_session_lock, and the output render threads.
    ... sounds familiar.
    ..... (clutches my head)
    .... so. the main thread will grab everything that's needed. it will hold the connection, the globals, etc.
    ...  it will acquire the lock, _and then_ spawn the thread that handles input, and the lockscreen thread.
    ... the lockscreen thread will be responsible for managing the surfaces attached to a given WlOutput.
    .... it will manage the state (attached image, buffer, which lockscreen(s) gets the indicators) with its config blob
    => how do we get the current lockscreen viewport state for ourselves?
    ==> we should be able to fetch the offsets from the agent at lock time
    ===> niri agent needs a refactor to have a .getState {output: (image, mode, offset)} (and an updateState.... yeah....)
    ...  it will dispatch events for frame draws
    ...  it will read its event queue from the input thread
    ...  LastAction Instance::now whenever an event from event queue => reset state
    ...  it will use subsurfaces to separate wallpaper bottom layer from (cairo?) canvas upper layer
    ...  it will have appropriate handling of when the callback necessitates surface recreation (and wloutput object remapping in the vec as well i presume, oh god)
    ...  overall actually sounds quite pleasant
    ... the main thread will wait for the AUTH_FINISH response from lock thread
    ... IMMEDIATELY unlock and destroy() the lock. "OBJECTS CREATED ARE STILL VALID"
    ... upon which it passes that on to the lockscreen thread (exits gracefully) => up the surfaces and releases them all => we rejoice
    ... because the underlying compositor block should be gone, we can maybe opacity lighten? hmmm? eyewiggles?
    ... OPACITY LIGHTEN AND ZOOM IN MAYBE?? :eyes:

    okay yeah that's enough thinking on that

    methods accessible on the 'handler' (cerberus) should be defined by trait so pandora (hecate ? ) can reuse


    and im gonna write it all by hand because this is a security sensitive application and i wish to take it slowly and carefully!
    (so i can reuse the well written independent functions for the normal render threads/architecture)
    hggggh
    */
    fn _log(&self, msg: String) {
        self._do_log(LogLevel::DEFAULT, format!("[lockscreen] {msg}"));
    }
    fn _debug(&self, msg: String) {
        self._do_log(LogLevel::DEBUG, format!("[lockscreen] {msg}"));
    }
    fn _verbose(&self, msg: String) {
        self._do_log(LogLevel::VERBOSE, format!("[lockscreen] {msg}"));
    }
    fn _do_log(&self, level: LogLevel, msg: String) {
        let _ = self._logger.send((level, msg));
    }
}
