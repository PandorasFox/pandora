use crate::daemon::Daemon;

use std::os::linux::net::SocketAddrExt;
use std::os::unix::net::{SocketAddr, UnixListener};
use std::sync::{Arc, Weak};
use std::thread;

#[derive(Clone)]
pub struct InboundCommandHandler {
    listener: Arc<UnixListener>,
}

impl InboundCommandHandler {
    // roll these into pandora proper?
    pub fn new() -> Arc<InboundCommandHandler> {
        let listen_addr = SocketAddr::from_abstract_name("pandora")
            .expect("could not construct linux named-socket address (sorry bsd?)");
        let socket = UnixListener::bind_addr(&listen_addr)
            .expect("failed to bind to named socket (already running?)");

        return Arc::new(InboundCommandHandler {
            listener: Arc::new(socket),
        });
    }

    pub fn start(&self, pandora: Weak<dyn Daemon + Send + Sync>) {
        for connection in self.listener.incoming() {
            let p = pandora.upgrade().take().unwrap();
            thread::spawn(move || {
                let socket = connection.unwrap();
                let cmd = crate::pithos::sockets::read_command_from_client_socket(&socket);
                p.handle_cmd(&cmd);
                crate::pithos::sockets::write_response_to_client_socket(
                    "command dispatched",
                    &socket,
                )
                .expect("failed to write response to inbound ipc");
            });
        }
    }
}
