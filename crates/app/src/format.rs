pub fn kind_of(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "bz2" | "zst" | "iso" => "archive",
        "mp4" | "mkv" | "avi" | "mov" | "webm" => "video",
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" => "audio",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => "image",
        "pdf" | "doc" | "docx" | "txt" | "epub" | "xls" | "xlsx" | "ppt" | "pptx" => "doc",
        "exe" | "msi" | "deb" | "rpm" | "appimage" | "dmg" | "apk" => "app",
        _ => "other",
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn name_from_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let n = percent_decode(path.rsplit('/').next().unwrap_or(""));
    // The engine sanitises the real name; keep the placeholder shown in the list tidy too.
    let n: String = n.chars().map(|c| if c.is_control() || c == '/' || c == '\\' { '_' } else { c }).collect();
    let n = n.trim().to_string();
    if n.is_empty() { "download".into() } else { n }
}

pub fn fmt_size(b: u64) -> String {
    const U: [&str; 4] = ["B", "KB", "MB", "GB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < 3 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

pub fn fmt_speed(bps: f64) -> String {
    format!("{}/s", fmt_size(bps.max(0.0) as u64))
}

pub fn fmt_eta(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s left"),
        60..=3599 => format!("{}m {:02}s left", secs / 60, secs % 60),
        _ => format!("{}h {:02}m left", secs / 3600, (secs % 3600) / 60),
    }
}

pub fn fmt_ago(secs: u64) -> String {
    match secs {
        0..=10 => "just now".into(),
        11..=59 => format!("{secs} seconds ago"),
        60..=119 => "1 minute ago".into(),
        120..=3599 => format!("{} minutes ago", secs / 60),
        3600..=7199 => "1 hour ago".into(),
        7200..=86399 => format!("{} hours ago", secs / 3600),
        86400..=172799 => "yesterday".into(),
        _ => format!("{} days ago", secs / 86400),
    }
}

/// Short, human-friendly text for an engine error.
pub fn friendly(e: &anyhow::Error) -> String {
    let full = format!("{e:#}");
    let l = full.to_lowercase();
    let s = if l.contains("timed out") || l.contains("timeout") {
        "Connection timed out"
    } else if l.contains("dns") || l.contains("failed to lookup") || l.contains("name or service") {
        "Can't find that server"
    } else if l.contains("503") || l.contains("429") {
        "Server is busy (too many connections)"
    } else if l.contains("not enough disk space") {
        return full.chars().take(90).collect();
    } else if l.contains("403") {
        "Server refused access (403)"
    } else if l.contains("404") {
        "File not found (404)"
    } else if l.contains("disk write failed") {
        "Couldn't write to disk (full or no permission?)"
    } else if l.contains("connect") {
        "Couldn't connect to the server"
    } else {
        return full.chars().take(90).collect();
    };
    s.to_string()
}

pub fn is_http_link(s: &str) -> bool {
    let l = s.trim().to_ascii_lowercase();
    l.starts_with("http://") || l.starts_with("https://")
}

/// Every http(s) link found in a block of text (one per whitespace-separated token).
pub fn links_in(text: &str) -> Vec<String> {
    text.split_whitespace().filter(|t| is_http_link(t)).map(|t| t.trim_matches(|c| c == '"' || c == '\'' || c == '<' || c == '>' || c == '(').trim_end_matches([',', ';', ')', '.']).to_string()).take(25).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_and_links() {
        assert_eq!(name_from_url("https://a.com/x/My%20File.zip?token=1"), "My File.zip");
        assert_eq!(name_from_url("https://a.com/"), "download");
        assert_eq!(name_from_url("https://a.com/caf%C3%A9%2Bbar.txt"), "café+bar.txt");
        assert_eq!(name_from_url("https://a.com/100%25.zip"), "100%.zip");
        assert_eq!(name_from_url("https://a.com/bad%zz%"), "bad%zz%");
        assert_eq!(links_in("see https://a.com/1.zip and\nhttp://b.org/2.iso, not-a-link"), vec!["https://a.com/1.zip", "http://b.org/2.iso"]);
        assert_eq!(kind_of("movie.MKV"), "video");
        assert_eq!(fmt_size(1536), "1.5 KB");
        assert_eq!(fmt_eta(3725), "1h 02m left");
    }
}
