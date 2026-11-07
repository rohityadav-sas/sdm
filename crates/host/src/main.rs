//! Native-messaging host: the tiny program browsers start for the SDM extension.
//! Protocol (stdin/stdout): 4-byte little-endian length + JSON, both directions.
//! It forwards requests to the running SDM app (starting it if necessary).

use ipc::protocol::{Reply, Request};
use ipc::{client, data_dir};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_MESSAGE: usize = 1 << 20;

fn read_message(r: &mut impl Read) -> std::io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_MESSAGE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; n];
    r.read_exact(&mut buf)?;
    Ok(Some(buf))
}

fn write_message(w: &mut impl Write, bytes: &[u8]) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    frame.extend_from_slice(bytes);
    w.write_all(&frame)?;
    w.flush()
}

fn app_path() -> PathBuf {
    if let Some(p) = std::env::var_os("SDM_APP_PATH") {
        return PathBuf::from(p);
    }
    let name = if cfg!(windows) { "sdm.exe" } else { "sdm" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join(name);
            if sibling.exists() {
                return sibling;
            }
        }
    }
    PathBuf::from(name)
}

fn launch_app() -> std::io::Result<()> {
    let mut c = Command::new(app_path());
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // Keep the app alive even if the browser tears down our process group.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    c.spawn().map(|_| ())
}

fn log(dir: &Path, line: &str) {
    let path = dir.join("host.log");
    if std::fs::metadata(&path).map(|m| m.len() > 200_000).unwrap_or(false) {
        let _ = std::fs::remove_file(&path);
    }
    let _ = std::fs::create_dir_all(dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

fn handle(dir: &Path, req: Request) -> Reply {
    match req.kind.as_str() {
        // Checking the connection must not start the app.
        "ping" => match client::send(dir, req, Duration::from_secs(3)) {
            Ok(r) if r.ok => Reply { running: Some(true), ..r },
            _ => Reply { ok: true, version: Some(VERSION.into()), running: Some(false), error: None },
        },
        "add" | "show" => {
            if req.kind == "add" {
                let l = req.url.to_ascii_lowercase();
                if !(l.starts_with("http://") || l.starts_with("https://")) {
                    return Reply::err("Not a web link");
                }
            }
            if let Err(e) = client::ensure_running(dir, launch_app, Duration::from_secs(15)) {
                return Reply::err(format!("Couldn't start SDM: {e}"));
            }
            match client::send(dir, req, Duration::from_secs(10)) {
                Ok(r) => r,
                Err(e) => Reply::err(format!("Couldn't reach SDM: {e}")),
            }
        }
        other => Reply::err(format!("unknown request type '{other}'")),
    }
}

fn cli(args: &[String]) -> Option<i32> {
    match args.first().map(String::as_str) {
        Some("--install") => {
            let me = std::env::current_exe().ok()?;
            match ipc::install::install_native_host(&me) {
                Ok(names) if names.is_empty() => {
                    eprintln!("No supported browser found. Start one browser once, then run this again.");
                    Some(1)
                }
                Ok(names) => {
                    println!("Registered with: {}", names.join(", "));
                    Some(0)
                }
                Err(e) => {
                    eprintln!("Could not register: {e}");
                    Some(1)
                }
            }
        }
        Some("--uninstall") => {
            let names = ipc::install::uninstall_native_host();
            println!("Removed from: {}", if names.is_empty() { "nothing".to_string() } else { names.join(", ") });
            Some(0)
        }
        Some("--version") => {
            println!("sdm-host {VERSION}");
            Some(0)
        }
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(code) = cli(&args) {
        std::process::exit(code);
    }
    let dir = data_dir();
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let msg = match read_message(&mut stdin) {
            Ok(Some(m)) => m,
            Ok(None) => break,
            Err(e) => {
                log(&dir, &format!("read error: {e}"));
                break;
            }
        };
        let reply = match serde_json::from_slice::<Request>(&msg) {
            Ok(req) => {
                let kind = req.kind.clone();
                let shown: String = req.url.chars().take(120).collect();
                let r = handle(&dir, req);
                log(&dir, &format!("{kind} {shown} -> ok={} {}", r.ok, r.error.clone().unwrap_or_default()));
                r
            }
            Err(e) => Reply::err(format!("bad message: {e}")),
        };
        if let Err(e) = write_message(&mut stdout, &serde_json::to_vec(&reply).unwrap()) {
            log(&dir, &format!("write error: {e}"));
            break;
        }
    }
}
