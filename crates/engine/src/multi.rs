//! Multi-connection engine: up to 32 parallel HTTP/1.1 range connections with
//! IDM-style dynamic splitting (a finished worker steals half of the largest
//! remaining segment), retry with backoff, and resumable on-disk state.

use crate::probe::Probe;
use crate::types::*;
use anyhow::{anyhow, bail, Result};
use futures_util::StreamExt;
use reqwest::header::{CONTENT_RANGE, RANGE, RETRY_AFTER};
use reqwest::{Client, StatusCode};
use std::fs::{File, OpenOptions};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

const BUF: usize = 512 * 1024; // per-connection write buffer
const MIN_INIT_SEG: u64 = 1 << 20; // don't start segments smaller than 1 MiB
const MIN_SPLIT: u64 = 512 * 1024; // remaining bytes required to split a segment
const MAX_RETRIES: u32 = 12;

#[derive(Debug)]
struct Fatal(String);
impl std::fmt::Display for Fatal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Fatal {}
fn fatal(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(Fatal(msg.into()))
}

/// The server (or network) pushed back on opening this connection: too many
/// connections, rate limiting, or it simply wouldn't answer in time.
#[derive(Debug)]
struct Busy {
    retry_after: Option<Duration>,
    msg: String,
}
impl std::fmt::Display for Busy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}
impl std::error::Error for Busy {}

enum Flow {
    Done,
    /// This worker gave its segment back and should exit (connection limit hit).
    Released,
}

/// A byte range [start, end) owned by at most one worker.
struct Seg {
    /// Contiguous prefix already flushed to disk (absolute offset).
    written: AtomicU64,
    /// (claimed position, end). Claimed >= written; end can shrink on split.
    st: Mutex<(u64, u64)>,
    active: AtomicBool,
}

impl Seg {
    fn new(pos: u64, end: u64) -> Arc<Self> {
        Arc::new(Self { written: AtomicU64::new(pos), st: Mutex::new((pos, end)), active: AtomicBool::new(false) })
    }
    fn end(&self) -> u64 {
        self.st.lock().unwrap().1
    }
}

struct Ctx {
    client: Client,
    url: String,
    file: Arc<File>,
    segs: Mutex<Vec<Arc<Seg>>>,
    stop: CancellationToken,
    downloaded: AtomicU64,
    active: AtomicUsize,
    /// Workers still allowed to run; shrinks when the server limits connections.
    live: AtomicUsize,
    headers: Vec<(String, String)>,
    limiter: Option<std::sync::Arc<crate::RateLimiter>>,
}

impl Ctx {
    /// Next unit of work: an idle segment, else half of the largest active one.
    fn steal(&self) -> Option<Arc<Seg>> {
        let mut list = self.segs.lock().unwrap();
        for s in list.iter() {
            if !s.active.load(Acquire) && s.written.load(Relaxed) < s.end() {
                s.active.store(true, Release);
                return Some(s.clone());
            }
        }
        let victim = list
            .iter()
            .filter(|s| s.active.load(Acquire))
            .max_by_key(|s| {
                let g = s.st.lock().unwrap();
                g.1.saturating_sub(g.0)
            })?
            .clone();
        let (mid, old_end) = {
            let mut g = victim.st.lock().unwrap();
            let rem = g.1.saturating_sub(g.0);
            if rem < MIN_SPLIT {
                return None;
            }
            let mid = g.0 + rem / 2;
            let old_end = g.1;
            g.1 = mid;
            (mid, old_end)
        };
        let ns = Seg::new(mid, old_end);
        ns.active.store(true, Release);
        list.push(ns.clone());
        Some(ns)
    }

    /// Give up one worker slot (never the last one).
    fn try_shrink(&self) -> bool {
        dec_above(&self.live, 1)
    }

    fn remaining(&self) -> Vec<[u64; 2]> {
        let list = self.segs.lock().unwrap();
        list.iter()
            .filter_map(|s| {
                let w = s.written.load(Relaxed);
                let e = s.end();
                (w < e).then_some([w, e])
            })
            .collect()
    }
}

/// Atomically decrement `a` if it is greater than `floor`.
fn dec_above(a: &AtomicUsize, floor: usize) -> bool {
    let mut cur = a.load(Relaxed);
    loop {
        if cur <= floor {
            return false;
        }
        match a.compare_exchange_weak(cur, cur - 1, Relaxed, Relaxed) {
            Ok(_) => return true,
            Err(v) => cur = v,
        }
    }
}

fn initial_split(total: u64, conns: usize) -> Vec<(u64, u64)> {
    let n = (conns as u64).min((total / MIN_INIT_SEG).max(1));
    let size = total.div_ceil(n);
    (0..n).map(|i| (i * size, ((i + 1) * size).min(total))).filter(|(a, b)| a < b).collect()
}

#[cfg(unix)]
fn write_all_at(f: &File, buf: &[u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    f.write_all_at(buf, off)
}
#[cfg(windows)]
fn write_all_at(f: &File, mut buf: &[u8], mut off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_write(buf, off)?;
        buf = &buf[n..];
        off += n as u64;
    }
    Ok(())
}

async fn flush(ctx: &Ctx, seg: &Seg, buf: &mut Vec<u8>, off: &mut u64) -> Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    let data = std::mem::take(buf);
    let file = ctx.file.clone();
    let o = *off;
    let data = tokio::task::spawn_blocking(move || write_all_at(&file, &data, o).map(|_| data))
        .await?
        .map_err(|e| fatal(format!("disk write failed: {e}")))?;
    let n = data.len() as u64;
    *buf = data;
    buf.clear();
    *off += n;
    seg.written.fetch_add(n, Relaxed);
    ctx.downloaded.fetch_add(n, Relaxed);
    Ok(())
}

async fn attempt(ctx: &Ctx, seg: &Seg) -> Result<()> {
    // Restart from what is safely on disk; unflushed claimed bytes are redone.
    let (pos, end) = {
        let mut g = seg.st.lock().unwrap();
        g.0 = seg.written.load(Relaxed);
        *g
    };
    if pos >= end {
        return Ok(());
    }
    let req = crate::types::with_headers(
        ctx.client.get(&ctx.url).header(RANGE, format!("bytes={pos}-{}", end - 1)),
        &ctx.headers,
    )
    .send();
    // Pausing must work even while still connecting / waiting for headers.
    let resp = tokio::select! {
        biased;
        _ = ctx.stop.cancelled() => return Ok(()),
        r = req => match r {
            Ok(resp) => resp,
            Err(e) if e.is_connect() || e.is_timeout() => {
                return Err(anyhow::Error::new(Busy { retry_after: None, msg: format!("{e}") }));
            }
            Err(e) => return Err(e.into()),
        },
    };
    let status = resp.status();
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        let retry_after = resp
            .headers()
            .get(RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .map(|n| Duration::from_secs(n.min(30)));
        return Err(anyhow::Error::new(Busy { retry_after, msg: format!("server returned {status}") }));
    }
    if status == StatusCode::OK {
        return Err(fatal("server ignored the Range header"));
    }
    if status != StatusCode::PARTIAL_CONTENT {
        let permanent = status.is_client_error()
            && status != StatusCode::REQUEST_TIMEOUT
            && status != StatusCode::TOO_MANY_REQUESTS;
        if permanent {
            return Err(fatal(format!("server returned {status}")));
        }
        bail!("server returned {status}");
    }
    let start_ok = resp
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("bytes "))
        .and_then(|s| s.split('-').next())
        .and_then(|s| s.parse::<u64>().ok())
        == Some(pos);
    if !start_ok {
        return Err(fatal("server sent the wrong byte range"));
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::with_capacity(BUF + 64 * 1024);
    let mut off = pos;
    let mut last_flush = Instant::now();
    loop {
        let item = tokio::select! {
            biased;
            _ = ctx.stop.cancelled() => {
                flush(ctx, seg, &mut buf, &mut off).await?;
                return Ok(());
            }
            i = stream.next() => i,
        };
        match item {
            Some(Ok(chunk)) => {
                if let Some(l) = &ctx.limiter {
                    tokio::select! {
                        biased;
                        _ = ctx.stop.cancelled() => {
                            flush(ctx, seg, &mut buf, &mut off).await?;
                            return Ok(());
                        }
                        _ = l.acquire(chunk.len()) => {}
                    }
                }
                let (take, finished) = {
                    let mut g = seg.st.lock().unwrap();
                    let rem = g.1.saturating_sub(g.0);
                    let t = (chunk.len() as u64).min(rem);
                    g.0 += t;
                    (t as usize, g.0 >= g.1)
                };
                buf.extend_from_slice(&chunk[..take]);
                // Flush when full, when done, or at least 4x a second so progress
                // stays smooth even on slow or rate-limited connections.
                if finished || buf.len() >= BUF || last_flush.elapsed() >= Duration::from_millis(250) {
                    flush(ctx, seg, &mut buf, &mut off).await?;
                    last_flush = Instant::now();
                }
                if finished {
                    return Ok(());
                }
            }
            Some(Err(e)) => {
                let _ = flush(ctx, seg, &mut buf, &mut off).await;
                return Err(e.into());
            }
            None => {
                flush(ctx, seg, &mut buf, &mut off).await?;
                let done = {
                    let g = seg.st.lock().unwrap();
                    g.0 >= g.1
                };
                if done {
                    return Ok(());
                }
                bail!("connection closed early");
            }
        }
    }
}

async fn run_segment(ctx: &Ctx, seg: &Seg) -> Result<Flow> {
    let mut fails = 0u32;
    loop {
        let before = seg.written.load(Relaxed);
        match attempt(ctx, seg).await {
            Ok(()) => return Ok(Flow::Done),
            Err(e) => {
                if e.downcast_ref::<Fatal>().is_some() {
                    return Err(e);
                }
                if seg.written.load(Relaxed) > before {
                    fails = 0;
                }
                fails += 1;
                let busy = e.downcast_ref::<Busy>();
                // Server refuses this many connections: hand the segment back and
                // run with fewer workers instead of hammering it.
                if busy.is_some() && fails >= 2 && ctx.try_shrink() {
                    seg.active.store(false, Release);
                    return Ok(Flow::Released);
                }
                if fails > MAX_RETRIES {
                    return Err(e.context("too many retries"));
                }
                let wait = busy
                    .and_then(|b| b.retry_after)
                    .unwrap_or_else(|| Duration::from_millis((250u64 << (fails - 1).min(6)).min(15_000)));
                tokio::select! {
                    _ = ctx.stop.cancelled() => return Ok(Flow::Done),
                    _ = tokio::time::sleep(wait) => {}
                }
            }
        }
    }
}

async fn worker(ctx: Arc<Ctx>) -> Result<()> {
    while !ctx.stop.is_cancelled() {
        let Some(seg) = ctx.steal() else { break };
        ctx.active.fetch_add(1, Relaxed);
        let r = run_segment(&ctx, &seg).await;
        ctx.active.fetch_sub(1, Relaxed);
        if let Flow::Released = r? {
            return Ok(()); // slot already given up by try_shrink
        }
    }
    let _ = dec_above(&ctx.live, 0);
    Ok(())
}

pub(crate) async fn run(
    client: &Client,
    url: &str,
    p: &Probe,
    paths: &Paths,
    conns: usize,
    opts: &Options,
    cancel: &CancellationToken,
    on_progress: &mut impl FnMut(Progress),
) -> Result<Outcome> {
    let total = p.total.ok_or_else(|| anyhow!("unknown size"))?;

    // Resume only if the server still serves the exact same file.
    let mut resumed: Option<Vec<(u64, u64)>> = None;
    if let Some(m) = read_meta(&paths.meta) {
        if m.url == url && m.total == p.total && m.etag == p.etag && m.last_modified == p.last_modified {
            let part_len = std::fs::metadata(&paths.part).map(|m| m.len()).ok();
            if let (Some(segs), Some(len)) = (m.segments, part_len) {
                if len == total && segs.iter().all(|s| s[0] < s[1] && s[1] <= total) {
                    resumed = Some(segs.iter().map(|s| (s[0], s[1])).collect());
                }
            }
        }
    }
    let part_path = paths.part.clone();
    let dest_parent = paths.dest.parent().unwrap_or(std::path::Path::new(".")).to_path_buf();
    let (file, ranges) = tokio::task::spawn_blocking(move || -> Result<(File, Vec<(u64, u64)>)> {
        match resumed {
            Some(r) => Ok((OpenOptions::new().write(true).open(&part_path)?, r)),
            None => {
                check_space(&dest_parent, total)?;
                let f = File::create(&part_path)?;
                f.set_len(total)?;
                Ok((f, initial_split(total, conns)))
            }
        }
    })
    .await??;
    let left: u64 = ranges.iter().map(|(a, b)| b - a).sum();

    let ctx = Arc::new(Ctx {
        client: client.clone(),
        url: p.final_url.clone(),
        file: Arc::new(file),
        segs: Mutex::new(ranges.iter().map(|&(a, b)| Seg::new(a, b)).collect()),
        stop: cancel.child_token(),
        downloaded: AtomicU64::new(total - left),
        active: AtomicUsize::new(0),
        live: AtomicUsize::new(conns),
        headers: opts.headers.clone(),
        limiter: opts.limiter.clone(),
    });

    let meta_of = |ctx: &Ctx| Meta {
        url: url.to_string(),
        total: p.total,
        etag: p.etag.clone(),
        last_modified: p.last_modified.clone(),
        segments: Some(ctx.remaining()),
    };
    save_meta(&paths.meta, &meta_of(&ctx)).await?;

    let mut set: JoinSet<Result<()>> = JoinSet::new();
    for _ in 0..conns {
        set.spawn(worker(ctx.clone()));
    }

    let report = |ctx: &Ctx| Progress {
        downloaded: ctx.downloaded.load(Relaxed),
        total: Some(total),
        connections: ctx.active.load(Relaxed),
    };
    on_progress(report(&ctx));

    let mut tick = tokio::time::interval(Duration::from_millis(100));
    let mut last_save = Instant::now();
    let mut first_err: Option<anyhow::Error> = None;
    let mut rounds = 0u32;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                on_progress(report(&ctx));
                if last_save.elapsed() >= Duration::from_secs(1) {
                    last_save = Instant::now();
                    let _ = save_meta(&paths.meta, &meta_of(&ctx)).await;
                }
            }
            r = set.join_next() => match r {
                None => {
                    // A worker that gave its segment back may have been the last one
                    // standing; make sure leftovers are picked up.
                    if first_err.is_none() && !cancel.is_cancelled() && !ctx.remaining().is_empty() && rounds < 8 {
                        rounds += 1;
                        ctx.live.store(1, Relaxed);
                        set.spawn(worker(ctx.clone()));
                        continue;
                    }
                    break;
                }
                Some(Ok(Ok(()))) => {}
                Some(Ok(Err(e))) => { first_err.get_or_insert(e); ctx.stop.cancel(); }
                Some(Err(e)) => { first_err.get_or_insert(anyhow!(e)); ctx.stop.cancel(); }
            }
        }
    }

    on_progress(report(&ctx));
    save_meta(&paths.meta, &meta_of(&ctx)).await?;
    if let Some(e) = first_err {
        return Err(e);
    }
    if cancel.is_cancelled() {
        return Ok(Outcome::Paused);
    }
    if !ctx.remaining().is_empty() {
        bail!("incomplete download (run again to resume)");
    }

    drop(set);
    drop(ctx); // close the file before renaming (required on Windows)
    tokio::fs::rename(&paths.part, &paths.dest).await?;
    let _ = std::fs::remove_file(&paths.meta);
    Ok(Outcome::Done(paths.dest.clone()))
}
