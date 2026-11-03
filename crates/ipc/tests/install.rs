//! Native-host registration, run against a fake HOME. Linux layout.
#![cfg(target_os = "linux")]
use ipc::install::*;
use std::path::PathBuf;

#[test]
fn registers_only_browsers_that_exist_and_unpacks_the_extension() {
    let home: PathBuf = std::env::temp_dir().join(format!("sdm-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".config/google-chrome")).unwrap();
    std::fs::create_dir_all(home.join(".config/BraveSoftware/Brave-Browser")).unwrap();
    std::fs::create_dir_all(home.join(".mozilla")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::remove_var("XDG_CONFIG_HOME");

    assert!(!is_installed());
    let host = PathBuf::from("/opt/sdm/sdm-host");
    let done = install_native_host(&host).unwrap();
    assert_eq!(done, vec!["Google Chrome", "Brave", "Firefox"]);
    assert!(is_installed());
    assert!(!home.join(".config/chromium").exists(), "browsers that are not installed are left alone");

    let chrome: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join(".config/google-chrome/NativeMessagingHosts/com.sdm.host.json")).unwrap()).unwrap();
    assert_eq!(chrome["name"], "com.sdm.host");
    assert_eq!(chrome["path"], "/opt/sdm/sdm-host");
    assert_eq!(chrome["type"], "stdio");
    assert_eq!(chrome["allowed_origins"][0], format!("chrome-extension://{CHROMIUM_EXTENSION_ID}/"));
    assert!(chrome.get("allowed_extensions").is_none());

    let ff: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join(".mozilla/native-messaging-hosts/com.sdm.host.json")).unwrap()).unwrap();
    assert_eq!(ff["allowed_extensions"][0], "sdm@local.sdm");
    assert!(ff.get("allowed_origins").is_none());

    // the fixed extension id matches the key shipped in the Chromium manifest
    let (c, f) = extract_extension(&home.join("ext")).unwrap();
    let m: serde_json::Value = serde_json::from_slice(&std::fs::read(c.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["manifest_version"], 3);
    assert!(m["key"].as_str().unwrap().len() > 100);
    let m: serde_json::Value = serde_json::from_slice(&std::fs::read(f.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["browser_specific_settings"]["gecko"]["id"], "sdm@local.sdm");
    for dir in [&c, &f] {
        for file in ["common.js", "background.js", "popup.html", "popup.js", "icons/icon16.png", "icons/icon128.png"] {
            assert!(dir.join(file).exists(), "{file} missing in {dir:?}");
        }
    }

    assert_eq!(uninstall_native_host().len(), 3);
    assert!(!is_installed());
}

#[test]
fn chromium_extension_id_is_derived_from_the_manifest_key() {
    use std::process::{Command, Stdio};
    let m: serde_json::Value = serde_json::from_str(include_str!("../../../extension/manifest.chromium.json")).unwrap();
    let key = m["key"].as_str().unwrap();
    // id = first 16 bytes of sha256(DER public key), hex digits mapped to a-p
    let der = Command::new("sh").arg("-c").arg(format!("printf %s '{key}' | base64 -d | sha256sum")).stdout(Stdio::piped()).output().unwrap();
    let hex = String::from_utf8(der.stdout).unwrap();
    let id: String = hex.chars().take(32).map(|c| (b'a' + c.to_digit(16).unwrap() as u8) as char).collect();
    assert_eq!(id, CHROMIUM_EXTENSION_ID);
}
