//! Local control channel between the SDM app, the browser's native-messaging
//! host (`sdm-host`) and the command line.
//!
//! The app listens on 127.0.0.1 (random port) and publishes `ipc.json`
//! (port + secret token, readable only by the user) in its data directory.
//! Clients read that file, connect, send one JSON line and read one JSON line.

pub mod client;
pub mod install;
pub mod protocol;
pub mod server;

use std::io::Write;
use std::path::{Path, PathBuf};

/// Where SDM keeps its settings, history and `ipc.json`.
/// `SDM_DATA_DIR` overrides the default (used by tests).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("SDM_DATA_DIR") {
        return PathBuf::from(d);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("sdm")
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = tmp_path(path);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

#[cfg(unix)]
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = tmp_path(path);
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    drop(f);
    std::fs::rename(tmp, path)
}

#[cfg(not(unix))]
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic(path, bytes)
}

pub use protocol::{Reply, Request};
pub use server::{Handler, Server};
