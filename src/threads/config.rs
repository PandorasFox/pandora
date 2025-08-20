use crate::daemon::Daemon;
use crate::pithos::config::{get_config_dir, load_config};

use std::path::PathBuf;
use std::sync::{Arc, Weak, mpsc};
use std::thread;

use notify::{Event, RecursiveMode, Result, Watcher, event::EventKind, event::ModifyKind};

#[derive(Clone)]
pub struct ConfigWatcher {}

impl ConfigWatcher {
    pub fn new() -> Arc<ConfigWatcher> {
        return Arc::new(ConfigWatcher {});
    }

    pub fn start(&self, weak: Weak<dyn Daemon + Send + Sync>) {
        let p = weak.upgrade().take().unwrap();
        thread::spawn(move || watch(&get_config_dir(), p));
    }
}

fn process_event(e: &Event) -> bool {
    // return false if we dgaf about this event
    if !e.paths.contains(&get_config_dir().join("pandora.kdl")) {
        return false;
    }
    match e.kind {
        EventKind::Create(_) => return true,
        EventKind::Modify(modkind) => {
            match modkind {
                ModifyKind::Data(_) => return true,
                ModifyKind::Name(_) => return true, // vim-type tmp file -> rename clobber, probably
                _ => return false,
            }
        }
        _ => return false,
    }
}

fn watch(path: &PathBuf, pandora: Arc<dyn Daemon>) {
    let (tx, rx) = mpsc::channel::<Result<Event>>();
    let mut watcher =
        notify::recommended_watcher(tx).expect("Could not create a watcher for config dir");
    watcher
        .watch(path, RecursiveMode::Recursive)
        .expect("Could not start watcher for config dir");
    for res in rx {
        let p = pandora.clone();
        match res {
            Ok(e) => {
                if !process_event(&e) {
                    continue;
                };

                match load_config() {
                    Ok(conf) => {
                        p.handle_cmd(&crate::pithos::commands::CommandType::Dc(
                            crate::pithos::commands::DaemonCommand::ReloadConfig(conf),
                        ));
                    }
                    Err(e) => p.log("config-watcher", format!("{e:?}")),
                };
            }
            Err(e) => p.log("config-watcher", format!("watch error: {e:?}")),
        };
    }
}
