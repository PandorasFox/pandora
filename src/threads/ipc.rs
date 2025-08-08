use std::sync::{Arc, Weak};
use std::thread;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener};

use crate::pandora::Pandora;

#[derive(Clone)]
pub struct InboundCommandHandler {
    listener: Arc<UnixListener>,
}

impl InboundCommandHandler {
    pub fn new() -> Arc<InboundCommandHandler> {
        let listen_addr = SocketAddr::from_abstract_name("pandora").expect(
            "could not construct linux named-socket address (sorry bsd?)");
        let socket = UnixListener::bind_addr(&listen_addr).expect(
            "failed to bind to named socket (already running?)");

        return Arc::new(InboundCommandHandler {
            listener: Arc::new(socket),
        });
    }

    pub fn start(&self, pandora: Weak<Pandora>) {
        for connection in self.listener.incoming() {
            let p = pandora.upgrade().take().unwrap();
            thread::spawn(move || 
                p.process_ipc(&connection.expect(
                    "could not accept incoming client socket/connection")));
        }
    }
}