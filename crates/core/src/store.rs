//! Where PodStack keeps its files (XDG base directories) and small JSON persistence helpers.
//!
//! Layout under `$XDG_DATA_HOME/podstack` (usually `~/.local/share/podstack`):
//! - `auth.json`            EVE login tokens (mode 0600)
//! - `characters/<id>.json` cached ESI data per character
//! - `settings.json`        app settings
//! - `<name>.json`          documents saved by features via [`load_doc`]/[`save_doc`] (skill plans, farms, …)
//!
//! `$XDG_CACHE_HOME/podstack` holds re-downloadable data: names, types, structures and images.
//!
//! On Windows the folders are `%APPDATA%\PodStack Native` and `%LOCALAPPDATA%\PodStack Native`: the original
//! Electron PodStack keeps its files in `%APPDATA%\PodStack`, and Windows ignores case in folder names, so plain
//! "podstack" would mix the two apps' files.

use serde::{de::DeserializeOwned, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The folder name under the platform's data and cache folders.
const DIR_NAME: &str = if cfg!(windows) { "PodStack Native" } else { "podstack" };

pub fn data_dir() -> PathBuf {
    let dir = std::env::var_os("PODSTACK_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join(DIR_NAME));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn cache_dir() -> PathBuf {
    let dir = std::env::var_os("PODSTACK_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::cache_dir().unwrap_or_else(|| PathBuf::from(".")).join(DIR_NAME));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn characters_dir() -> PathBuf {
    let dir = data_dir().join("characters");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Reads a JSON file; a missing or unreadable file gives `None` (a corrupt one is logged and kept aside).
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(v) => Some(v),
        Err(err) => {
            tracing::warn!("{} is not valid ({err}); keeping it as .corrupt", path.display());
            let _ = std::fs::rename(path, path.with_extension("json.corrupt"));
            None
        }
    }
}

/// Writes JSON atomically (temp file + rename), so a crash mid-write never leaves a half file.
pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T, private: bool) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    write_bytes(path, &bytes, private)
}

pub fn write_bytes(path: &Path, bytes: &[u8], private: bool) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        if private {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        // On Windows the user's profile folder is already closed to other users.
        #[cfg(not(unix))]
        let _ = private;
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, path)
}

/// A feature's own document in the data directory, e.g. `load_doc::<Plans>("skill_plans")`.
pub fn load_doc<T: DeserializeOwned + Default>(name: &str) -> T {
    read_json(&data_dir().join(format!("{name}.json"))).unwrap_or_default()
}

pub fn save_doc<T: Serialize + ?Sized>(name: &str, value: &T) {
    if let Err(err) = write_json(&data_dir().join(format!("{name}.json")), value, false) {
        tracing::error!("saving {name}: {err}");
    }
}
