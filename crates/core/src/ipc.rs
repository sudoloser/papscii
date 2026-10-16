//! Tiny length-prefixed bincode protocol over a unix socket, shared by the
//! `papscii` CLI, the GUI, and the daemon.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    Ping,
    Stop,
    Next,
    Query,
    Pause,
    Resume,
    TogglePause,
    Reload,
    /// Re-read the settings file (focus-exempt apps) without restarting.
    ReloadSettings,
    /// Switch the displayed wallpaper to a `.pscii` path (file is loaded fresh).
    SetWallpaper {
        path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Response {
    Ok,
    Pong,
    State {
        wallpaper: Option<String>,
        frame: u32,
        frames: u32,
        paused: bool,
    },
    Error(String),
}

fn bincode_err(e: bincode::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

pub fn write_msg<T: Serialize>(stream: &mut UnixStream, msg: &T) -> io::Result<()> {
    let bytes = bincode::serialize(msg).map_err(bincode_err)?;
    stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}

pub fn read_msg<T: DeserializeOwned>(stream: &mut UnixStream) -> io::Result<T> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message too large",
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    bincode::deserialize(&buf).map_err(bincode_err)
}

/// One-shot helper: connect to `socket`, send `req`, read the reply.
pub fn request_at(socket: &std::path::Path, req: &Request) -> io::Result<Response> {
    let mut stream = UnixStream::connect(socket)?;
    write_msg(&mut stream, req)?;
    read_msg(&mut stream)
}
