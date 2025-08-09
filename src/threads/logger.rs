use ::pandora::pithos::config::LogLevel;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

pub struct LogThread {
    pub inbox: Arc<Sender<(LogLevel, String)>>,
    spool: Arc<Mutex<Receiver<(LogLevel, String)>>>,
    level: LogLevel,
}

impl LogThread {
    pub fn log(&self, level: LogLevel, name: &str, msg: String) {
        let _ = self.inbox.send((level, format!("[{name}] {msg}")));
    }

    pub fn new(verbosity: LogLevel) -> Arc<LogThread> {
        let (send, recv) = channel::<(LogLevel, String)>();
        let logger = Arc::new(LogThread {
            inbox: Arc::new(send),
            spool: Arc::new(Mutex::new(recv)),
            level: verbosity,
        });
        logger.start();
        return logger;
    }

    pub fn start(&self) {
        let spool_guard = self.spool.clone();
        let threshold = self.level.clone();
        thread::spawn(move || {
            match spool_guard.lock() {
                Ok(spool) => LogThread::run(threshold, spool),
                Err(e) => println!("LOGGER: failed to start logger: {e:?}"),
            }
        });
    }

    fn run(threshold: LogLevel, spool: MutexGuard<Receiver<(LogLevel, String)>>) {
        loop {
            if let Ok((level, msg)) = spool.recv() {
                if threshold.check(&level) {
                    println!("{msg}");
                }
            }
        }
    }
}