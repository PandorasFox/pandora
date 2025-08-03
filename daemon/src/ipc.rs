use std::sync::Arc;
use std::thread;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener};

use crate::pandora::Pandora;

#[derive(Clone)]
pub struct IpcHandler {
    listener: Arc<UnixListener>,
    pandora: Arc<Pandora>,
}

impl IpcHandler {
    pub fn new(pandora: Arc<Pandora>) -> Arc<IpcHandler> {
        let listen_addr = SocketAddr::from_abstract_name("pandora").expect(
            "could not construct linux named-socket address (sorry bsd?)");
        let socket = UnixListener::bind_addr(&listen_addr).expect(
            "failed to bind to named socket (already running?)");

        return Arc::new(IpcHandler {
            listener: Arc::new(socket),
            pandora: pandora,
        });
    }

    pub fn start_listen(&self) {
        for connection in self.listener.incoming() {
            let pandora = self.pandora.as_ref().clone();
            thread::spawn(move || 
                pandora.process_ipc(&connection.expect(
                    "could not accept incoming client socket/connection")));
        }
    }
}