use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub download_folder: String,
    pub connections: i32,
    pub max_active: i32,
    pub speed_limit: i32,
    /// 0 = follow the system, 1 = light, 2 = dark
    pub theme: i32,
    pub clipboard: bool,
    pub notifications: bool,
    /// Keep downloading after the window is closed.
    pub keep_running: bool,
    /// Use the regular window frame instead of SDM's own header bar (applies after restart).
    pub system_titlebar: bool,
    /// Last window size in logical pixels (0 = default).
    pub window_width: i32,
    pub window_height: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_folder: dirs::download_dir().unwrap_or_else(|| PathBuf::from(".")).to_string_lossy().into_owned(),
            connections: 16,
            max_active: 4,
            speed_limit: 0,
            theme: 0,
            clipboard: true,
            notifications: true,
            keep_running: true,
            system_titlebar: false,
            window_width: 0,
            window_height: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Saved {
    pub id: i32,
    pub url: String,
    pub name: String,
    pub dir: String,
    pub path: Option<String>,
    pub total: u64,
    pub done: u64,
    pub complete: bool,
    pub connections: i32,
    pub finished: Option<u64>,
    /// Cookies / referer captured by the browser extension (needed to resume).
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

fn read<T: for<'a> Deserialize<'a>>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_atomic(path: &Path, bytes: &[u8]) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, bytes).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

pub fn load_settings(dir: &Path) -> Settings {
    read(&dir.join("settings.json")).unwrap_or_default()
}
pub fn save_settings(dir: &Path, s: &Settings) {
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        write_atomic(&dir.join("settings.json"), &b);
    }
}
pub fn load_history(dir: &Path) -> Vec<Saved> {
    read(&dir.join("downloads.json")).unwrap_or_default()
}
pub fn save_history(dir: &Path, items: &[Saved]) {
    if let Ok(b) = serde_json::to_vec(items) {
        // May contain session cookies: readable by the user only.
        write_private(&dir.join("downloads.json"), &b);
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tmp = path.with_extension("tmp");
    let ok = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .and_then(|mut f| f.write_all(bytes))
        .is_ok();
    if ok {
        let _ = std::fs::rename(&tmp, path);
    }
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) {
    write_atomic(path, bytes);
}
