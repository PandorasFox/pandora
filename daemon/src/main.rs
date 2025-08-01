use std::sync::Arc;

mod pandora;
mod ipc;
mod render;
mod wl_session;

fn main() {
    // initialize daemon & ipc handler, and glue them together.
    // TODO: maybe also initialize single wayland Conn here?
    // conceptually i like each render thread having their own conn
    // but it makes ownership.... weird.... since Connection<> can't be copied/cloned
    let mut pandora = crate::pandora::Pandora::new();
    let ipc = crate::ipc::IpcHandler::new(pandora.clone());
    Arc::make_mut(&mut pandora).bind_ipc(ipc.clone());
    pandora.start()
}