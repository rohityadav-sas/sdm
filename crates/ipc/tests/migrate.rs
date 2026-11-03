//! The app used to be called Velo: its data folder and browser registration must carry over.
#![cfg(target_os = "linux")]

#[test]
fn old_velo_data_is_adopted_and_old_registration_removed() {
    let base = std::env::temp_dir().join(format!("sdm-migrate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("velo")).unwrap();
    std::fs::write(base.join("velo/settings.json"), br#"{"connections": 12}"#).unwrap();
    std::fs::create_dir_all(base.join("google-chrome/NativeMessagingHosts")).unwrap();
    std::fs::write(base.join("google-chrome/NativeMessagingHosts/com.velo.host.json"), b"{}").unwrap();
    std::env::set_var("XDG_CONFIG_HOME", &base);
    std::env::set_var("HOME", &base);
    std::env::remove_var("SDM_DATA_DIR");

    let dir = ipc::data_dir();
    assert_eq!(dir, base.join("sdm"));
    assert_eq!(std::fs::read(dir.join("settings.json")).unwrap(), br#"{"connections": 12}"#, "settings carried over");
    assert!(!base.join("velo").exists());
    assert_eq!(ipc::data_dir(), dir, "second call changes nothing");

    let done = ipc::install::install_native_host(std::path::Path::new("/opt/sdm/sdm-host")).unwrap();
    assert!(done.contains(&"Google Chrome".to_string()));
    assert!(!base.join("google-chrome/NativeMessagingHosts/com.velo.host.json").exists(), "old registration cleaned up");
    assert!(base.join("google-chrome/NativeMessagingHosts/com.sdm.host.json").exists());
}
