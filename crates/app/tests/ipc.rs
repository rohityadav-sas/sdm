//! The app as seen from the browser extension / command line, headless.
use slint::{ComponentHandle, Model};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use sdm_app::devserver::{expected, serve_requiring};
use sdm_app::{store, App, Config, MainWindow};

mod common;
use common::*;

/// Send a request from another thread (like sdm-host does) while the UI keeps running.
fn request(data: &Path, req: ipc::Request) -> ipc::Reply {
    let (tx, rx) = mpsc::channel();
    let d = data.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(ipc::client::send(&d, req, Duration::from_secs(10)));
    });
    let mut out = None;
    pump_until(12, "ipc reply", || {
        if let Ok(r) = rx.try_recv() {
            out = Some(r);
            true
        } else {
            false
        }
    });
    out.unwrap().expect("ipc call failed")
}

#[test]
fn browser_extension_and_cli_requests() {
    let _win = setup();
    // Never look at the real machine's browser registration.
    let fake_home = tmp("home");
    std::env::set_var("HOME", &fake_home);
    std::env::remove_var("XDG_CONFIG_HOME");

    let data = tmp("data");
    let dl = tmp("dl");
    // This server only serves clients that present the browser's Cookie header.
    let site = serve_requiring(3_000_000, 0, Some(("Cookie", "sid=42")));

    let ui = MainWindow::new().unwrap();
    let _app = App::install(&ui, Config { data_dir: data.clone(), ipc: true });
    ui.show().unwrap();
    ui.set_download_folder(dl.to_string_lossy().into_owned().into());
    assert!(data.join("ipc.json").exists(), "the app announces itself for clients");

    // connection check from the extension popup
    assert_eq!(ui.get_ext_state(), 0);
    let mut ping = ipc::Request::new("ping");
    ping.source = Some("extension".into());
    let r = request(&data, ping);
    assert!(r.ok);
    assert_eq!(r.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(ui.get_ext_state(), 2, "UI shows the extension as connected");

    // a captured download with the browser's cookies completes
    let mut add = ipc::Request::add(&format!("{site}/files/secret.bin"));
    add.source = Some("extension".into());
    add.filename = Some("secret.bin".into());
    add.headers = BTreeMap::from([("Cookie".into(), "sid=42".into()), ("Referer".into(), "https://example.com/".into())]);
    assert!(request(&data, add).ok);
    pump_until(20, "captured download to finish", || status(&ui, 0) == "done");
    assert_eq!(std::fs::read(dl.join("secret.bin")).unwrap(), expected(3_000_000));

    // without the cookie the site refuses, and the row says so
    let mut add = ipc::Request::add(&format!("{site}/files/nocookie.bin"));
    add.source = Some("extension".into());
    assert!(request(&data, add).ok, "accepted by SDM; the failure shows up in the list");
    pump_until(10, "403 row", || status(&ui, 0) == "error");
    assert!(ui.get_downloads().row_data(0).unwrap().detail.contains("403"));

    // cookies are kept so a restart can still resume the download
    pump_until(5, "history saved", || {
        store::load_history(&data).iter().any(|h| h.name == "secret.bin" && h.headers.iter().any(|(k, v)| k == "Cookie" && v == "sid=42"))
    });
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(data.join("downloads.json")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "history holds cookies: user-only");
    }

    // bad input is refused with a reason
    let r = request(&data, ipc::Request::add("javascript:alert(1)"));
    assert!(!r.ok);
    assert!(r.error.unwrap().contains("web link"));
    assert!(!request(&data, ipc::Request::new("bogus")).ok);

    // "show" (second launch of the app) is accepted
    assert!(request(&data, ipc::Request::new("show")).ok);
}
