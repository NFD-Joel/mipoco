//! In-app update check + self-update against GitHub Releases.
//!
//! To stay light (and keep the Windows cross-`check` green — a bundled TLS
//! stack pulls in `ring`, which doesn't cross-compile here) the network call
//! shells out to `curl`, which ships on modern Linux/macOS/Windows. If `curl`
//! is missing or the request fails, the check is simply a no-op — mipoco never
//! blocks or errors on it.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

const OWNER: &str = "NFD-Joel";
const REPO: &str = "mipoco";

/// A newer release than the running binary.
#[derive(Clone, Debug)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub release_url: String,
    pub asset_url: Option<String>,
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

/// Check GitHub for the latest release; `Some` only when it is newer than the
/// running version. Never panics; returns `None` on any network/parse failure.
pub fn check() -> Option<UpdateInfo> {
    let json = curl_json(&format!(
        "https://api.github.com/repos/{OWNER}/{REPO}/releases/latest"
    ))?;
    let rel: Release = serde_json::from_str(&json).ok()?;
    if !is_newer(&rel.tag_name, env!("CARGO_PKG_VERSION")) {
        return None;
    }
    let asset_url = rel
        .assets
        .iter()
        .find(|a| asset_matches_target(&a.name))
        .map(|a| a.browser_download_url.clone());
    Some(UpdateInfo {
        version: rel.tag_name.trim_start_matches('v').to_string(),
        notes: rel.body,
        release_url: rel.html_url,
        asset_url,
    })
}

/// Download the matching asset and replace the running binary in place.
/// Returns a human-readable success message.
pub fn apply(info: &UpdateInfo) -> Result<String> {
    let url = info
        .asset_url
        .as_deref()
        .ok_or_else(|| anyhow!("no downloadable asset for this platform in the release"))?;

    let dir = std::env::temp_dir().join(format!("mipoco-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;

    let download = dir.join("asset");
    curl_download(url, &download).context("download failed")?;

    let lower = url.to_lowercase();
    let binary = if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
        extract(&download, &dir, true)?
    } else if lower.ends_with(".zip") {
        // Windows assets ship as .zip; bsdtar (tar.exe on Windows 10+) reads them.
        extract(&download, &dir, false)?
    } else {
        download.clone()
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    }

    self_replace::self_replace(&binary)
        .context("could not replace the running binary (is it writable?)")?;
    let _ = std::fs::remove_dir_all(&dir);
    Ok(format!("updated to v{} — restart mipoco", info.version))
}

/// Whether release tag `latest` (e.g. "v0.7.0") is a higher semver than `current`.
fn is_newer(latest: &str, current: &str) -> bool {
    parse_ver(latest)
        .zip(parse_ver(current))
        .map(|(l, c)| l > c)
        .unwrap_or(false)
}

/// Parse `major.minor.patch` from a tag, ignoring a leading `v` and any
/// pre-release/build suffix.
fn parse_ver(s: &str) -> Option<(u32, u32, u32)> {
    let core = s.trim().trim_start_matches('v');
    let core = core.split(['-', '+']).next().unwrap_or(core);
    let mut it = core.split('.').map(|p| p.parse::<u32>().ok());
    let major = it.next()??;
    let minor = it.next().flatten().unwrap_or(0);
    let patch = it.next().flatten().unwrap_or(0);
    Some((major, minor, patch))
}

/// Match a release asset to the current build target (arch + OS), preferring
/// archives. e.g. `mipoco-x86_64-unknown-linux-gnu.tar.gz`.
fn asset_matches_target(name: &str) -> bool {
    let n = name.to_lowercase();
    let arch = std::env::consts::ARCH; // "x86_64", "aarch64", …
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other, // "linux", "windows"
    };
    n.contains(arch) && (n.contains(os) || n.contains(std::env::consts::OS))
}

fn curl_json(url: &str) -> Option<String> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "User-Agent: mipoco",
            url,
        ])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn curl_download(url: &str, dest: &std::path::Path) -> Result<()> {
    let status = std::process::Command::new("curl")
        .args(["-fsSL", "-H", "User-Agent: mipoco", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .context("could not run curl")?;
    if !status.success() {
        return Err(anyhow!("curl exited with {status}"));
    }
    Ok(())
}

/// Extract an archive with `tar` into `dir` and return the `mipoco` binary
/// inside it. `gzip` selects `-xzf` (`.tar.gz`) vs `-xf` (`.zip`, read by the
/// bsdtar that ships as `tar.exe` on Windows 10+).
fn extract(archive: &std::path::Path, dir: &std::path::Path, gzip: bool) -> Result<PathBuf> {
    let flag = if gzip { "-xzf" } else { "-xf" };
    let status = std::process::Command::new("tar")
        .arg(flag)
        .arg(archive)
        .arg("-C")
        .arg(dir)
        .status()
        .context("could not run tar")?;
    if !status.success() {
        return Err(anyhow!("tar exited with {status}"));
    }
    find_binary(dir).ok_or_else(|| anyhow!("no `mipoco` binary inside the archive"))
}

/// Recursively locate the `mipoco` (or `mipoco.exe`) binary in an extracted dir.
fn find_binary(dir: &std::path::Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_binary(&path) {
                return Some(found);
            }
        } else if matches!(
            path.file_name().and_then(|n| n.to_str()),
            Some("mipoco") | Some("mipoco.exe")
        ) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("v0.7.0", "0.6.0"));
        assert!(is_newer("0.6.1", "0.6.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("v0.6.0", "0.6.0"));
        assert!(!is_newer("0.5.0", "0.6.0"));
        assert!(!is_newer("garbage", "0.6.0"));
    }

    #[test]
    fn version_parsing_tolerates_suffixes_and_missing_parts() {
        assert_eq!(parse_ver("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_ver("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_ver("2"), Some((2, 0, 0)));
        assert_eq!(parse_ver("1.2.3-rc1"), Some((1, 2, 3)));
        assert_eq!(parse_ver("nope"), None);
    }

    #[test]
    fn asset_target_matching() {
        // construct a plausible asset name for the current target
        let name = format!(
            "mipoco-{}-unknown-{}-gnu.tar.gz",
            std::env::consts::ARCH,
            std::env::consts::OS
        );
        assert!(asset_matches_target(&name));
        assert!(!asset_matches_target("mipoco_amd64.deb-for-other-arch-xyz"));
    }
}
