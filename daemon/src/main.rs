use std::sync::Arc;

mod pandora;
mod ipc;
mod render;

fn main() {
    // initialize daemon & ipc handler, and glue them together.
    // TODO name all spawned threads & use that in logs
    let mut pandora = crate::pandora::Pandora::new();
    let ipc = crate::ipc::IpcHandler::new(pandora.clone());
    Arc::make_mut(&mut pandora).bind_ipc(ipc.clone());
    pandora.start()
}