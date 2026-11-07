use crate::protocol::{Reply, Request};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

const MAX_ACTIVE_CONNS: usize = 32;

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (&x, &y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

pub type Handler = Arc<dyn Fn(Request) -> Reply + Send + Sync>;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Info {
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub version: String,
}

pub(crate) fn info_path(dir: &Path) -> PathBuf {
    dir.join("ipc.json")
}

pub struct Server {
    dir: PathBuf,
    port: u16,
    pid: u32,
    stop: Arc<AtomicBool>,
}

fn random_token() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("random bytes");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Server {
    pub fn start(dir: &Path, version: &str, handler: Handler) -> std::io::Result<Server> {
        std::fs::create_dir_all(dir)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let token = random_token();
        let info = Info { port, token: token.clone(), pid: std::process::id(), version: version.to_string() };
        write_private(&info_path(dir), &serde_json::to_vec(&info).unwrap())?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let active_conns = Arc::new(AtomicUsize::new(0));
        std::thread::Builder::new().name("sdm-ipc".into()).spawn(move || {
            for conn in listener.incoming() {
                if stop2.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = conn else { continue };
                if active_conns.load(Ordering::Relaxed) >= MAX_ACTIVE_CONNS {
                    continue;
                }
                active_conns.fetch_add(1, Ordering::Relaxed);
                let (handler, token, counter) = (handler.clone(), token.clone(), active_conns.clone());
                std::thread::spawn(move || {
                    let _ = serve_one(stream, &token, &handler);
                    counter.fetch_sub(1, Ordering::Relaxed);
                });
            }
        })?;
        Ok(Server { dir: dir.to_path_buf(), port, pid: std::process::id(), stop })
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Remove our info file (but never another instance's).
        if let Ok(b) = std::fs::read(info_path(&self.dir)) {
            if serde_json::from_slice::<Info>(&b).map(|i| i.pid == self.pid && i.port == self.port).unwrap_or(false) {
                let _ = std::fs::remove_file(info_path(&self.dir));
            }
        }
        // Wake the accept loop so the thread exits.
        let _ = TcpStream::connect_timeout(&SocketAddr::from((Ipv4Addr::LOCALHOST, self.port)), Duration::from_millis(200));
    }
}

fn serve_one(stream: TcpStream, token: &str, handler: &Handler) -> std::io::Result<()> {
    let _ = stream.set_nodelay(true);
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut line = String::new();
    // Requests are one JSON line; cap the size so a stray client can't eat memory.
    BufReader::new((&stream).take_limited(1 << 20)).read_line(&mut line)?;
    let reply = match serde_json::from_str::<Request>(&line) {
        Err(_) => Reply::err("bad request"),
        Ok(req) if !constant_time_eq(req.token.as_bytes(), token.as_bytes()) => Reply::err("unauthorized"),
        Ok(req) => handler(req),
    };
    let mut out = serde_json::to_vec(&reply).unwrap();
    out.push(b'\n');
    (&stream).write_all(&out)
}

trait TakeLimited {
    fn take_limited(self, n: u64) -> std::io::Take<Self>
    where
        Self: Sized + std::io::Read;
}
impl<T: std::io::Read> TakeLimited for T {
    fn take_limited(self, n: u64) -> std::io::Take<Self> {
        std::io::Read::take(self, n)
    }
}

#[cfg(unix)]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    drop(f);
    std::fs::rename(tmp, path)
}

#[cfg(not(unix))]
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}
