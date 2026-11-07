//! Registers the native-messaging host with the browsers found on this machine
//! and unpacks the bundled browser extension.

use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};

pub const HOST_NAME: &str = "com.sdm.host";
/// Registered by the app under its old name (Velo); removed on setup.
const LEGACY_HOST_NAME: &str = "com.velo.host";
/// Fixed ID of the Chromium extension (derived from the key in its manifest).
pub const CHROMIUM_EXTENSION_ID: &str = env!("SDM_EXTENSION_ID");
pub const FIREFOX_EXTENSION_ID: &str = "sdm@local.sdm";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Chromium,
    Firefox,
}

#[derive(Clone, Debug)]
pub struct Browser {
    pub name: &'static str,
    pub kind: Kind,
    /// The browser's profile root: if it exists, the browser has been used here.
    pub detect: PathBuf,
    /// Where its native-messaging manifests live.
    pub manifests: PathBuf,
}

#[cfg(target_os = "linux")]
pub fn browsers() -> Vec<Browser> {
    let home = dirs::home_dir().unwrap_or_default();
    let cfg = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
    let chromium = |name, sub: &str| {
        let detect = cfg.join(sub);
        Browser { name, kind: Kind::Chromium, manifests: detect.join("NativeMessagingHosts"), detect }
    };
    vec![
        chromium("Google Chrome", "google-chrome"),
        chromium("Chromium", "chromium"),
        chromium("Brave", "BraveSoftware/Brave-Browser"),
        chromium("Microsoft Edge", "microsoft-edge"),
        chromium("Vivaldi", "vivaldi"),
        chromium("Opera", "opera"),
        Browser { name: "Firefox", kind: Kind::Firefox, detect: home.join(".mozilla"), manifests: home.join(".mozilla/native-messaging-hosts") },
    ]
}

#[cfg(target_os = "macos")]
pub fn browsers() -> Vec<Browser> {
    let base = dirs::home_dir().unwrap_or_default().join("Library/Application Support");
    let b = |name, sub: &str, kind| {
        let detect = base.join(sub);
        Browser { name, kind, manifests: detect.join("NativeMessagingHosts"), detect }
    };
    vec![
        b("Google Chrome", "Google/Chrome", Kind::Chromium),
        b("Chromium", "Chromium", Kind::Chromium),
        b("Brave", "BraveSoftware/Brave-Browser", Kind::Chromium),
        b("Microsoft Edge", "Microsoft Edge", Kind::Chromium),
        b("Vivaldi", "Vivaldi", Kind::Chromium),
        b("Firefox", "Mozilla", Kind::Firefox),
    ]
}

#[cfg(target_os = "windows")]
pub fn browsers() -> Vec<Browser> {
    // Windows registers hosts in the registry; the manifest lives in our data dir.
    let dir = crate::data_dir();
    let b = |name, kind| Browser { name, kind, detect: dir.clone(), manifests: dir.clone() };
    vec![b("Google Chrome", Kind::Chromium), b("Microsoft Edge", Kind::Chromium), b("Brave", Kind::Chromium), b("Firefox", Kind::Firefox)]
}

pub fn manifest_json(kind: Kind, host_exe: &Path) -> serde_json::Value {
    let mut m = json!({
        "name": HOST_NAME,
        "description": "SDM download manager",
        "path": host_exe,
        "type": "stdio",
    });
    match kind {
        Kind::Chromium => m["allowed_origins"] = json!([format!("chrome-extension://{CHROMIUM_EXTENSION_ID}/")]),
        Kind::Firefox => m["allowed_extensions"] = json!([FIREFOX_EXTENSION_ID]),
    }
    m
}

/// Register the host for every browser found. Returns the names configured.
pub fn install_native_host(host_exe: &Path) -> io::Result<Vec<String>> {
    let mut done = Vec::new();
    for b in browsers() {
        #[cfg(not(target_os = "windows"))]
        if !b.detect.exists() {
            continue;
        }
        let _ = std::fs::remove_file(b.manifests.join(format!("{LEGACY_HOST_NAME}.json")));
        std::fs::create_dir_all(&b.manifests)?;
        let file = b.manifests.join(format!("{HOST_NAME}{}.json", if cfg!(windows) { format!(".{:?}", b.kind).to_lowercase() } else { String::new() }));
        std::fs::write(&file, serde_json::to_vec_pretty(&manifest_json(b.kind, host_exe)).unwrap())?;
        #[cfg(target_os = "windows")]
        register_windows(&b, &file)?;
        done.push(b.name.to_string());
    }
    Ok(done)
}

pub fn uninstall_native_host() -> Vec<String> {
    let mut done = Vec::new();
    for b in browsers() {
        let _ = std::fs::remove_file(b.manifests.join(format!("{LEGACY_HOST_NAME}.json")));
        let file = b.manifests.join(format!("{HOST_NAME}.json"));
        if std::fs::remove_file(&file).is_ok() {
            done.push(b.name.to_string());
        }
    }
    done
}

/// Is the host registered with at least one installed browser?
pub fn is_installed() -> bool {
    browsers().iter().any(|b| b.manifests.join(format!("{HOST_NAME}.json")).exists())
}

#[cfg(target_os = "windows")]
fn register_windows(b: &Browser, manifest: &Path) -> io::Result<()> {
    let key = match (b.kind, b.name) {
        (Kind::Firefox, _) => format!("HKCU\\Software\\Mozilla\\NativeMessagingHosts\\{HOST_NAME}"),
        (_, "Microsoft Edge") => format!("HKCU\\Software\\Microsoft\\Edge\\NativeMessagingHosts\\{HOST_NAME}"),
        (_, "Brave") => format!("HKCU\\Software\\BraveSoftware\\Brave-Browser\\NativeMessagingHosts\\{HOST_NAME}"),
        _ => format!("HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\{HOST_NAME}"),
    };
    let st = std::process::Command::new("reg").args(["add", &key, "/ve", "/t", "REG_SZ", "/d"]).arg(manifest).arg("/f").status()?;
    if st.success() { Ok(()) } else { Err(io::Error::other("reg add failed")) }
}

// ---- bundled extension -------------------------------------------------------

macro_rules! bundled {
    ($($name:literal => $path:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../../../extension/", $path)) as &[u8])),*]
    };
}

const COMMON_FILES: &[(&str, &[u8])] = bundled! {
    "common.js" => "common.js",
    "background.js" => "background.js",
    "popup.html" => "popup.html",
    "popup.js" => "popup.js",
    "icons/icon16.png" => "icons/icon16.png",
    "icons/icon32.png" => "icons/icon32.png",
    "icons/icon48.png" => "icons/icon48.png",
    "icons/icon128.png" => "icons/icon128.png",
};
const MANIFEST_CHROMIUM: &[u8] = include_bytes!("../../../extension/manifest.chromium.json");
const MANIFEST_FIREFOX: &[u8] = include_bytes!("../../../extension/manifest.firefox.json");

fn write_if_changed(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if std::fs::read(path).map(|old| old == bytes).unwrap_or(false) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)
}

/// Write the two ready-to-load extension folders under `base`:
/// `base/chromium` (Chrome, Edge, Brave, ...) and `base/firefox`.
pub fn extract_extension(base: &Path) -> io::Result<(PathBuf, PathBuf)> {
    let out = |sub: &str, manifest: &[u8]| -> io::Result<PathBuf> {
        let dir = base.join(sub);
        for (name, bytes) in COMMON_FILES {
            write_if_changed(&dir.join(name), bytes)?;
        }
        write_if_changed(&dir.join("manifest.json"), manifest)?;
        Ok(dir)
    };
    Ok((out("chromium", MANIFEST_CHROMIUM)?, out("firefox", MANIFEST_FIREFOX)?))
}
