//! EVE login tokens per character, refreshed on demand. Kept in `auth.json` (mode 0600) in the data directory.

use crate::esi::{Auth, EsiError};
use crate::sso::{self, SsoError};
use crate::{config, store};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthorizedCharacter {
    pub id: i64,
    #[serde(default)]
    pub name: String,
    pub access_token: String,
    pub access_token_expiry: DateTime<Utc>,
    pub refresh_token: String,
    #[serde(default)]
    pub owner_hash: String,
    #[serde(default)]
    pub scopes: Vec<String>,
    /// The EVE application that issued the tokens; refreshing and revoking must use the same one.
    pub client_id: String,
    #[serde(default)]
    pub last_refresh: Option<RefreshOutcome>,
    /// When the character was first added (keeps the overview in a stable order).
    #[serde(default = "Utc::now")]
    pub added: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshOutcome {
    pub date: DateTime<Utc>,
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
    /// false when retrying can't help (revoked token, wrong client)
    #[serde(default = "yes")]
    pub should_retry: bool,
    #[serde(default)]
    pub client_problem: bool,
}

fn yes() -> bool {
    true
}

impl AuthorizedCharacter {
    pub fn uses_legacy_client(&self) -> bool {
        self.client_id == config::LEGACY_CLIENT_ID
    }

    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    pub fn missing_scopes(&self) -> Vec<&'static str> {
        config::SCOPES.iter().map(|s| s.name).filter(|s| !self.has_scope(s)).collect()
    }

    /// The token can't be refreshed until the user logs in again.
    pub fn needs_reauth(&self) -> bool {
        matches!(&self.last_refresh, Some(r) if !r.success && !r.should_retry)
    }

    fn refresh_blocked(&self) -> bool {
        match &self.last_refresh {
            Some(r) if !r.success => !r.should_retry || r.date > Utc::now() - Duration::minutes(5),
            _ => false,
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct AuthFile {
    characters: BTreeMap<i64, AuthorizedCharacter>,
}

pub struct AuthStore {
    chars: parking_lot::Mutex<BTreeMap<i64, AuthorizedCharacter>>,
    // one refresh per character at a time
    locks: parking_lot::Mutex<BTreeMap<i64, Arc<tokio::sync::Mutex<()>>>>,
}

fn path() -> std::path::PathBuf {
    store::data_dir().join("auth.json")
}

impl AuthStore {
    pub fn load() -> Self {
        let file: AuthFile = store::read_json(&path()).unwrap_or_default();
        AuthStore { chars: parking_lot::Mutex::new(file.characters), locks: Default::default() }
    }

    fn save_locked(chars: &BTreeMap<i64, AuthorizedCharacter>) {
        let file = AuthFile { characters: chars.clone() };
        if let Err(e) = store::write_json(&path(), &file, true) {
            tracing::error!("saving auth.json: {e}");
        }
    }

    pub fn ids(&self) -> Vec<i64> {
        let chars = self.chars.lock();
        let mut v: Vec<_> = chars.values().collect();
        v.sort_by_key(|c| c.added);
        v.into_iter().map(|c| c.id).collect()
    }

    pub fn get(&self, id: i64) -> Option<AuthorizedCharacter> {
        self.chars.lock().get(&id).cloned()
    }

    pub fn all(&self) -> Vec<AuthorizedCharacter> {
        self.chars.lock().values().cloned().collect()
    }

    pub fn upsert(&self, ch: AuthorizedCharacter) {
        let mut chars = self.chars.lock();
        let added = chars.get(&ch.id).map(|c| c.added).unwrap_or(ch.added);
        chars.insert(ch.id, AuthorizedCharacter { added, ..ch });
        Self::save_locked(&chars);
    }

    pub fn remove(&self, id: i64) -> Option<AuthorizedCharacter> {
        let mut chars = self.chars.lock();
        let r = chars.remove(&id);
        Self::save_locked(&chars);
        r
    }

    /// Replaces the whole store (backup restore).
    pub fn replace_all(&self, list: Vec<AuthorizedCharacter>) {
        let mut chars = self.chars.lock();
        *chars = list.into_iter().map(|c| (c.id, c)).collect();
        Self::save_locked(&chars);
    }

    fn lock_for(&self, id: i64) -> Arc<tokio::sync::Mutex<()>> {
        self.locks.lock().entry(id).or_default().clone()
    }

    /// An access token valid for at least `min_validity` seconds, refreshing it first when needed.
    pub async fn auth(&self, http: &reqwest::Client, id: i64, min_validity: i64) -> Result<Auth, EsiError> {
        let lock = self.lock_for(id);
        let _g = lock.lock().await;
        let ch = self.get(id).ok_or(EsiError::NotAuthorized)?;
        if ch.access_token_expiry > Utc::now() + Duration::seconds(min_validity) {
            return Ok(Auth { character_id: id, token: ch.access_token, scopes: Arc::new(ch.scopes) });
        }
        if ch.refresh_blocked() {
            let r = ch.last_refresh.clone().unwrap();
            return Err(EsiError::Token {
                message: r.error.unwrap_or_else(|| "refresh failed recently".into()),
                permanent: !r.should_retry,
                client: r.client_problem,
            });
        }
        let result = sso::refresh(http, &ch.client_id, &ch.refresh_token).await;
        let mut chars = self.chars.lock();
        let Some(entry) = chars.get_mut(&id) else { return Err(EsiError::NotAuthorized) };
        match result {
            Ok(tokens) => {
                if tokens.refresh_token != entry.refresh_token {
                    tracing::info!("refresh token for {id} changed");
                }
                entry.access_token = tokens.access_token;
                entry.access_token_expiry = tokens.expiry;
                entry.refresh_token = tokens.refresh_token;
                entry.last_refresh = Some(RefreshOutcome {
                    date: Utc::now(),
                    success: true,
                    error: None,
                    should_retry: true,
                    client_problem: false,
                });
                let auth = Auth { character_id: id, token: entry.access_token.clone(), scopes: Arc::new(entry.scopes.clone()) };
                Self::save_locked(&chars);
                Ok(auth)
            }
            Err(err) => {
                let permanent = err.is_permanent();
                let client = err.is_client_problem();
                entry.last_refresh = Some(RefreshOutcome {
                    date: Utc::now(),
                    success: false,
                    error: Some(err.to_string()),
                    should_retry: !permanent,
                    client_problem: client,
                });
                Self::save_locked(&chars);
                Err(EsiError::Token { message: err.to_string(), permanent, client })
            }
        }
    }
}

/// Completes a login: trades the code for tokens and stores the character. Returns its id and name.
pub async fn complete_login(
    store: &AuthStore,
    http: &reqwest::Client,
    client_id: &str,
    code: &str,
    pkce: &sso::Pkce,
) -> Result<(i64, String), SsoError> {
    let tokens = sso::exchange_code(http, client_id, code, pkce).await?;
    let claims = sso::decode_claims(&tokens.access_token)?;
    store.upsert(AuthorizedCharacter {
        id: claims.character_id,
        name: claims.character_name.clone(),
        access_token: tokens.access_token,
        access_token_expiry: tokens.expiry,
        refresh_token: tokens.refresh_token,
        owner_hash: claims.owner_hash,
        scopes: claims.scopes,
        client_id: client_id.to_string(),
        last_refresh: None,
        added: Utc::now(),
    });
    Ok((claims.character_id, claims.character_name))
}
