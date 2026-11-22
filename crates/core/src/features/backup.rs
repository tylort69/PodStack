//! Password-protected backups of everything PodStack keeps in its data directory: EVE logins (`auth.json`),
//! settings, cached character data and every feature document (skill plans, farms, …). Caches PodStack rebuilds
//! from ESI (`$XDG_CACHE_HOME/podstack`) are left out.
//!
//! The file is JSON with a readable header (date, app version, a short summary) and the data encrypted with
//! AES-256-GCM under a key derived from the password with Argon2id. It contains refresh tokens, so it is useless
//! without the password, and the password can't be recovered.
//!
//! Backups made by the original Electron app (scrypt, its own store layout) can't be restored here.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

const FORMAT: &str = "podstack-native-backup";
const VERSION: u32 = 1;
pub const MIN_PASSWORD_LENGTH: usize = 8;
pub const EXTENSION: &str = "podstack_backup";

/// Argon2id cost: 64 MiB, 3 passes (OWASP's recommendation is at least 19 MiB / 2 passes).
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_PASSES: u32 = 3;

#[derive(Debug, thiserror::Error)]
pub enum BackupError {
    #[error("Use a password of at least {MIN_PASSWORD_LENGTH} characters.")]
    WeakPassword,
    #[error("That file isn't a PodStack backup.")]
    NotABackup,
    #[error("That backup was made by a newer version of PodStack.")]
    TooNew,
    #[error("Wrong password, or the backup file is damaged.")]
    WrongPassword,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub characters: usize,
    pub files: usize,
}

#[derive(Serialize, Deserialize)]
struct Kdf {
    name: String,
    memory_kib: u32,
    passes: u32,
    lanes: u32,
    salt: String,
}

#[derive(Serialize, Deserialize)]
struct Cipher {
    name: String,
    nonce: String,
}

#[derive(Serialize, Deserialize)]
struct BackupFile {
    format: String,
    version: u32,
    created: DateTime<Utc>,
    app: String,
    summary: Summary,
    kdf: Kdf,
    cipher: Cipher,
    /// ciphertext with the GCM tag appended, base64
    data: String,
}

#[derive(Serialize, Deserialize)]
struct Payload {
    /// path relative to the data directory ("characters/123.json") → file contents
    files: BTreeMap<String, String>,
}

/// A decrypted backup, ready to restore.
pub struct Backup {
    pub created: DateTime<Utc>,
    pub app: String,
    pub summary: Summary,
    files: BTreeMap<String, String>,
}

impl Backup {
    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(|s| s.as_str())
    }
}

/// Every JSON file under `dir` (recursively), as paths relative to it. Temporary and corrupt copies are skipped.
fn json_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = match std::fs::read_dir(&d) {
            Ok(e) => e,
            Err(e) if d == dir => return Err(e),
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let ft = entry.file_type()?;
            let path = entry.path();
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() && path.extension().is_some_and(|e| e == "json") {
                if let Ok(rel) = path.strip_prefix(dir) {
                    out.push(rel.to_path_buf());
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

fn rel_key(p: &Path) -> String {
    p.components().filter_map(|c| c.as_os_str().to_str()).collect::<Vec<_>>().join("/")
}

fn summarize(files: &BTreeMap<String, String>) -> Summary {
    let from_auth = files
        .get("auth.json")
        .and_then(|a| serde_json::from_str::<serde_json::Value>(a).ok())
        .and_then(|v| v.get("characters").and_then(|c| c.as_object()).map(|o| o.len()));
    let characters = from_auth.unwrap_or_else(|| files.keys().filter(|k| k.starts_with("characters/")).count());
    Summary { characters, files: files.len() }
}

fn derive_key(password: &str, salt: &[u8], memory_kib: u32, passes: u32, lanes: u32) -> Result<[u8; 32], BackupError> {
    let params = Params::new(memory_kib, passes, lanes, Some(32)).map_err(|_| BackupError::NotABackup)?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|_| BackupError::NotABackup)?;
    Ok(key)
}

/// Reads what a backup of `data_dir` would contain.
fn collect(data_dir: &Path) -> Result<BTreeMap<String, String>, BackupError> {
    let mut files = BTreeMap::new();
    for rel in json_files(data_dir)? {
        let text = std::fs::read_to_string(data_dir.join(&rel))?;
        files.insert(rel_key(&rel), text);
    }
    Ok(files)
}

fn encrypt(files: BTreeMap<String, String>, password: &str, memory_kib: u32, passes: u32) -> Result<Vec<u8>, BackupError> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(BackupError::WeakPassword);
    }
    let summary = summarize(&files);
    let payload = serde_json::to_vec(&Payload { files }).map_err(std::io::Error::other)?;
    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut salt);
    rand::rng().fill_bytes(&mut nonce);
    let key = derive_key(password, &salt, memory_kib, passes, 1)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let data = cipher.encrypt(Nonce::from_slice(&nonce), payload.as_slice()).map_err(|_| std::io::Error::other("encryption failed"))?;
    let file = BackupFile {
        format: FORMAT.into(),
        version: VERSION,
        created: Utc::now(),
        app: format!("podstack/{}", crate::config::VERSION),
        summary,
        kdf: Kdf { name: "argon2id".into(), memory_kib, passes, lanes: 1, salt: B64.encode(salt) },
        cipher: Cipher { name: "aes-256-gcm".into(), nonce: B64.encode(nonce) },
        data: B64.encode(data),
    };
    Ok(serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?)
}

fn decrypt(bytes: &[u8], password: &str) -> Result<Backup, BackupError> {
    let file: BackupFile = serde_json::from_slice(bytes).map_err(|_| BackupError::NotABackup)?;
    if file.format != FORMAT {
        return Err(BackupError::NotABackup);
    }
    if file.version > VERSION || file.kdf.name != "argon2id" || file.cipher.name != "aes-256-gcm" {
        return Err(BackupError::TooNew);
    }
    // refuse absurd costs from a crafted file instead of trying to allocate them
    if file.kdf.memory_kib > 4 * 1024 * 1024 || file.kdf.passes > 100 || file.kdf.lanes == 0 || file.kdf.lanes > 64 {
        return Err(BackupError::NotABackup);
    }
    let salt = B64.decode(&file.kdf.salt).map_err(|_| BackupError::NotABackup)?;
    let nonce = B64.decode(&file.cipher.nonce).map_err(|_| BackupError::NotABackup)?;
    let data = B64.decode(&file.data).map_err(|_| BackupError::NotABackup)?;
    if nonce.len() != 12 {
        return Err(BackupError::NotABackup);
    }
    let key = derive_key(password, &salt, file.kdf.memory_kib, file.kdf.passes, file.kdf.lanes)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let payload = cipher.decrypt(Nonce::from_slice(&nonce), data.as_slice()).map_err(|_| BackupError::WrongPassword)?;
    let payload: Payload = serde_json::from_slice(&payload).map_err(|_| BackupError::WrongPassword)?;
    Ok(Backup { created: file.created, app: file.app, summary: summarize(&payload.files), files: payload.files })
}

/// Backs up `data_dir` into `dest` (mode 0600). Slow (the key derivation): call off the UI thread.
pub fn create(data_dir: &Path, dest: &Path, password: &str) -> Result<Summary, BackupError> {
    let files = collect(data_dir)?;
    let summary = summarize(&files);
    let bytes = encrypt(files, password, KDF_MEMORY_KIB, KDF_PASSES)?;
    crate::store::write_bytes(dest, &bytes, true)?;
    Ok(summary)
}

/// Opens a backup, checking the password. Slow: call off the UI thread.
pub fn read(path: &Path, password: &str) -> Result<Backup, BackupError> {
    let bytes = std::fs::read(path)?;
    decrypt(&bytes, password)
}

/// A backup's path, if it's a plain relative path inside the data directory (no `..`, not absolute).
fn safe_path(data_dir: &Path, key: &str) -> Option<PathBuf> {
    let rel = Path::new(key);
    if key.is_empty() || !rel.components().all(|c| matches!(c, Component::Normal(_))) || rel.extension().is_none_or(|e| e != "json") {
        return None;
    }
    Some(data_dir.join(rel))
}

/// Replaces the JSON files in `data_dir` with the backup's: files the backup doesn't have are removed, so the
/// result is exactly what was backed up. PodStack must restart afterwards (its data is loaded at startup).
pub fn restore(data_dir: &Path, backup: &Backup) -> Result<(), BackupError> {
    let targets: Vec<(PathBuf, &String)> = backup.files.iter().filter_map(|(k, v)| safe_path(data_dir, k).map(|p| (p, v))).collect();
    for rel in json_files(data_dir)? {
        std::fs::remove_file(data_dir.join(rel))?;
    }
    for (path, text) in targets {
        let private = path.file_name().is_some_and(|n| n == "auth.json");
        crate::store::write_bytes(&path, text.as_bytes(), private)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("podstack-backup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("characters")).unwrap();
        d
    }

    fn fill(d: &Path) {
        std::fs::write(d.join("auth.json"), r#"{"characters":{"1":{},"2":{}}}"#).unwrap();
        std::fs::write(d.join("settings.json"), r#"{"theme":{}}"#).unwrap();
        std::fs::write(d.join("skill_plans.json"), r#"{"plans":[]}"#).unwrap();
        std::fs::write(d.join("characters/1.json"), r#"{"id":1}"#).unwrap();
        std::fs::write(d.join("notes.txt"), "not json").unwrap();
        std::fs::write(d.join("settings.json.corrupt"), "x").unwrap();
    }

    // small KDF cost so tests are quick; the cost is stored in the file
    fn quick(d: &Path, password: &str) -> Result<Vec<u8>, BackupError> {
        encrypt(collect(d)?, password, 1024, 1)
    }

    #[test]
    fn round_trip() {
        let src = temp_dir("src");
        fill(&src);
        let bytes = quick(&src, "correct horse").unwrap();
        let backup = decrypt(&bytes, "correct horse").unwrap();
        assert_eq!(backup.summary, Summary { characters: 2, files: 4 });
        assert_eq!(backup.file_names().collect::<Vec<_>>(), vec!["auth.json", "characters/1.json", "settings.json", "skill_plans.json"]);

        let dst = temp_dir("dst");
        std::fs::write(dst.join("characters/99.json"), "{}").unwrap();
        std::fs::write(dst.join("other.json"), "{}").unwrap();
        restore(&dst, &backup).unwrap();
        assert_eq!(std::fs::read_to_string(dst.join("characters/1.json")).unwrap(), r#"{"id":1}"#);
        assert_eq!(std::fs::read_to_string(dst.join("skill_plans.json")).unwrap(), r#"{"plans":[]}"#);
        assert!(!dst.join("characters/99.json").exists());
        assert!(!dst.join("other.json").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dst.join("auth.json")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn wrong_password_and_bad_files() {
        let src = temp_dir("pw");
        fill(&src);
        let bytes = quick(&src, "correct horse").unwrap();
        assert!(matches!(decrypt(&bytes, "wrong horse"), Err(BackupError::WrongPassword)));
        assert!(matches!(decrypt(b"{\"hello\":1}", "x"), Err(BackupError::NotABackup)));
        assert!(matches!(quick(&src, "short"), Err(BackupError::WeakPassword)));
        // tampering is caught by GCM
        let mut file: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mut data = B64.decode(file["data"].as_str().unwrap()).unwrap();
        data[0] ^= 1;
        file["data"] = B64.encode(data).into();
        assert!(matches!(decrypt(&serde_json::to_vec(&file).unwrap(), "correct horse"), Err(BackupError::WrongPassword)));
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn create_and_read_files() {
        let src = temp_dir("file");
        fill(&src);
        let dest = src.join("out").join("backup.podstack_backup");
        let files = collect(&src).unwrap();
        crate::store::write_bytes(&dest, &encrypt(files, "password1", 1024, 1).unwrap(), true).unwrap();
        assert_eq!(read(&dest, "password1").unwrap().summary.characters, 2);
        let _ = std::fs::remove_dir_all(&src);
    }

    #[test]
    fn restore_ignores_paths_outside() {
        let dir = temp_dir("safe");
        assert!(safe_path(&dir, "../evil.json").is_none());
        assert!(safe_path(&dir, "/etc/evil.json").is_none());
        assert!(safe_path(&dir, "characters/1.json").is_some());
        assert!(safe_path(&dir, "script.sh").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
