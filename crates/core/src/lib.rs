//! PodStack's data layer: EVE SSO login, the ESI client, cached character data and the background refresher.
//! Nothing here depends on the UI toolkit.
//!
//! The UI holds one [`Core`] (`Arc<Core>`), reads characters with [`Core::characters`]/[`Core::character`], and
//! listens to [`Core::subscribe`] for [`Event`]s to redraw. All async functions run on a Tokio runtime.

pub mod auth;
pub mod blueprints;
pub mod character;
pub mod config;
pub mod domains;
pub mod esi;
pub mod features;
pub mod fmt;
pub mod market;
pub mod pi;
pub mod public_contracts;
pub mod sde;
pub mod sso;
pub mod static_data;
pub mod store;
pub mod training;
pub mod universe;

pub use character::Character;

use chrono::{Duration, Utc};
use character::RefreshState;
use esi::EsiError;
use futures::stream::{FuturesUnordered, StreamExt};
use parking_lot::{Mutex, RwLock};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Characters refreshed at the same time by the background refresher.
const CHARACTERS_AT_ONCE: usize = 2;
/// How often the background refresher looks for data that's due.
const TICK_SECS: u64 = 15;

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A character's data changed.
    CharacterUpdated(i64),
    /// A character was added or removed.
    CharactersChanged,
    SettingsChanged,
    /// A refresh is running: `done` of `total` data kinds fetched. `done == total` when finished.
    RefreshProgress { id: i64, done: usize, total: usize },
    /// A feature document (e.g. "skill_plans", "farms") changed.
    Doc(String),
    /// A short message for the user (shown as a snackbar).
    Toast(String),
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ThemeSource {
    /// matugen from the current wallpaper when matugen is installed, else the default faction.
    Auto,
    /// matugen from an image (None = the current Hyprland wallpaper).
    Matugen { image: Option<String> },
    /// One of the built-in EVE faction themes, by name.
    Faction { name: String },
    /// A seed color the user picked, "#rrggbb".
    Custom { seed: String },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    Dark,
    Light,
    /// Follow the desktop's color-scheme preference.
    System,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ThemeSettings {
    pub source: ThemeSource,
    pub mode: ThemeMode,
    /// Material scheme variant: tonal-spot, vibrant, expressive, content, fidelity, neutral, monochrome, rainbow,
    /// fruit-salad
    #[serde(default = "default_variant")]
    pub variant: String,
    /// -1 (less) … 0 (standard) … 1 (more)
    #[serde(default)]
    pub contrast: f64,
}

fn default_variant() -> String {
    "tonal-spot".into()
}

impl Default for ThemeSettings {
    fn default() -> Self {
        ThemeSettings { source: ThemeSource::Auto, mode: ThemeMode::Dark, variant: default_variant(), contrast: 0.0 }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct SsoSettings {
    /// The user's own EVE application (None = PodStack's).
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub callback: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub theme: ThemeSettings,
    #[serde(default)]
    pub sso: SsoSettings,
    /// Feature settings by key, e.g. "alerts", "window". Use [`Core::setting`]/[`Core::set_setting`].
    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

pub struct Core {
    pub esi: esi::Esi,
    pub auth: auth::AuthStore,
    pub universe: universe::Universe,
    pub market: market::Market,
    pub public_contracts: public_contracts::PublicContracts,
    chars: RwLock<BTreeMap<i64, Arc<Character>>>,
    settings: RwLock<Settings>,
    tx: broadcast::Sender<Event>,
    dirty: Mutex<HashSet<i64>>,
    refreshing: Mutex<HashMap<i64, (usize, usize)>>,
    deleted: Mutex<HashSet<i64>>,
}

fn settings_path() -> std::path::PathBuf {
    store::data_dir().join("settings.json")
}

fn character_path(id: i64) -> std::path::PathBuf {
    store::characters_dir().join(format!("{id}.json"))
}

impl Core {
    /// Loads everything saved on disk. Call [`Core::start`] from inside a Tokio runtime to begin refreshing.
    pub fn load() -> Arc<Core> {
        let auth = auth::AuthStore::load();
        let mut chars = BTreeMap::new();
        for id in auth.ids() {
            let c: Character = store::read_json(&character_path(id)).unwrap_or_else(|| Character::new(id));
            chars.insert(id, Arc::new(Character { id, ..c }));
        }
        let (tx, _) = broadcast::channel(512);
        Arc::new(Core {
            esi: esi::Esi::new(),
            auth,
            universe: universe::Universe::load(),
            market: market::Market::load(),
            public_contracts: public_contracts::PublicContracts::load(),
            chars: RwLock::new(chars),
            settings: RwLock::new(store::read_json(&settings_path()).unwrap_or_default()),
            tx,
            dirty: Default::default(),
            refreshing: Default::default(),
            deleted: Default::default(),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    pub fn emit(&self, e: Event) {
        let _ = self.tx.send(e);
    }

    pub fn toast(&self, msg: impl Into<String>) {
        self.emit(Event::Toast(msg.into()));
    }

    /// All characters, in the order they were added.
    pub fn characters(&self) -> Vec<Arc<Character>> {
        let chars = self.chars.read();
        self.auth.ids().into_iter().filter_map(|id| chars.get(&id).cloned()).collect()
    }

    pub fn character(&self, id: i64) -> Option<Arc<Character>> {
        self.chars.read().get(&id).cloned()
    }

    /// Changes a character (local feature state, or data applied by a refresh), saves it soon and tells the UI.
    pub fn update_character(&self, id: i64, f: impl FnOnce(&mut Character)) {
        if self.deleted.lock().contains(&id) {
            return;
        }
        {
            let mut chars = self.chars.write();
            let Some(c) = chars.get_mut(&id) else { return };
            f(Arc::make_mut(c));
        }
        self.dirty.lock().insert(id);
        self.emit(Event::CharacterUpdated(id));
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let snapshot = {
            let mut s = self.settings.write();
            f(&mut s);
            s.clone()
        };
        if let Err(e) = store::write_json(&settings_path(), &snapshot, false) {
            tracing::error!("saving settings: {e}");
        }
        self.emit(Event::SettingsChanged);
    }

    /// A feature setting (default when unset or unreadable).
    pub fn setting<T: DeserializeOwned + Default>(&self, key: &str) -> T {
        self.settings.read().extra.get(key).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    pub fn set_setting<T: Serialize>(&self, key: &str, value: &T) {
        if let Ok(v) = serde_json::to_value(value) {
            self.update_settings(|s| {
                s.extra.insert(key.to_string(), v);
            });
        }
    }

    /// The EVE application and callback logins use.
    pub fn sso_client(&self) -> (String, String) {
        let s = self.settings.read();
        match (&s.sso.client_id, &s.sso.callback) {
            (Some(id), Some(cb)) if !id.trim().is_empty() && !cb.trim().is_empty() => (id.trim().to_string(), cb.trim().to_string()),
            _ => (config::DEFAULT_CLIENT_ID.to_string(), config::DEFAULT_CALLBACK.to_string()),
        }
    }

    /// Finishes a login: stores the tokens, adds the character and starts refreshing it.
    pub async fn login(self: &Arc<Self>, client_id: &str, code: &str, pkce: &sso::Pkce) -> Result<(i64, String), sso::SsoError> {
        let (id, name) = auth::complete_login(&self.auth, &self.esi.http, client_id, code, pkce).await?;
        self.deleted.lock().remove(&id);
        {
            let mut chars = self.chars.write();
            let c = chars.entry(id).or_insert_with(|| Arc::new(Character::new(id)));
            // a new login may grant scopes: retry everything
            let c = Arc::make_mut(c);
            for r in c.refreshes.values_mut() {
                r.next = Some(Utc::now());
                r.error = None;
            }
        }
        self.dirty.lock().insert(id);
        self.emit(Event::CharactersChanged);
        let core = self.clone();
        tokio::spawn(async move { core.refresh_character(id, true).await });
        Ok((id, name))
    }

    /// Forgets a character: revokes its token, deletes its data and plans.
    pub async fn remove_character(self: &Arc<Self>, id: i64) {
        self.deleted.lock().insert(id);
        if let Some(a) = self.auth.remove(id) {
            let _ = sso::revoke(&self.esi.http, &a.client_id, &a.refresh_token).await;
        }
        self.chars.write().remove(&id);
        self.dirty.lock().remove(&id);
        let _ = std::fs::remove_file(character_path(id));
        features::plans::delete_all_for(self, id);
        features::farming::remove(self, id);
        features::accounts::forget_character(self, id);
        self.emit(Event::CharactersChanged);
    }

    /// Refresh progress (done, total) while a character is refreshing.
    pub fn refresh_progress(&self, id: i64) -> Option<(usize, usize)> {
        self.refreshing.lock().get(&id).copied()
    }

    /// The "Refresh from ESI" button: refreshes everything now, once per cooldown. Err = time left to wait.
    pub fn refresh_now(self: &Arc<Self>, id: i64) -> Result<(), Duration> {
        if let Some(wait) = self.character(id).and_then(|c| c.manual_refresh_wait()) {
            return Err(wait);
        }
        self.update_character(id, |c| c.last_manual_refresh = Some(Utc::now()));
        let core = self.clone();
        tokio::spawn(async move { core.refresh_character(id, true).await });
        Ok(())
    }

    fn due_domains(&self, c: &Character, force: bool) -> Vec<&'static domains::DomainSpec> {
        let now = Utc::now();
        domains::all()
            .into_iter()
            .filter(|d| match c.refreshes.get(d.key) {
                None => true,
                Some(r) => {
                    if force {
                        // scope failures stay failed until a new login
                        r.error.as_deref() != Some("scope")
                    } else {
                        r.next.map(|n| n <= now).unwrap_or(false)
                    }
                }
            })
            .collect()
    }

    fn mark(&self, id: i64, key: &str, interval_secs: i64, result: Result<(), &EsiError>) {
        let now = Utc::now();
        let state = match result {
            Ok(()) => {
                let jitter = rand::random::<f64>() * interval_secs as f64 * 0.1;
                RefreshState { last: Some(now), next: Some(now + Duration::seconds(interval_secs + jitter as i64)), error: None, message: None }
            }
            Err(err) => {
                let (kind, next) = match err {
                    EsiError::ScopeMissing(_) => ("scope", None),
                    EsiError::Token { permanent: true, client: true, .. } => ("client", None),
                    EsiError::Token { permanent: true, .. } => ("token", None),
                    _ => ("error", Some(now + Duration::minutes(5))),
                };
                RefreshState { last: Some(now), next, error: Some(kind.into()), message: Some(err.to_string()) }
            }
        };
        let key = key.to_string();
        self.update_character(id, move |c| {
            c.refreshes.insert(key, state);
        });
    }

    /// Fetches whatever is due for a character (everything with `force`).
    pub async fn refresh_character(self: &Arc<Self>, id: i64, force: bool) {
        let Some(prev) = self.character(id) else { return };
        let due = self.due_domains(&prev, force);
        if due.is_empty() {
            return;
        }
        {
            let mut r = self.refreshing.lock();
            if r.contains_key(&id) {
                return;
            }
            r.insert(id, (0, due.len()));
        }
        let total = due.len();
        self.emit(Event::RefreshProgress { id, done: 0, total });

        match self.auth.auth(&self.esi.http, id, 60).await {
            Err(err) => {
                tracing::warn!("character {id}: {err}");
                for d in &due {
                    self.mark(id, d.key, d.interval_secs, Err(&err));
                }
            }
            Ok(auth) => {
                let cx = domains::Cx { core: self.clone(), id, auth, prev };
                let mut running: FuturesUnordered<_> =
                    due.iter().map(|d| { let cx = cx.clone(); async move { (*d, (d.run)(cx).await) } }).collect();
                let mut done = 0;
                while let Some((spec, result)) = running.next().await {
                    done += 1;
                    match result {
                        Ok(apply) => {
                            self.update_character(id, apply);
                            self.mark(id, spec.key, spec.interval_secs, Ok(()));
                        }
                        Err(err) => {
                            if !matches!(err, EsiError::ScopeMissing(_)) {
                                tracing::warn!("character {id} {}: {err}", spec.key);
                            }
                            self.mark(id, spec.key, spec.interval_secs, Err(&err));
                        }
                    }
                    self.refreshing.lock().insert(id, (done, total));
                    self.emit(Event::RefreshProgress { id, done, total });
                }
            }
        }
        // Accelerator detection compares attributes and training speed between refreshes, so it runs right after
        // each one rather than only when a page shows it.
        features::plan_tools::accelerator::refresh(self, id);
        self.refreshing.lock().remove(&id);
        self.emit(Event::RefreshProgress { id, done: total, total });
        self.flush();
    }

    /// Writes changed characters and caches to disk.
    pub fn flush(&self) {
        let ids: Vec<i64> = self.dirty.lock().drain().collect();
        for id in ids {
            if let Some(c) = self.character(id) {
                if let Err(e) = store::write_json(&character_path(id), &*c, false) {
                    tracing::error!("saving character {id}: {e}");
                }
            }
        }
        self.universe.flush();
    }

    /// Downloads CCP's newest static data if there is a newer build (`force`: ask CCP even if it was asked
    /// recently). Pages showing static data rebuild on `Event::Doc("sde")`.
    pub async fn update_static_data(&self, force: bool) -> Option<i64> {
        match static_data::update(force).await {
            Ok(Some(build)) => {
                self.emit(Event::Doc("sde".into()));
                if force {
                    self.emit(Event::Toast(format!("Game data updated to CCP build {build}")));
                }
                Some(build)
            }
            Ok(None) => {
                if force {
                    self.emit(Event::Toast("Game data is up to date".into()));
                }
                None
            }
            Err(err) => {
                tracing::warn!("static data update: {err}");
                if force {
                    self.emit(Event::Toast(format!("Couldn't update game data: {err}")));
                }
                None
            }
        }
    }

    /// Starts the background refresher and periodic saving. Must be called inside a Tokio runtime.
    pub fn start(self: &Arc<Self>) {
        // CCP's static data: checks for a newer build now and then every hour (update() itself only asks CCP every
        // few hours).
        let core = self.clone();
        tokio::spawn(async move {
            loop {
                core.update_static_data(false).await;
                tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            }
        });
        let core = self.clone();
        tokio::spawn(async move {
            let sem = Arc::new(tokio::sync::Semaphore::new(CHARACTERS_AT_ONCE));
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(TICK_SECS));
            loop {
                tick.tick().await;
                for c in core.characters() {
                    if core.refreshing.lock().contains_key(&c.id) || core.due_domains(&c, false).is_empty() {
                        continue;
                    }
                    let (core, sem) = (core.clone(), sem.clone());
                    tokio::spawn(async move {
                        let _p = sem.acquire().await;
                        core.refresh_character(c.id, false).await;
                    });
                }
                core.flush();
            }
        });
    }
}
