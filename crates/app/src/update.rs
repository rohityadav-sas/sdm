//! In-app self-update module.
//! Checks GitHub Releases, downloads release archive, and replaces
//! current executable and companion helper using `self-replace`.

use anyhow::Result;
use std::io::Read;
use std::path::PathBuf;

pub const GITHUB_REPO: &str = "rohityadav-sas/sdm";
pub const MAX_BINARY_SIZE: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub latest_version: String,
    pub current_version: String,
    pub is_newer: bool,
    pub asset_name: String,
    pub download_url: String,
}

pub fn target_asset_suffix() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("windows-x86_64.zip"),
        ("windows", "aarch64") => Some("windows-arm64.zip"),
        ("linux", "x86_64") => Some("linux-x86_64.zip"),
        ("linux", "aarch64") => Some("linux-arm64.zip"),
        ("macos", "x86_64") => Some("macos-x86_64.zip"),
        ("macos", "aarch64") => Some("macos-arm64.zip"),
        _ => None,
    }
}

pub fn parse_semver(v: &str) -> Option<semver::Version> {
    let clean = v.trim().strip_prefix('v').unwrap_or(v.trim());
    semver::Version::parse(clean).ok()
}

pub fn is_newer_version(latest: &str, current: &str) -> bool {
    match (parse_semver(latest), parse_semver(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn binary_name() -> &'static str {
    if cfg!(windows) { "sdm.exe" } else { "sdm" }
}

pub fn host_binary_name() -> &'static str {
    if cfg!(windows) { "sdm-host.exe" } else { "sdm-host" }
}

/// Check if the installation directory is writable by the current user.
pub fn check_permissions() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate current executable: {e}"))?;
    if let Some(parent) = exe.parent() {
        let test_file = parent.join(format!(".sdm_update_test_{}", std::process::id()));
        match std::fs::File::create(&test_file) {
            Ok(_) => {
                let _ = std::fs::remove_file(test_file);
                Ok(())
            }
            Err(_) => Err(format!(
                "SDM is installed in a system directory ({}). Please update using your system package manager.",
                parent.display()
            )),
        }
    } else {
        Ok(())
    }
}

/// Query GitHub Releases API for the latest version.
pub async fn check_for_update(repo_override: Option<&str>) -> Result<UpdateInfo, String> {
    let env_repo = std::env::var("SDM_UPDATE_REPO").ok();
    let repo = repo_override.or(env_repo.as_deref()).unwrap_or(GITHUB_REPO);
    let api_url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let current = current_version();

    let client = engine::client().map_err(|e| format!("Failed to create HTTP client: {e}"))?;
    let resp = client
        .get(&api_url)
        .header("User-Agent", format!("sdm/{current}"))
        .send()
        .await
        .map_err(|e| format!("Could not reach GitHub Releases: {e}"))?;

    let status = resp.status();
    if status.as_u16() == 404 {
        return Err("No published releases found on GitHub yet".to_string());
    }
    if !status.is_success() {
        return Err(format!("GitHub API returned HTTP {status}"));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("Failed to read release response: {e}"))?;

    let release: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid release JSON response: {e}"))?;

    let latest_tag = release["tag_name"]
        .as_str()
        .ok_or_else(|| "Release JSON missing tag_name".to_string())?
        .trim_start_matches('v')
        .to_string();

    let is_newer = is_newer_version(&latest_tag, current);

    let suffix = target_asset_suffix().ok_or_else(|| {
        format!("Unsupported platform: {} {}", std::env::consts::OS, std::env::consts::ARCH)
    })?;

    let assets = release["assets"]
        .as_array()
        .ok_or_else(|| "Release contains no assets list".to_string())?;

    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .is_some_and(|name| name.ends_with(suffix) || (name.contains(std::env::consts::OS) && name.ends_with(".zip")))
        })
        .or_else(|| {
            // fallback: find any matching os archive
            assets.iter().find(|a| {
                a["name"]
                    .as_str()
                    .is_some_and(|name| name.contains(std::env::consts::OS))
            })
        });

    let (asset_name, download_url) = if let Some(a) = asset {
        (
            a["name"].as_str().unwrap_or("").to_string(),
            a["browser_download_url"].as_str().unwrap_or("").to_string(),
        )
    } else {
        (String::new(), String::new())
    };

    Ok(UpdateInfo {
        latest_version: latest_tag,
        current_version: current.to_string(),
        is_newer,
        asset_name,
        download_url,
    })
}

/// Check if enough time has passed since the last background update check (24 hours).
pub fn should_check_background(data_dir: &std::path::Path) -> bool {
    let stamp_path = data_dir.join(".last_update_check");
    if let Ok(meta) = std::fs::metadata(&stamp_path) {
        if let Ok(modified) = meta.modified() {
            if let Ok(elapsed) = modified.elapsed() {
                if elapsed < std::time::Duration::from_secs(24 * 3600) {
                    return false;
                }
            }
        }
    }
    true
}

/// Mark that a background check was performed.
pub fn touch_update_check(data_dir: &std::path::Path) {
    let stamp_path = data_dir.join(".last_update_check");
    let _ = std::fs::write(&stamp_path, b"");
}

/// Extract an entry from a zip archive directly to a target file on disk.
fn extract_entry_to_path<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    filename: &str,
    target: &std::path::Path,
) -> Result<bool, String> {
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| format!("Zip error: {e}"))?;
        let name = file.name().to_string();
        let matches = name == filename || name.ends_with(&format!("/{filename}")) || name.ends_with(&format!("\\{filename}"));
        if matches {
            if file.size() > MAX_BINARY_SIZE {
                return Err(format!("File '{name}' exceeds size limit"));
            }
            let mut out = std::fs::File::create(target).map_err(|e| format!("Failed to create {}: {e}", target.display()))?;
            std::io::copy(&mut file, &mut out).map_err(|e| format!("Failed to extract '{name}': {e}"))?;
            return Ok(true);
        }
    }
    Ok(false)
}

/// Extract a file from an in-memory ZIP archive.
#[cfg(test)]
fn extract_file(zip_bytes: &[u8], filename: &str) -> Result<Option<Vec<u8>>, String> {
    let reader = std::io::Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(reader).map_err(|e| format!("Invalid zip archive: {e}"))?;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| format!("Zip error: {e}"))?;
        let name = file.name().to_string();
        let matches = name == filename || name.ends_with(&format!("/{filename}")) || name.ends_with(&format!("\\{filename}"));
        if matches {
            if file.size() > MAX_BINARY_SIZE {
                return Err(format!("File '{name}' exceeds size limit"));
            }
            let mut buf = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buf).map_err(|e| format!("Failed to read '{name}': {e}"))?;
            return Ok(Some(buf));
        }
    }
    Ok(None)
}

/// Download the update archive and replace current binaries.
pub async fn download_and_apply(info: &UpdateInfo) -> Result<(), String> {
    if info.download_url.is_empty() {
        return Err(format!("No release asset found for {} {}", std::env::consts::OS, std::env::consts::ARCH));
    }

    check_permissions()?;

    let client = engine::client().map_err(|e| format!("Failed to create client: {e}"))?;
    let resp = client
        .get(&info.download_url)
        .header("User-Agent", format!("sdm/{}", info.current_version))
        .send()
        .await
        .map_err(|e| format!("Failed to download update: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Download failed with HTTP {}", resp.status()));
    }

    // Stream the archive to a temp file on disk instead of buffering the entire zip in memory
    let temp_zip = std::env::temp_dir().join(format!("sdm-update-{}.zip", std::process::id()));
    let mut file = std::fs::File::create(&temp_zip).map_err(|e| format!("Failed to create temp zip: {e}"))?;
    let mut resp = resp;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("Download stream error: {e}"))? {
        use std::io::Write;
        file.write_all(&chunk).map_err(|e| format!("Failed to write temp zip: {e}"))?;
    }
    drop(file);

    let zip_file = std::fs::File::open(&temp_zip).map_err(|e| format!("Failed to open temp zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(zip_file).map_err(|e| format!("Invalid zip archive: {e}"))?;

    let current_exe = std::env::current_exe().map_err(|e| format!("Cannot locate current exe: {e}"))?;
    let temp_path = std::env::temp_dir().join(format!("sdm-update-{}", std::process::id()));

    if !extract_entry_to_path(&mut archive, binary_name(), &temp_path)? {
        let _ = std::fs::remove_file(&temp_zip);
        return Err(format!("'{}' not found in update archive", binary_name()));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&temp_path, std::fs::Permissions::from_mode(0o755));
    }

    let replace_res = self_replace::self_replace(&temp_path).map_err(|e| format!("Could not replace executable: {e}"));
    let _ = std::fs::remove_file(&temp_path);
    if let Err(e) = replace_res {
        let _ = std::fs::remove_file(&temp_zip);
        return Err(e);
    }

    // Also replace sdm-host if present in archive
    let host_target = match current_exe.parent() {
        Some(p) => p.join(host_binary_name()),
        None => PathBuf::from(host_binary_name()),
    };
    let temp_host = std::env::temp_dir().join(format!("sdm-host-update-{}", std::process::id()));
    if let Ok(true) = extract_entry_to_path(&mut archive, host_binary_name(), &temp_host) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temp_host, std::fs::Permissions::from_mode(0o755));
        }
        let _ = std::fs::rename(&temp_host, &host_target);
        let _ = std::fs::remove_file(&temp_host);
    }

    let _ = std::fs::remove_file(&temp_zip);
    Ok(())
}

/// Restart the application using the updated executable.
pub fn restart_app() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("Cannot locate current executable: {e}"))?;
    std::process::Command::new(exe)
        .spawn()
        .map_err(|e| format!("Failed to launch updated app: {e}"))?;
    std::process::exit(0);
}

/// CLI runner for `sdm --update`
pub fn run_cli() -> Result<(), String> {
    println!("Checking for updates...");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("Runtime error: {e}"))?;

    let info = rt.block_on(check_for_update(None))?;
    if !info.is_newer {
        println!("SDM is already up to date (v{}).", info.current_version);
        return Ok(());
    }

    println!("Updating SDM v{} -> v{} ({})...", info.current_version, info.latest_version, info.asset_name);
    rt.block_on(download_and_apply(&info))?;
    println!("Successfully updated to v{}!", info.latest_version);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_parsing_and_comparison() {
        assert!(is_newer_version("1.0.1", "1.0.0"));
        assert!(is_newer_version("v1.1.0", "1.0.9"));
        assert!(is_newer_version("2.0.0", "1.99.99"));
        assert!(!is_newer_version("1.0.0", "1.0.0"));
        assert!(!is_newer_version("1.0.0", "1.0.1"));
        assert!(!is_newer_version("0.9.9", "1.0.0"));
        assert!(!is_newer_version("invalid", "1.0.0"));
    }

    #[test]
    fn current_platform_has_target_suffix() {
        assert!(target_asset_suffix().is_some());
    }

    #[test]
    fn in_memory_zip_extraction() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("sdm", options).unwrap();
            writer.write_all(b"fake-binary-content").unwrap();
            writer.finish().unwrap();
        }

        let extracted = extract_file(&buf, "sdm").unwrap();
        assert_eq!(extracted, Some(b"fake-binary-content".to_vec()));

        let missing = extract_file(&buf, "sdm-host").unwrap();
        assert_eq!(missing, None);
    }
}

