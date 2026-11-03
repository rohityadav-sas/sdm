use engine::{download, CancellationToken, Options, Outcome, RateLimiter};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn data(n: usize) -> Arc<Vec<u8>> {
    Arc::new((0..n).map(|i| (i * 31 % 251) as u8).collect())
}

#[derive(Clone, Default)]
struct Cfg {
    ranges: bool,
    /// Sleep between 64 KB pieces (throttle).
    chunk_delay_ms: u64,
    /// Cut every Nth connection halfway through its body (0 = never).
    drop_every: usize,
    /// Only throttle the request that starts at byte 0.
    slow_first_only: bool,
    /// Delay before sending response headers (simulates a slow server).
    header_delay_ms: u64,
    /// Reject (503 + Retry-After) transfers beyond this many at once (0 = no limit).
    max_active: usize,
    retry_after: u64,
    /// Require the request header `X-SDM-Test: yes`, else 403.
    require_header: bool,
    /// Pretend the file has this many bytes (header only).
    fake_total: u64,
    /// Answer with chunked encoding and no Content-Length, ignoring Range.
    chunked: bool,
}

struct Srv {
    addr: SocketAddr,
    hits: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    rejected: Arc<AtomicUsize>,
}

struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Srv {
    fn url(&self) -> String {
        format!("http://{}/x", self.addr)
    }
}

fn ranged() -> Cfg {
    Cfg { ranges: true, ..Default::default() }
}

async fn serve(body: Arc<Vec<u8>>, cfg: Cfg) -> Srv {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let (active, peak, rejected) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let (h2, a2, p2, r2) = (hits.clone(), active.clone(), peak.clone(), rejected.clone());
    tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            let (body, cfg, hits) = (body.clone(), cfg.clone(), h2.clone());
            let (active, peak, rejected) = (a2.clone(), p2.clone(), r2.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let mut n = 0;
                loop {
                    let r = s.read(&mut buf[n..]).await.unwrap_or(0);
                    if r == 0 {
                        return;
                    }
                    n += r;
                    if buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let hit = hits.fetch_add(1, Ordering::SeqCst) + 1;
                let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                let total = body.len();
                if cfg.require_header && !req.contains("x-sdm-test: yes") {
                    let _ = s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                let range = req.lines().find_map(|l| l.strip_prefix("range: bytes=").map(str::to_owned));
                if cfg.chunked {
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\nContent-Disposition: attachment; filename=\"stream.bin\"\r\n\r\n").await;
                    for piece in body.chunks(50_000) {
                        if s.write_all(format!("{:x}\r\n", piece.len()).as_bytes()).await.is_err() || s.write_all(piece).await.is_err() || s.write_all(b"\r\n").await.is_err() {
                            return;
                        }
                    }
                    let _ = s.write_all(b"0\r\n\r\n").await;
                    let _ = s.shutdown().await;
                    return;
                }
                if total == 0 && range.is_some() && cfg.ranges {
                    let _ = s.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                let mut start = 0usize;
                let head;
                let slice: &[u8];
                match (cfg.ranges, range) {
                    (true, Some(r)) => {
                        let (a, b) = r.split_once('-').unwrap();
                        let a: usize = a.parse().unwrap();
                        let b: usize = if b.is_empty() { total - 1 } else { b.parse().unwrap() };
                        start = a;
                        slice = &body[a..=b];
                        let shown_total = if cfg.fake_total > 0 { cfg.fake_total as usize } else { total };
                        head = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{b}/{shown_total}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nETag: \"t1\"\r\nContent-Disposition: attachment; filename=\"test file.bin\"\r\nConnection: close\r\n\r\n",
                            slice.len()
                        );
                    }
                    _ => {
                        slice = &body[..];
                        head = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nETag: \"t1\"\r\nContent-Disposition: attachment; filename=\"test file.bin\"\r\nConnection: close\r\n\r\n"
                        );
                    }
                }
                let big = slice.len() > 1000; // skip the 1-byte probe
                let _guard = if big && cfg.max_active > 0 {
                    let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                    if now > cfg.max_active {
                        active.fetch_sub(1, Ordering::SeqCst);
                        rejected.fetch_add(1, Ordering::SeqCst);
                        let _ = s
                            .write_all(format!("HTTP/1.1 503 Service Unavailable\r\nRetry-After: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", cfg.retry_after).as_bytes())
                            .await;
                        return;
                    }
                    peak.fetch_max(now, Ordering::SeqCst);
                    Some(Guard(active.clone()))
                } else {
                    None
                };
                let cut = cfg.drop_every > 0 && hit % cfg.drop_every == 0 && slice.len() > 200_000;
                let delay = if cfg.slow_first_only { big && start == 0 } else { big } && cfg.chunk_delay_ms > 0;
                if cfg.header_delay_ms > 0 && big {
                    tokio::time::sleep(Duration::from_millis(cfg.header_delay_ms)).await;
                }
                let _ = s.write_all(head.as_bytes()).await;
                let limit = if cut { slice.len() / 2 } else { slice.len() };
                for piece in slice[..limit].chunks(65536) {
                    if s.write_all(piece).await.is_err() {
                        return;
                    }
                    if delay {
                        tokio::time::sleep(Duration::from_millis(cfg.chunk_delay_ms)).await;
                    }
                }
                let _ = s.shutdown().await;
            });
        }
    });
    Srv { addr, hits, peak, rejected }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("sdm-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn opts(n: usize) -> Options {
    Options { connections: n, ..Default::default() }
}

async fn fetch(url: &str, dir: &PathBuf, conns: usize) -> PathBuf {
    let c = engine::client().unwrap();
    match download(&c, url, dir, &opts(conns), &CancellationToken::new(), |_| {}).await.unwrap() {
        Outcome::Done(p) => p,
        Outcome::Paused => panic!("unexpected pause"),
    }
}

#[tokio::test]
async fn single_connection_download() {
    let body = data(3_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let dir = tmp("single");
    let out = fetch(&srv.url(), &dir, 1).await;
    assert_eq!(out.file_name().unwrap(), "test file.bin");
    assert_eq!(std::fs::read(&out).unwrap(), *body);
    assert!(!dir.join("test file.bin.part").exists());
    assert!(!dir.join("test file.bin.part.json").exists());
}

#[tokio::test]
async fn multi_connection_download_is_byte_exact() {
    let body = data(20_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let dir = tmp("multi");
    let out = fetch(&srv.url(), &dir, 8).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body);
    assert!(srv.hits.load(Ordering::SeqCst) >= 9, "should use several connections");
    assert!(!dir.join("test file.bin.part.json").exists());
}

#[tokio::test]
async fn thirty_two_connections() {
    let body = data(40_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let dir = tmp("c32");
    let out = fetch(&srv.url(), &dir, 32).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body);
    assert!(srv.hits.load(Ordering::SeqCst) >= 33);
}

#[tokio::test]
async fn connections_above_32_are_clamped() {
    let body = data(40_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let dir = tmp("c99");
    let out = fetch(&srv.url(), &dir, 99).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body);
    assert!(srv.hits.load(Ordering::SeqCst) <= 1 + 32 + 40, "no more than 32 at a time");
}

#[tokio::test]
async fn idle_workers_steal_from_slow_segment() {
    let body = data(8_000_000);
    let cfg = Cfg { ranges: true, chunk_delay_ms: 30, slow_first_only: true, ..Default::default() };
    let srv = serve(body.clone(), cfg).await;
    let dir = tmp("steal");
    let out = fetch(&srv.url(), &dir, 4).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body);
    // 1 probe + 4 initial segments + at least 2 steals from the slow one
    assert!(srv.hits.load(Ordering::SeqCst) >= 7, "hits = {}", srv.hits.load(Ordering::SeqCst));
}

#[tokio::test]
async fn pause_then_resume_multi() {
    let body = data(8_000_000);
    let cfg = Cfg { ranges: true, chunk_delay_ms: 20, ..Default::default() };
    let srv = serve(body.clone(), cfg).await;
    let dir = tmp("pause");
    let c = engine::client().unwrap();

    let token = CancellationToken::new();
    let t2 = token.clone();
    let out = download(&c, &srv.url(), &dir, &opts(4), &token, |p| {
        if p.downloaded * 4 >= p.total.unwrap() {
            t2.cancel();
        }
    })
    .await
    .unwrap();
    assert!(matches!(out, Outcome::Paused));
    assert!(dir.join("test file.bin.part.json").exists());
    assert!(!dir.join("test file.bin").exists());

    let mut first = None;
    let out = download(&c, &srv.url(), &dir, &opts(4), &CancellationToken::new(), |p| {
        first.get_or_insert(p.downloaded);
    })
    .await
    .unwrap();
    let Outcome::Done(path) = out else { panic!("expected done") };
    assert!(first.unwrap() > 0, "must resume, not restart");
    assert_eq!(std::fs::read(&path).unwrap(), *body);
}

#[tokio::test]
async fn survives_dropped_connections() {
    let body = data(10_000_000);
    let cfg = Cfg { ranges: true, drop_every: 3, ..Default::default() };
    let srv = serve(body.clone(), cfg).await;
    let dir = tmp("flaky");
    let out = fetch(&srv.url(), &dir, 4).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body);
}

#[tokio::test]
async fn resumes_single_connection_partial() {
    let body = data(2_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let url = srv.url();
    let dir = tmp("resume1");
    std::fs::write(dir.join("test file.bin.part"), &body[..700_000]).unwrap();
    std::fs::write(
        dir.join("test file.bin.part.json"),
        format!(r#"{{"url":"{url}","total":2000000,"etag":"\"t1\"","last_modified":null}}"#),
    )
    .unwrap();
    let c = engine::client().unwrap();
    let mut first = None;
    let out = download(&c, &url, &dir, &opts(1), &CancellationToken::new(), |p| {
        first.get_or_insert(p.downloaded);
    })
    .await
    .unwrap();
    assert_eq!(first, Some(700_000), "must resume, not restart");
    let Outcome::Done(path) = out else { panic!() };
    assert_eq!(std::fs::read(&path).unwrap(), *body);
}

#[tokio::test]
async fn no_range_support_falls_back_to_one_connection() {
    let body = data(5_000_000);
    let srv = serve(body.clone(), Cfg::default()).await;
    let dir = tmp("norange");
    let out = fetch(&srv.url(), &dir, 16).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body);
}

#[tokio::test]
async fn existing_file_gets_numbered_name() {
    let body = data(10_000);
    let srv = serve(body.clone(), ranged()).await;
    let dir = tmp("dup");
    std::fs::write(dir.join("test file.bin"), b"old").unwrap();
    let out = fetch(&srv.url(), &dir, 8).await;
    assert_eq!(out.file_name().unwrap(), "test file (1).bin");
    assert_eq!(std::fs::read(dir.join("test file.bin")).unwrap(), b"old");
}

#[tokio::test]
async fn pause_is_instant_while_server_is_slow_to_answer() {
    let body = data(8_000_000);
    let cfg = Cfg { ranges: true, header_delay_ms: 10_000, ..Default::default() };
    let srv = serve(body.clone(), cfg).await;
    let dir = tmp("slowhdr");
    let c = engine::client().unwrap();
    let token = CancellationToken::new();
    let t2 = token.clone();
    let started = std::time::Instant::now();
    let out = download(&c, &srv.url(), &dir, &opts(8), &token, |_| t2.cancel()).await.unwrap();
    assert!(matches!(out, Outcome::Paused));
    assert!(started.elapsed() < Duration::from_secs(3), "pause took {:?}", started.elapsed());
}

async fn fetch_with(url: &str, dir: &PathBuf, o: Options) -> anyhow::Result<PathBuf> {
    let c = engine::client().unwrap();
    match download(&c, url, dir, &o, &CancellationToken::new(), |_| {}).await? {
        Outcome::Done(p) => Ok(p),
        Outcome::Paused => panic!("unexpected pause"),
    }
}

#[tokio::test]
async fn backs_off_when_server_limits_connections() {
    let body = data(8_000_000);
    let cfg = Cfg { ranges: true, chunk_delay_ms: 20, max_active: 3, retry_after: 1, ..Default::default() };
    let srv = serve(body.clone(), cfg).await;
    let dir = tmp("busy");
    let started = Instant::now();
    let out = fetch(&srv.url(), &dir, 12).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body);
    assert!(srv.rejected.load(Ordering::SeqCst) > 0, "the server should have pushed back");
    assert!(srv.peak.load(Ordering::SeqCst) <= 3);
    assert!(started.elapsed() < Duration::from_secs(40), "took {:?}", started.elapsed());
}

#[tokio::test]
async fn speed_limit_is_respected() {
    let body = data(6_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let dir = tmp("limit");
    let o = Options { connections: 8, limiter: Some(RateLimiter::new(2_000_000)), ..Default::default() };
    let started = Instant::now();
    let out = fetch_with(&srv.url(), &dir, o).await.unwrap();
    let took = started.elapsed();
    assert_eq!(std::fs::read(&out).unwrap(), *body);
    assert!(took >= Duration::from_millis(2300), "finished too fast for a 2 MB/s limit: {took:?}");
    assert!(took < Duration::from_secs(10), "too slow: {took:?}");
}

#[tokio::test]
async fn speed_limit_can_be_lifted_while_running() {
    let body = data(4_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let dir = tmp("lift");
    let lim = RateLimiter::new(400_000); // would take ~10 s
    let o = Options { connections: 4, limiter: Some(lim.clone()), ..Default::default() };
    let l2 = lim.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(600)).await;
        l2.set_rate(0);
    });
    let started = Instant::now();
    let out = fetch_with(&srv.url(), &dir, o).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), *body);
    assert!(started.elapsed() < Duration::from_secs(5), "took {:?}", started.elapsed());
}

#[tokio::test]
async fn refuses_files_bigger_than_free_disk_space() {
    let body = data(2_000_000);
    let cfg = Cfg { ranges: true, fake_total: 1 << 50, ..Default::default() };
    let srv = serve(body, cfg).await;
    let dir = tmp("space");
    let e = fetch_with(&srv.url(), &dir, Options::default()).await.unwrap_err();
    assert!(e.to_string().contains("Not enough disk space"), "{e}");
    assert!(files_none(&dir), "must not leave a giant partial file behind");
}

fn files_none(dir: &PathBuf) -> bool {
    std::fs::read_dir(dir).unwrap().flatten().all(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        !n.ends_with(".part")
    })
}

#[tokio::test]
async fn sends_custom_headers() {
    let body = data(3_000_000);
    let cfg = Cfg { ranges: true, require_header: true, ..Default::default() };
    let srv = serve(body.clone(), cfg).await;

    let dir = tmp("hdr-no");
    let e = fetch_with(&srv.url(), &dir, Options::default()).await.unwrap_err();
    assert!(e.to_string().contains("403"), "{e}");

    let dir = tmp("hdr-yes");
    let o = Options { connections: 4, headers: vec![("X-SDM-Test".into(), "yes".into())], ..Default::default() };
    let out = fetch_with(&srv.url(), &dir, o).await.unwrap();
    assert_eq!(std::fs::read(&out).unwrap(), *body);
}

#[tokio::test]
async fn empty_file_downloads_instead_of_failing_on_416() {
    let srv = serve(data(0), ranged()).await;
    let dir = tmp("empty");
    let out = fetch(&srv.url(), &dir, 8).await;
    assert_eq!(std::fs::metadata(&out).unwrap().len(), 0);
    assert_eq!(out.file_name().unwrap(), "x", "named after the URL: a 416 reply carries no file name");
}

#[tokio::test]
async fn unknown_size_chunked_download_works() {
    let body = data(2_500_000);
    let srv = serve(body.clone(), Cfg { chunked: true, ..Default::default() }).await;
    let dir = tmp("chunked");
    let mut last = 0;
    let c = engine::client().unwrap();
    let out = download(&c, &srv.url(), &dir, &opts(8), &CancellationToken::new(), |p| {
        assert!(p.total.is_none() || p.total == Some(0) || p.total == Some(2_500_000));
        last = p.downloaded;
    })
    .await
    .unwrap();
    let Outcome::Done(path) = out else { panic!() };
    assert_eq!(path.file_name().unwrap(), "stream.bin");
    assert_eq!(std::fs::read(&path).unwrap(), *body);
    assert_eq!(last, 2_500_000);
    assert!(files_none(&dir));
}

#[tokio::test]
async fn stale_partial_data_is_discarded_when_the_file_changed_on_the_server() {
    let body = data(6_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let url = srv.url();
    let dir = tmp("changed");
    // Leftovers from an older version of the file (different ETag), full of garbage.
    std::fs::write(dir.join("test file.bin.part"), vec![0xFFu8; 6_000_000]).unwrap();
    std::fs::write(
        dir.join("test file.bin.part.json"),
        format!(r#"{{"url":"{url}","total":6000000,"etag":"\"old-version\"","last_modified":null,"segments":[[1000000,6000000]]}}"#),
    )
    .unwrap();
    let out = fetch(&url, &dir, 4).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body, "garbage from the old version must not survive");
}

#[tokio::test]
async fn survives_a_corrupt_state_file() {
    let body = data(3_000_000);
    let srv = serve(body.clone(), ranged()).await;
    let dir = tmp("corrupt");
    std::fs::write(dir.join("test file.bin.part"), b"junk").unwrap();
    std::fs::write(dir.join("test file.bin.part.json"), b"{ not json").unwrap();
    let out = fetch(&srv.url(), &dir, 4).await;
    assert_eq!(std::fs::read(&out).unwrap(), *body);
}
