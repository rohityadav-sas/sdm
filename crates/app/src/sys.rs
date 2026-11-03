//! Small OS integrations: open files/folders and desktop notifications.
use std::path::Path;
use std::process::{Command, Stdio};

pub fn open(path: &Path) {
    #[cfg(target_os = "linux")]
    let mut c = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut c = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut c = Command::new("explorer");
    c.arg(path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    let _ = c.spawn();
}

pub fn notify(title: &str, body: &str) {
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("notify-send")
            .args(["--app-name=SDM", "--icon=folder-download", title, body])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let script = format!("display notification {:?} with title {:?}", body, title);
        let _ = Command::new("osascript").args(["-e", &script]).spawn();
    }
    #[cfg(target_os = "windows")]
    {
        let _ = (title, body);
    }
}
