//! New PodStack versions: asks GitHub for the latest release, so the app can offer it. Packages from the Arch repo or
//! the Windows installer don't update themselves; this only tells the user and links the release page.

use crate::config;
use serde::{Deserialize, Serialize};

/// Settings key (`core.setting::<UpdateSettings>(SETTINGS_KEY)`).
pub const SETTINGS_KEY: &str = "updates";

const LATEST_URL: &str = "https://api.github.com/repos/tylort69/PodStack/releases/latest";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UpdateSettings {
    /// Ask GitHub for a newer release when PodStack starts.
    pub check: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings { check: true }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// "0.4.1"
    pub version: String,
    /// The release page, with its notes and downloads.
    pub url: String,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
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

/// The latest published release when it's newer than this build.
pub async fn check() -> Result<Option<Release>, String> {
    let client = reqwest::Client::builder().user_agent(config::user_agent()).build().map_err(|e| e.to_string())?;
    let resp = client
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
    if release.draft || release.prerelease || !is_newer(&release.tag_name, config::VERSION) {
        return Ok(None);
    }
    Ok(Some(Release { version: release.tag_name.trim_start_matches('v').to_string(), url: release.html_url }))
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
}
