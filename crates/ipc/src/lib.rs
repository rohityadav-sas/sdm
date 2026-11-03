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

use std::path::PathBuf;

/// Where SDM keeps its settings, history and `ipc.json`.
/// `SDM_DATA_DIR` overrides the default (used by tests).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("SDM_DATA_DIR") {
        return PathBuf::from(d);
    }
    let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
    let (new, old) = (base.join("sdm"), base.join("velo"));
    // The app used to be called Velo: carry its settings and history over once.
    if !new.exists() && old.exists() {
        let _ = std::fs::rename(&old, &new);
    }
    new
}

pub use protocol::{Reply, Request};
pub use server::{Handler, Server};
