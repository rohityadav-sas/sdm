use anyhow::Result;
use percent_encoding::percent_decode_str;
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_RANGE, ETAG, LAST_MODIFIED, RANGE};
use reqwest::{Client, StatusCode};

#[derive(Debug, Clone)]
pub struct Probe {
    pub final_url: String,
    pub total: Option<u64>,
    pub accepts_ranges: bool,
    pub filename: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Many servers reject HEAD, so probe with `Range: bytes=0-0`.
pub async fn probe(client: &Client, url: &str) -> Result<Probe> {
    probe_with(client, url, &[]).await
}

pub async fn probe_with(client: &Client, url: &str, headers: &[(String, String)]) -> Result<Probe> {
    let resp = crate::types::with_headers(client.get(url).header(RANGE, "bytes=0-0"), headers)
        .send()
        .await?;
    // 416 on "bytes=0-0" means an empty file ("Content-Range: bytes */0").
    let empty_file = resp.status() == StatusCode::RANGE_NOT_SATISFIABLE;
    if !empty_file {
        resp.error_for_status_ref()?;
    }

    let h = resp.headers();
    let (total, accepts_ranges) = if empty_file {
        let total = h
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .or(Some(0));
        (total, false)
    } else if resp.status() == StatusCode::PARTIAL_CONTENT {
        let total = h
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next())
            .and_then(|s| s.trim().parse::<u64>().ok());
        (total, total.is_some())
    } else {
        (resp.content_length(), false)
    };

    let text = |name| h.get(name).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let etag = text(ETAG);
    let last_modified = text(LAST_MODIFIED);
    let filename = text(CONTENT_DISPOSITION)
        .and_then(|cd| filename_from_disposition(&cd))
        .or_else(|| filename_from_url(resp.url()))
        .map(|n| sanitize(&n))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "download".to_string());

    Ok(Probe {
        final_url: resp.url().to_string(),
        total,
        accepts_ranges,
        filename,
        etag,
        last_modified,
    })
}

fn filename_from_disposition(cd: &str) -> Option<String> {
    let lower = cd.to_ascii_lowercase();
    if let Some(i) = lower.find("filename*=") {
        let v = cd[i + 10..].split(';').next()?.trim().trim_matches('"');
        let v = v.splitn(3, '\'').last()?;
        return Some(percent_decode_str(v).decode_utf8_lossy().into_owned());
    }
    let i = lower.find("filename=")?;
    let v = cd[i + 9..].split(';').next()?.trim().trim_matches('"');
    Some(v.to_string())
}

fn filename_from_url(url: &reqwest::Url) -> Option<String> {
    let seg = url.path_segments()?.last()?;
    if seg.is_empty() {
        return None;
    }
    Some(percent_decode_str(seg).decode_utf8_lossy().into_owned())
}

const RESERVED: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4",
    "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A server-supplied name must never escape the download folder or break on any OS.
pub(crate) fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control() { '_' } else { c })
        .collect();
    let mut n = cleaned.trim().trim_matches('.').trim().to_string();
    // Keep names well under common 255-byte limits, preserving the extension.
    if n.len() > 180 {
        let (stem, ext) = match n.rfind('.') {
            Some(i) if n.len() - i <= 12 => (n[..i].to_string(), n[i..].to_string()),
            _ => (n.clone(), String::new()),
        };
        let mut cut = 180usize.saturating_sub(ext.len());
        while cut > 0 && !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        n = format!("{}{}", &stem[..cut], ext);
    }
    let stem_upper = n.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem_upper.as_str()) {
        n.insert(0, '_');
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_cannot_escape_the_folder() {
        assert_eq!(sanitize("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(sanitize(".."), "");
        assert_eq!(sanitize("..\\..\\evil.exe"), "_.._evil.exe");
        assert_eq!(sanitize("/abs/path.txt"), "_abs_path.txt");
        assert_eq!(sanitize("  report.pdf  "), "report.pdf");
        assert_eq!(sanitize("a\nb.txt"), "a_b.txt");
    }

    #[test]
    fn long_and_reserved_names() {
        let long = format!("{}.tar.gz", "x".repeat(400));
        let n = sanitize(&long);
        assert!(n.len() <= 180 && n.ends_with(".gz"), "{n}");
        let uni = format!("{}.zip", "é".repeat(300));
        let n = sanitize(&uni);
        assert!(n.len() <= 180 && n.ends_with(".zip"));
        assert_eq!(sanitize("CON.txt"), "_CON.txt");
        assert_eq!(sanitize("nul"), "_nul");
        assert_eq!(sanitize("console.txt"), "console.txt");
    }

    #[test]
    fn content_disposition_variants() {
        assert_eq!(filename_from_disposition("attachment; filename=\"a b.zip\"").unwrap(), "a b.zip");
        assert_eq!(filename_from_disposition("attachment; filename*=UTF-8''caf%C3%A9.txt").unwrap(), "café.txt");
        assert_eq!(filename_from_disposition("inline; filename=plain.bin; size=3").unwrap(), "plain.bin");
        assert!(filename_from_disposition("attachment").is_none());
    }
}
