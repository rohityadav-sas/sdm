use crate::protocol::{Reply, Request};
use crate::server::{info_path, Info};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

pub fn read_info(dir: &Path) -> Option<Info> {
    serde_json::from_slice(&std::fs::read(info_path(dir)).ok()?).ok()
}

/// Send one request to the running app. Errors if no app is listening.
pub fn send(dir: &Path, mut req: Request, timeout: Duration) -> std::io::Result<Reply> {
    let info = read_info(dir).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "SDM is not running"))?;
    req.token = info.token;
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, info.port));
    let stream = TcpStream::connect_timeout(&addr, Duration::from_millis(800))?;
    let _ = stream.set_nodelay(true);
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut line = serde_json::to_vec(&req).unwrap();
    line.push(b'\n');
    (&stream).write_all(&line)?;
    let mut reply = String::new();
    BufReader::new(&stream).read_line(&mut reply)?;
    serde_json::from_str(&reply).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Is an app instance answering right now?
pub fn is_running(dir: &Path) -> Option<Reply> {
    send(dir, Request::new("ping"), Duration::from_secs(2)).ok().filter(|r| r.ok)
}

/// Make sure an app instance is answering, starting one with `launch` if needed.
pub fn ensure_running(dir: &Path, launch: impl FnOnce() -> std::io::Result<()>, wait: Duration) -> std::io::Result<()> {
    if is_running(dir).is_some() {
        return Ok(());
    }
    launch()?;
    let end = Instant::now() + wait;
    let mut delay = Duration::from_millis(25);
    while Instant::now() < end {
        if info_path(dir).exists() && is_running(dir).is_some() {
            return Ok(());
        }
        std::thread::sleep(delay);
        delay = (delay * 2).min(Duration::from_millis(100));
    }
    Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "SDM didn't start in time"))
}
