use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Reported once, right after the server has been probed.
#[derive(Clone, Debug)]
pub struct Info {
    /// Final file path (already de-duplicated, e.g. "name (1).zip").
    pub path: PathBuf,
    pub filename: String,
    pub total: Option<u64>,
}

#[derive(Clone)]
pub struct Options {
    /// 1..=32
    pub connections: usize,
    /// Called once when the file name and size are known.
    pub on_info: Option<std::sync::Arc<dyn Fn(Info) + Send + Sync>>,
    /// Extra request headers (cookies, referer, authorization, ...).
    pub headers: Vec<(String, String)>,
    /// Optional shared bandwidth limiter.
    pub limiter: Option<std::sync::Arc<crate::RateLimiter>>,
}

impl Default for Options {
    fn default() -> Self {
        Self { connections: 8, on_info: None, headers: Vec::new(), limiter: None }
    }
}

pub(crate) fn with_headers(mut rb: reqwest::RequestBuilder, h: &[(String, String)]) -> reqwest::RequestBuilder {
    for (k, v) in h {
        rb = rb.header(k.as_str(), v.as_str());
    }
    rb
}

pub(crate) fn check_space(dir: &Path, need: u64) -> Result<()> {
    if let Ok(avail) = fs2::available_space(dir) {
        if avail < need {
            anyhow::bail!("Not enough disk space: need {} MB, only {} MB free", need >> 20, avail >> 20);
        }
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Connections currently transferring.
    pub connections: usize,
}

#[derive(Debug)]
pub enum Outcome {
    Done(PathBuf),
    Paused,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Meta {
    pub url: String,
    pub total: Option<u64>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Multi-connection mode: remaining work as [start, end) ranges.
    #[serde(default)]
    pub segments: Option<Vec<[u64; 2]>>,
}

pub(crate) struct Paths {
    pub dest: PathBuf,
    pub part: PathBuf,
    pub meta: PathBuf,
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

pub(crate) fn read_meta(path: &Path) -> Option<Meta> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub(crate) async fn save_meta(path: &Path, meta: &Meta) -> Result<()> {
    let tmp = with_suffix(path, ".tmp");
    tokio::fs::write(&tmp, serde_json::to_vec(meta)?).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

/// Resume a matching `.part`, else pick a free "name (N).ext".
pub(crate) fn locate(dir: &Path, url: &str, filename: &str) -> Paths {
    let (stem, ext) = match filename.rfind('.') {
        Some(i) if i > 0 => (&filename[..i], &filename[i..]),
        _ => (filename, ""),
    };
    for n in 0u32.. {
        let name = if n == 0 { filename.to_string() } else { format!("{stem} ({n}){ext}") };
        let dest = dir.join(name);
        let part = with_suffix(&dest, ".part");
        let meta = with_suffix(&dest, ".part.json");
        if read_meta(&meta).is_some_and(|m| m.url == url) {
            return Paths { dest, part, meta };
        }
        if !dest.exists() && !part.exists() {
            return Paths { dest, part, meta };
        }
    }
    unreachable!()
}
