use ipc::protocol::{Reply, Request};
use ipc::{client, Handler, Server};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("sdm-ipc-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn echo() -> Handler {
    Arc::new(|r: Request| Reply { ok: true, error: Some(format!("{}|{}", r.kind, r.url)), ..Default::default() })
}

#[test]
fn round_trip_and_cleanup() {
    let dir = tmp("rt");
    {
        let _s = Server::start(&dir, "1.2.3", echo()).unwrap();
        let r = client::send(&dir, Request::add("https://e.com/x"), Duration::from_secs(2)).unwrap();
        assert!(r.ok);
        assert_eq!(r.error.as_deref(), Some("add|https://e.com/x"));
        assert!(client::is_running(&dir).is_some());
        assert_eq!(client::read_info(&dir).unwrap().version, "1.2.3");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(dir.join("ipc.json")).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "the secret must be readable only by the user");
        }
    }
    assert!(!dir.join("ipc.json").exists(), "ipc.json removed when the app exits");
    assert!(client::send(&dir, Request::new("ping"), Duration::from_secs(1)).is_err());
}

#[test]
fn rejects_wrong_token_and_garbage() {
    let dir = tmp("auth");
    let s = Server::start(&dir, "1", echo()).unwrap();
    for payload in [r#"{"token":"nope","type":"add","url":"https://e.com"}"#, "garbage"] {
        let mut c = TcpStream::connect(("127.0.0.1", s.port())).unwrap();
        c.write_all(format!("{payload}\n").as_bytes()).unwrap();
        let mut line = String::new();
        BufReader::new(&c).read_line(&mut line).unwrap();
        let r: Reply = serde_json::from_str(&line).unwrap();
        assert!(!r.ok, "{payload}");
    }
}

#[test]
fn stale_info_file_is_not_mistaken_for_a_running_app() {
    let dir = tmp("stale");
    std::fs::write(dir.join("ipc.json"), r#"{"port":9,"token":"t","pid":1,"version":"x"}"#).unwrap();
    assert!(client::is_running(&dir).is_none());
}

#[test]
fn ensure_running_launches_then_waits() {
    let dir = tmp("launch");
    let d2 = dir.clone();
    let mut keep = None;
    let r = client::ensure_running(
        &dir,
        || {
            // "start the app": it listens a moment later
            let s = Server::start(&d2, "1", echo());
            keep = Some(s);
            Ok(())
        },
        Duration::from_secs(3),
    );
    assert!(r.is_ok());
    drop(keep);
    // a failing launch is reported immediately
    let dir = tmp("launch-fail");
    let r = client::ensure_running(&dir, || Err(std::io::Error::other("no such app")), Duration::from_secs(3));
    assert!(r.is_err());
}
