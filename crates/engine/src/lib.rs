mod limiter;
mod multi;
mod probe;
mod single;
mod types;

pub use limiter::RateLimiter;
pub use probe::{name_from_url, probe, probe_with, Probe};
pub use reqwest::{Client, Url};
pub use tokio_util::sync::CancellationToken;
pub use types::{fmt_size, Info, Options, Outcome, Progress};

use anyhow::Result;
use std::path::Path;
use std::time::Duration;
use types::locate;

/// Files smaller than this use a single connection.
const MIN_MULTI_SIZE: u64 = 1 << 20;

/// HTTP/1.1 only on purpose: HTTP/2 multiplexes everything over one TCP
/// connection, which defeats multi-connection downloading.
pub fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .http1_only()
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) SDM/1.0")
        .redirect(reqwest::redirect::Policy::limited(10))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .tcp_nodelay(true)
        .pool_max_idle_per_host(32)
        .build()?)
}

/// Download `url` into `dir`. Cancel `cancel` to pause (progress is saved and
/// the next call resumes). Uses up to `opts.connections` (max 32) connections
/// when the server supports ranges, otherwise falls back to one.
pub async fn download(
    client: &reqwest::Client,
    url: &str,
    dir: &Path,
    opts: &Options,
    cancel: &CancellationToken,
    mut on_progress: impl FnMut(Progress),
) -> Result<Outcome> {
    tokio::fs::create_dir_all(dir).await?;
    let p = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(Outcome::Paused),
        r = probe_with(client, url, &opts.headers) => r?,
    };
    let paths = locate(dir, url, &p.filename);
    if let Some(f) = &opts.on_info {
        f(Info { path: paths.dest.clone(), filename: p.filename.clone(), total: p.total });
    }
    let conns = opts.connections.clamp(1, 32);
    let multi = conns > 1 && p.accepts_ranges && p.total.is_some_and(|t| t >= MIN_MULTI_SIZE);
    if multi {
        multi::run(client, url, &p, &paths, conns, opts, cancel, &mut on_progress).await
    } else {
        single::run(client, url, &p, &paths, opts, cancel, &mut on_progress).await
    }
}
