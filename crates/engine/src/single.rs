use crate::probe::Probe;
use crate::types::*;
use anyhow::{bail, Context, Result};
use futures_util::StreamExt;
use reqwest::header::{IF_RANGE, RANGE};
use reqwest::{Client, StatusCode};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio_util::sync::CancellationToken;

pub(crate) async fn run(
    client: &Client,
    url: &str,
    p: &Probe,
    paths: &Paths,
    opts: &Options,
    cancel: &CancellationToken,
    on_progress: &mut impl FnMut(Progress),
) -> Result<Outcome> {
    // Resume only if the server still serves the exact same file.
    let mut start = 0u64;
    if let Some(m) = read_meta(&paths.meta) {
        if m.url == url
            && m.segments.is_none()
            && m.total == p.total
            && m.etag == p.etag
            && m.last_modified == p.last_modified
            && p.accepts_ranges
        {
            start = std::fs::metadata(&paths.part).map(|m| m.len()).unwrap_or(0);
        }
    }
    if p.total.is_some_and(|t| start > t) {
        start = 0;
    }

    let meta = Meta {
        url: url.to_string(),
        total: p.total,
        etag: p.etag.clone(),
        last_modified: p.last_modified.clone(),
        segments: None,
    };
    save_meta(&paths.meta, &meta).await?;

    if p.total != Some(start) || start == 0 {
        if start == 0 {
            if let Some(t) = p.total {
                check_space(paths.dest.parent().unwrap_or(std::path::Path::new(".")), t)?;
            }
        }
        let mut req = with_headers(client.get(&p.final_url), &opts.headers);
        if start > 0 {
            req = req.header(RANGE, format!("bytes={start}-"));
            if let Some(v) = p.etag.as_ref().or(p.last_modified.as_ref()) {
                req = req.header(IF_RANGE, v);
            }
        }
        let resp = req.send().await?.error_for_status()?;
        if start > 0 && resp.status() != StatusCode::PARTIAL_CONTENT {
            start = 0; // server ignored the range: start over
        }

        let file = if start > 0 {
            OpenOptions::new().append(true).open(&paths.part).await?
        } else {
            OpenOptions::new().write(true).create(true).truncate(true).open(&paths.part).await?
        };
        let mut w = BufWriter::with_capacity(256 * 1024, file);
        let mut downloaded = start;
        on_progress(Progress { downloaded, total: p.total, connections: 1 });

        let mut stream = resp.bytes_stream();
        loop {
            let item = tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    w.flush().await?;
                    return Ok(Outcome::Paused);
                }
                i = stream.next() => i,
            };
            let Some(chunk) = item else { break };
            let c = chunk.context("connection lost")?;
            if let Some(l) = &opts.limiter {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        w.write_all(&c).await?;
                        w.flush().await?;
                        return Ok(Outcome::Paused);
                    }
                    _ = l.acquire(c.len()) => {}
                }
            }
            w.write_all(&c).await?;
            downloaded += c.len() as u64;
            on_progress(Progress { downloaded, total: p.total, connections: 1 });
        }
        w.flush().await?;

        if let Some(t) = p.total {
            if downloaded != t {
                bail!("incomplete: got {downloaded} of {t} bytes (run again to resume)");
            }
        }
    }

    tokio::fs::rename(&paths.part, &paths.dest).await?;
    let _ = std::fs::remove_file(&paths.meta);
    Ok(Outcome::Done(paths.dest.clone()))
}
