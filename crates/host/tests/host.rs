//! Drives the real `sdm-host` binary exactly like a browser would.
use ipc::protocol::{Reply, Request};
use ipc::{Handler, Server};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sdm-host-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Host {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Host {
    fn start(dir: &std::path::Path, app: &str) -> Host {
        let mut child = Command::new(env!("CARGO_BIN_EXE_sdm-host"))
            .env("SDM_DATA_DIR", dir)
            .env("SDM_APP_PATH", app)
            .arg("chrome-extension://abc/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let (stdin, stdout) = (child.stdin.take().unwrap(), child.stdout.take().unwrap());
        Host { child, stdin, stdout }
    }
    fn send(&mut self, json: &str) -> serde_json::Value {
        self.stdin.write_all(&(json.len() as u32).to_le_bytes()).unwrap();
        self.stdin.write_all(json.as_bytes()).unwrap();
        self.stdin.flush().unwrap();
        let mut len = [0u8; 4];
        self.stdout.read_exact(&mut len).unwrap();
        let mut buf = vec![0u8; u32::from_le_bytes(len) as usize];
        self.stdout.read_exact(&mut buf).unwrap();
        serde_json::from_slice(&buf).unwrap()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn recording_server(dir: &std::path::Path) -> (Server, Arc<Mutex<Vec<Request>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s2 = seen.clone();
    let handler: Handler = Arc::new(move |r: Request| {
        s2.lock().unwrap().push(r.clone());
        match r.kind.as_str() {
            "ping" => Reply { ok: true, version: Some("9.9.9".into()), ..Default::default() },
            "add" if r.url.contains("reject") => Reply::err("Not a web link"),
            _ => Reply::ok(),
        }
    });
    (Server::start(dir, "9.9.9", handler).unwrap(), seen)
}

#[test]
fn forwards_ping_and_downloads_to_the_app() {
    let dir = tmp("fwd");
    let (_server, seen) = recording_server(&dir);
    let mut h = Host::start(&dir, "/nonexistent/sdm");

    let r = h.send(r#"{"type":"ping","source":"extension"}"#);
    assert_eq!(r["ok"], true);
    assert_eq!(r["version"], "9.9.9");
    assert_eq!(r["running"], true);

    let r = h.send(r#"{"type":"add","url":"https://example.com/a.zip","headers":{"Cookie":"sid=1","Referer":"https://example.com/"},"filename":"a.zip","size":1234,"mime":"application/zip","source":"extension"}"#);
    assert_eq!(r["ok"], true, "{r}");

    let seen = seen.lock().unwrap();
    let add = seen.iter().find(|r| r.kind == "add").expect("app received the download");
    assert_eq!(add.url, "https://example.com/a.zip");
    assert_eq!(add.headers.get("Cookie").map(String::as_str), Some("sid=1"));
    assert_eq!(add.headers.get("Referer").map(String::as_str), Some("https://example.com/"));
    assert_eq!(add.filename.as_deref(), Some("a.zip"));
    assert_eq!(add.source.as_deref(), Some("extension"));
    assert!(!add.token.is_empty(), "host must authenticate with the app's token");
}

#[test]
fn passes_app_errors_back_and_rejects_bad_input() {
    let dir = tmp("errs");
    let (_server, _seen) = recording_server(&dir);
    let mut h = Host::start(&dir, "/nonexistent/sdm");
    assert_eq!(h.send(r#"{"type":"add","url":"https://example.com/reject.zip"}"#)["error"], "Not a web link");
    assert_eq!(h.send(r#"{"type":"add","url":"javascript:alert(1)"}"#)["ok"], false);
    assert_eq!(h.send(r#"{"type":"add","url":"ftp://example.com/a"}"#)["ok"], false);
    assert_eq!(h.send(r#"{"type":"frobnicate"}"#)["ok"], false);
    assert_eq!(h.send(r#"not json"#)["ok"], false);
    // still alive and answering afterwards
    assert_eq!(h.send(r#"{"type":"ping"}"#)["ok"], true);
}

#[test]
fn handles_large_messages() {
    let dir = tmp("big");
    let (_server, seen) = recording_server(&dir);
    let mut h = Host::start(&dir, "/nonexistent/sdm");
    let cookie = "c=".to_string() + &"x".repeat(300_000);
    let msg = format!(r#"{{"type":"add","url":"https://example.com/big.bin","headers":{{"Cookie":"{cookie}"}}}}"#);
    assert_eq!(h.send(&msg)["ok"], true);
    let seen = seen.lock().unwrap();
    assert_eq!(seen.iter().find(|r| r.kind == "add").unwrap().headers["Cookie"].len(), cookie.len());
}

#[test]
fn ping_works_without_the_app_and_does_not_start_it() {
    let dir = tmp("noapp-ping");
    let marker = dir.join("started");
    let script = dir.join("fake-sdm.sh");
    std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut h = Host::start(&dir, script.to_str().unwrap());
    let r = h.send(r#"{"type":"ping"}"#);
    assert_eq!(r["ok"], true);
    assert_eq!(r["running"], false);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!marker.exists(), "a connection check must never launch the app");
}

#[test]
fn reports_a_clear_error_when_the_app_cannot_be_started() {
    let dir = tmp("noapp-add");
    let mut h = Host::start(&dir, "/nonexistent/sdm");
    let started = Instant::now();
    let r = h.send(r#"{"type":"add","url":"https://example.com/a.zip"}"#);
    assert_eq!(r["ok"], false);
    assert!(r["error"].as_str().unwrap().contains("Couldn't start SDM"), "{r}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[cfg(unix)]
#[test]
fn starts_the_app_when_needed_and_waits_for_it() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tmp("launch");
    let marker = dir.join("started");
    let script = dir.join("fake-sdm.sh");
    std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    // The "app" comes up ~1 s after it was launched.
    let (dir2, marker2) = (dir.clone(), marker.clone());
    let app = std::thread::spawn(move || {
        while !marker2.exists() {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(900));
        let (server, seen) = recording_server(&dir2);
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end && !seen.lock().unwrap().iter().any(|r| r.kind == "add") {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(server);
        let found = seen.lock().unwrap().iter().any(|r| r.kind == "add");
        found
    });

    let mut h = Host::start(&dir, script.to_str().unwrap());
    let r = h.send(r#"{"type":"add","url":"https://example.com/a.zip"}"#);
    assert_eq!(r["ok"], true, "{r}");
    assert!(app.join().unwrap(), "the freshly started app must receive the download");
}

#[cfg(target_os = "linux")]
#[test]
fn install_and_uninstall_flags_manage_browser_registration() {
    let home = tmp("home");
    std::fs::create_dir_all(home.join(".config/google-chrome")).unwrap();
    std::fs::create_dir_all(home.join(".mozilla")).unwrap();
    let run = |flag: &str| {
        Command::new(env!("CARGO_BIN_EXE_sdm-host"))
            .arg(flag)
            .env("HOME", &home)
            .env_remove("XDG_CONFIG_HOME")
            .output()
            .unwrap()
    };
    let out = run("--install");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("Google Chrome"));
    let m: serde_json::Value = serde_json::from_slice(&std::fs::read(home.join(".config/google-chrome/NativeMessagingHosts/com.sdm.host.json")).unwrap()).unwrap();
    assert_eq!(m["path"], env!("CARGO_BIN_EXE_sdm-host"));
    assert!(home.join(".mozilla/native-messaging-hosts/com.sdm.host.json").exists());
    assert!(run("--uninstall").status.success());
    assert!(!home.join(".config/google-chrome/NativeMessagingHosts/com.sdm.host.json").exists());
    // nothing to register with -> a clear failure, not a silent success
    let empty = tmp("home-empty");
    let out = Command::new(env!("CARGO_BIN_EXE_sdm-host")).arg("--install").env("HOME", &empty).env_remove("XDG_CONFIG_HOME").output().unwrap();
    assert!(!out.status.success());
}
