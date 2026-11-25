//! New PodStack versions: asks GitHub for the latest release, so the app can offer it. A copy installed with the
//! Windows installer updates itself: it downloads the new installer, checks it against the release's SHA-256 file,
//! and runs it silently once PodStack has quit, which then starts PodStack again. Everything else (the Arch package,
//! the portable zip) only tells the user and links the release page.

use crate::config;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Settings key (`core.setting::<UpdateSettings>(SETTINGS_KEY)`).
pub const SETTINGS_KEY: &str = "updates";

const LATEST_URL: &str = "https://api.github.com/repos/tylort69/PodStack/releases/latest";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateSettings {
    /// Ask GitHub for a newer release when PodStack starts.
    pub check: bool,
    /// Where PodStack can update itself: download new versions in the background and install them when PodStack quits.
    pub auto_install: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { check: true, auto_install: false }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "0.4.1"
    pub version: String,
    /// The release page, with its notes and downloads.
    pub url: String,
    /// The Windows installer and its SHA-256 file, when the release has both.
    pub installer: Option<Installer>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Installer {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// The `<installer>.sha256` asset: the hash, as `sha256sum` writes it.
    pub checksum_url: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// `(major, minor, patch)` from "v0.4.1", "0.4.1" or "0.4.1-beta"; `None` for anything else.
fn parse(version: &str) -> Option<(u64, u64, u64)> {
    let v = version.trim().trim_start_matches('v');
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next().flatten().unwrap_or(0), parts.next().flatten().unwrap_or(0)))
}

/// Whether `latest` is a newer version than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    matches!((parse(latest), parse(current)), (Some(l), Some(c)) if l > c)
}

/// The installer of `version` among a release's assets, with its checksum file: without the checksum there's no
/// self-update, only the link.
fn installer(assets: &[GithubAsset], version: &str) -> Option<Installer> {
    let name = format!("PodStack-Native-Setup-{version}.exe");
    let exe = assets.iter().find(|a| a.name == name)?;
    let sum = assets.iter().find(|a| a.name == format!("{name}.sha256"))?;
    Some(Installer { name, url: exe.browser_download_url.clone(), size: exe.size, checksum_url: sum.browser_download_url.clone() })
}

fn release_from(r: GithubRelease) -> Option<Release> {
    if r.draft || r.prerelease || !is_newer(&r.tag_name, config::VERSION) {
        return None;
    }
    let version = r.tag_name.trim_start_matches('v').to_string();
    let installer = installer(&r.assets, &version);
    Some(Release { version, url: r.html_url, installer })
}

fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder().user_agent(config::user_agent()).build().map_err(|e| e.to_string())
}

/// The latest published release when it's newer than this build.
pub async fn check() -> Result<Option<Release>, String> {
    let resp = client()?
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("GitHub answered {}", resp.status()));
    }
    let release: GithubRelease = resp.json().await.map_err(|e| e.to_string())?;
    Ok(release_from(release))
}

/// Whether this copy can update itself: a Windows build that the installer put in place. The installer leaves
/// `install.ini` beside `bin\`; the portable zip has none.
pub fn can_self_update() -> bool {
    cfg!(windows) && install_dir().is_some_and(|d| d.join("install.ini").is_file())
}

/// The folder holding `bin\podstack.exe`.
fn install_dir() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.parent()?.to_path_buf())
}

fn downloads_dir() -> PathBuf {
    crate::store::cache_dir().join("updates")
}

/// The hash in a `.sha256` file: its first word, as `sha256sum` and PowerShell's `Get-FileHash` both put it.
fn parse_checksum(text: &str) -> Option<String> {
    let h = text.split_whitespace().next()?.to_ascii_lowercase();
    (h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h)
}

/// Downloads the release's installer into the cache (or finds it there from before), checks its size and SHA-256,
/// and returns its path. `progress(done, total)` is called as bytes arrive. Older downloads are removed.
pub async fn download(installer: &Installer, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    let dir = downloads_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&installer.name);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            if e.path() != path {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let client = client()?;
    let sum_text = client
        .get(&installer.checksum_url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .text()
        .await
        .map_err(|e| e.to_string())?;
    let expected = parse_checksum(&sum_text).ok_or("the release's checksum file is unreadable")?;
    if path.is_file() && hash_file(&path).as_deref() == Some(expected.as_str()) {
        progress(installer.size, installer.size);
        return Ok(path);
    }

    let part = dir.join(format!("{}.part", installer.name));
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    {
        use tokio::io::AsyncWriteExt;
        let mut resp = client.get(&installer.url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
        let total = resp.content_length().unwrap_or(installer.size);
        let mut file = tokio::fs::File::create(&part).await.map_err(|e| e.to_string())?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
            done += chunk.len() as u64;
            progress(done, total);
        }
        file.flush().await.map_err(|e| e.to_string())?;
    }
    let got = format!("{:x}", hasher.finalize());
    if (installer.size > 0 && done != installer.size) || got != expected {
        let _ = std::fs::remove_file(&part);
        return Err("the download doesn't match the release's checksum".into());
    }
    std::fs::rename(&part, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

fn hash_file(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Some(format!("{:x}", h.finalize()))
}

/// An installer to run once PodStack has quit.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingInstall {
    pub path: PathBuf,
    /// Start PodStack again when it's done ("Update and restart"), or not (installing on the way out).
    pub relaunch: bool,
}

static PENDING: parking_lot::Mutex<Option<PendingInstall>> = parking_lot::Mutex::new(None);

/// Sets (or clears) the installer to run when PodStack quits.
pub fn set_pending(p: Option<PendingInstall>) {
    *PENDING.lock() = p;
}

pub fn pending() -> Option<PendingInstall> {
    PENDING.lock().clone()
}

/// Runs the pending installer, if any: called once PodStack's window is gone and its data is written. The installer
/// waits for this process to end (Windows' Restart Manager closes it if it lingers), replaces the files, and with
/// `/UPDATE=1` starts PodStack again.
pub fn run_pending() {
    let Some(p) = PENDING.lock().take() else { return };
    let mut cmd = std::process::Command::new(&p.path);
    cmd.args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/NOCANCEL", "/SP-"]);
    if p.relaunch {
        cmd.arg("/UPDATE=1");
    }
    match cmd.spawn() {
        Ok(_) => tracing::info!("running the installer {}", p.path.display()),
        Err(e) => tracing::error!("couldn't run the installer {}: {e}", p.path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.4.1", "0.4.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("v0.4.0", "0.4.0"));
        assert!(!is_newer("v0.3.9", "0.4.0"));
        assert!(!is_newer("v0.2.19-alpha", "0.4.0"));
        assert!(!is_newer("nightly", "0.4.0"));
    }

    fn asset(name: &str) -> GithubAsset {
        GithubAsset { name: name.into(), browser_download_url: format!("https://example.test/{name}"), size: 42 }
    }

    #[test]
    fn finds_the_installer_and_its_checksum() {
        let assets = vec![
            asset("PodStack-Native-0.8.0-portable.zip"),
            asset("PodStack-Native-Setup-0.8.0.exe"),
            asset("PodStack-Native-Setup-0.8.0.exe.sha256"),
            asset("podstack-git-0.8.0.r200.gabc-1-x86_64.pkg.tar.zst"),
        ];
        let i = installer(&assets, "0.8.0").unwrap();
        assert_eq!((i.name.as_str(), i.size), ("PodStack-Native-Setup-0.8.0.exe", 42));
        assert!(i.checksum_url.ends_with(".exe.sha256"));
        // another version's installer, or no checksum: no self-update
        assert_eq!(installer(&assets, "0.8.1"), None);
        assert_eq!(installer(&assets[..2], "0.8.0"), None);

        let r = GithubRelease { tag_name: "v99.0.0".into(), html_url: "page".into(), draft: false, prerelease: false, assets: vec![] };
        assert_eq!(release_from(r).map(|r| (r.version, r.installer)), Some(("99.0.0".into(), None)));
        let old = GithubRelease { tag_name: "v0.0.1".into(), html_url: "page".into(), draft: false, prerelease: false, assets: vec![] };
        assert_eq!(release_from(old), None);
    }

    #[test]
    fn reads_checksum_files() {
        let h = "AB".repeat(32);
        assert_eq!(parse_checksum(&format!("{h}  PodStack-Native-Setup-0.8.0.exe\r\n")), Some(h.to_lowercase()));
        assert_eq!(parse_checksum(&h.to_lowercase()), Some(h.to_lowercase()));
        assert_eq!(parse_checksum("not a hash"), None);
        assert_eq!(parse_checksum(""), None);
    }
}
