//! Tiny range-capable HTTP server producing a deterministic byte pattern.
//! Used by the end-to-end tests and the screenshot tool; not used by the app.
#![doc(hidden)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

pub fn pattern(i: u64) -> u8 {
    ((i * 31) % 251) as u8
}

pub fn expected(total: u64) -> Vec<u8> {
    (0..total).map(pattern).collect()
}

/// Serves `total` bytes at any path (paths containing "missing" give 404),
/// throttled to `per_conn_bps` bytes/s per connection (0 = unthrottled).
pub fn serve(total: u64, per_conn_bps: u64) -> String {
    serve_requiring(total, per_conn_bps, None)
}

/// Like [`serve`], but answers 403 unless the request carries `Name: value`.
pub fn serve_requiring(total: u64, per_conn_bps: u64, require: Option<(&str, &str)>) -> String {
    let require = require.map(|(n, v)| format!("{}: {}", n.to_ascii_lowercase(), v.to_ascii_lowercase()));
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let require = require.clone();
            std::thread::spawn(move || {
                let _ = handle(s, total, per_conn_bps, require.as_deref());
            });
        }
    });
    format!("http://{addr}")
}

fn handle(mut s: TcpStream, total: u64, bps: u64, require: Option<&str>) -> std::io::Result<()> {
    let mut buf = [0u8; 4096];
    let mut n = 0;
    loop {
        let r = s.read(&mut buf[n..])?;
        if r == 0 {
            return Ok(());
        }
        n += r;
        if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
    if let Some(r) = require {
        if !req.lines().any(|l| l.trim() == r) {
            return s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    }
    if path.contains("missing") {
        return s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    }
    let range = req.lines().find_map(|l| l.strip_prefix("range: bytes=")).and_then(|r| {
        let (a, b) = r.trim().split_once('-')?;
        let a: u64 = a.parse().ok()?;
        let b: u64 = if b.is_empty() { total - 1 } else { b.parse().ok()? };
        Some((a, b.min(total - 1)))
    });
    let (a, b, partial) = match range {
        Some((a, b)) => (a, b, true),
        None => (0, total - 1, false),
    };
    let head = if partial {
        format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{total}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"dev\"\r\nConnection: close\r\n\r\n", b - a + 1)
    } else {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nAccept-Ranges: bytes\r\nETag: \"dev\"\r\nConnection: close\r\n\r\n")
    };
    s.write_all(head.as_bytes())?;
    let mut chunk = vec![0u8; 65536];
    let mut pos = a;
    while pos <= b {
        let len = (65536u64).min(b - pos + 1) as usize;
        for (i, c) in chunk[..len].iter_mut().enumerate() {
            *c = pattern(pos + i as u64);
        }
        s.write_all(&chunk[..len])?;
        pos += len as u64;
        if bps > 0 {
            std::thread::sleep(Duration::from_nanos(len as u64 * 1_000_000_000 / bps));
        }
    }
    Ok(())
}
