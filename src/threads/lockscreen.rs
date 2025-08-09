use pandora::pithos::config::{DaemonConfig, LogLevel};
use std::{sync::{mpsc::Sender, Arc}, thread};

pub fn lock(log: Arc<Sender<(LogLevel, String)>>, config: DaemonConfig) { // the one public interface
    LockscreenThread::start(log, config);
}

struct LockscreenThread {
    logger: Arc<Sender<(LogLevel, String)>>,
    config: DaemonConfig,
}

impl LockscreenThread {
    fn new(log: Arc<Sender<(LogLevel, String)>>, config: DaemonConfig, ) -> LockscreenThread {
        return LockscreenThread {
            logger: log,
            config
        }
    }

    fn start(log: Arc<Sender<(LogLevel, String)>>, config: DaemonConfig) {
        thread::spawn(|| LockscreenThread::new(log, config).lock() );
    }

    fn lock(&mut self) {
        // get wayland handles, start connection, etc
        self.log("locking".to_string());
        self.debug("locking more".to_string());
        self.verbose("locking quite profusely".to_string());
    }

    fn _unlock(&mut self) {
    }

    fn log(&self, msg: String) {
        self._log(LogLevel::DEFAULT, format!("[lockscreen] {msg}"));
    }
    fn debug(&self, msg: String) {
        self._log(LogLevel::DEBUG, format!("[lockscreen] {msg}"));
    }
    fn verbose(&self, msg: String) {
        self._log(LogLevel::VERBOSE, format!("[lockscreen] {msg}"));
    }
    fn _log(&self, level: LogLevel, msg: String) {
        let _ = self.logger.send((level, msg));
    }
}