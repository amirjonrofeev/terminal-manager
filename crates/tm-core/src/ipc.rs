//! Local socket transport (named pipe on Windows, Unix socket elsewhere) with length-prefixed
//! postcard frames.

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Name};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{self, Read, Write};

pub use interprocess::local_socket::{Listener, Stream};

/// Refuse frames larger than this (a replay is at most a few MB).
const MAX_FRAME: usize = 64 * 1024 * 1024;

fn name(socket: &str) -> io::Result<Name<'_>> {
    if cfg!(windows) { socket.to_ns_name::<GenericNamespaced>() } else { socket.to_fs_name::<GenericFilePath>() }
}

pub fn connect(socket: &str) -> io::Result<Stream> {
    Stream::connect(name(socket)?)
}

/// Listens on `socket`. On Windows `sddl` sets who may connect to the pipe (an SDDL security
/// descriptor); on Unix a stale socket file is replaced and the socket is private to its owner.
pub fn listen(socket: &str, #[allow(unused_variables)] sddl: Option<&str>) -> io::Result<Listener> {
    let mut opts = ListenerOptions::new().name(name(socket)?);
    #[cfg(unix)]
    {
        use interprocess::os::unix::local_socket::ListenerOptionsExt;
        if let Some(dir) = std::path::Path::new(socket).parent() {
            std::fs::create_dir_all(dir)?;
        }
        let _ = std::fs::remove_file(socket);
        opts = opts.mode(0o600);
    }
    #[cfg(windows)]
    {
        use interprocess::os::windows::local_socket::ListenerOptionsExt;
        use interprocess::os::windows::security_descriptor::SecurityDescriptor;
        opts = opts.reclaim_name(false);
        if let Some(sddl) = sddl {
            let wide = widestring::U16CString::from_str(sddl).map_err(io::Error::other)?;
            opts = opts.security_descriptor(SecurityDescriptor::deserialize(&wide)?);
        }
    }
    opts.create_sync()
}

pub fn write_frame<W: Write, T: Serialize>(w: &mut W, msg: &T) -> io::Result<()> {
    let body = postcard::to_stdvec(msg).map_err(io::Error::other)?;
    let len = u32::try_from(body.len()).map_err(|_| io::Error::other("frame too large"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(&body)?;
    w.flush()
}

pub fn read_frame<R: Read, T: DeserializeOwned>(r: &mut R) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes is too large")));
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body)?;
    postcard::from_bytes(&body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Project, TerminalSpec, Workspace};
    use crate::protocol::{ClientMsg, HostMsg, Status, TerminalState};

    #[test]
    fn frames_round_trip() {
        let mut ws = Workspace::default();
        ws.projects.push(Project { name: "p".into(), terminals: vec![TerminalSpec::default()], ..Default::default() });
        let msgs = vec![
            HostMsg::Workspace(ws),
            HostMsg::State(TerminalState { status: Status::Exited(Some(3)), ..TerminalState::new(uuid::Uuid::new_v4()) }),
            HostMsg::Output { terminal: uuid::Uuid::new_v4(), bytes: b"\x1b[31mhi\r\n".to_vec() },
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write_frame(&mut buf, m).unwrap();
        }
        write_frame(&mut buf, &ClientMsg::Hello { version: 1 }).unwrap();
        let mut r = &buf[..];
        for m in &msgs {
            assert_eq!(&read_frame::<_, HostMsg>(&mut r).unwrap(), m);
        }
        assert_eq!(read_frame::<_, ClientMsg>(&mut r).unwrap(), ClientMsg::Hello { version: 1 });
        assert!(read_frame::<_, ClientMsg>(&mut r).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn pipe_acl_is_enforced() {
        let open = format!("tm-acl-open-{}", uuid::Uuid::new_v4());
        let _l = listen(&open, Some("D:P(A;;GA;;;SY)(A;;GA;;;OW)")).unwrap();
        assert!(connect(&open).is_ok(), "the owner may connect");

        let system_only = format!("tm-acl-closed-{}", uuid::Uuid::new_v4());
        let _l = listen(&system_only, Some("D:P(A;;GA;;;SY)")).unwrap();
        let err = connect(&system_only).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied, "{err}");
    }

    #[test]
    fn oversized_frame_is_rejected() {
        let mut r = &(u32::MAX).to_le_bytes()[..];
        assert!(read_frame::<_, ClientMsg>(&mut r).is_err());
    }
}
