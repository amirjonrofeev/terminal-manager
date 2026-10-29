//! Accepts client connections: a reader thread handles requests, a writer thread drains the
//! client's outgoing queue so a slow viewer never blocks a terminal.

use crate::host::Host;
use std::io;
use std::sync::Arc;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;
use tm_core::ipc::{self, Listener, Stream};
use tm_core::protocol::{ClientMsg, HostMsg, PROTOCOL_VERSION};

/// Outgoing messages queued per client before it counts as lagging.
const CLIENT_QUEUE: usize = 4096;

pub fn serve(host: Arc<Host>, listener: Listener) {
    use interprocess::local_socket::traits::ListenerExt as _;
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let host = host.clone();
                let _ = std::thread::Builder::new().name("client".into()).spawn(move || {
                    if let Err(e) = handle_client(&host, stream) {
                        log::debug!("client connection ended: {e}");
                    }
                });
            }
            Err(e) => log::warn!("accepting a connection failed: {e}"),
        }
    }
}

fn handle_client(host: &Arc<Host>, stream: Stream) -> io::Result<()> {
    let stream = Arc::new(stream);
    let mut reader = &*stream;
    match ipc::read_frame::<_, ClientMsg>(&mut reader)? {
        ClientMsg::Hello { version } if version == PROTOCOL_VERSION => {}
        ClientMsg::Hello { version } => {
            let msg = format!(
                "This Terminal Manager (protocol {version}) does not match the running host (protocol {PROTOCOL_VERSION}). Update both to the same version."
            );
            let _ = ipc::write_frame(&mut &*stream, &HostMsg::Error(msg));
            return Ok(());
        }
        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "expected Hello")),
    }

    let (tx, rx) = mpsc::sync_channel::<HostMsg>(CLIENT_QUEUE);
    let client = host.add_client(tx);

    let writer_stream = stream.clone();
    let writer_host = host.clone();
    let writer_client = client.clone();
    let writer = std::thread::Builder::new().name("client-writer".into()).spawn(move || {
        let mut w = &*writer_stream;
        loop {
            match rx.recv_timeout(Duration::from_millis(250)) {
                Ok(msg) => {
                    if ipc::write_frame(&mut w, &msg).is_err() {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            writer_host.resync(&writer_client);
        }
    })?;

    let result = loop {
        match ipc::read_frame::<_, ClientMsg>(&mut reader) {
            Ok(msg) => host.handle(&client, msg),
            Err(e) => break e,
        }
    };
    host.remove_client(client.id);
    drop(client);
    let _ = writer.join();
    Err(result)
}
