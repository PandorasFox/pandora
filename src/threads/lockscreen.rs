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
