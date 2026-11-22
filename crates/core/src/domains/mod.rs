//! One module per kind of character data. Each module exports `DOMAINS`, a list of [`DomainSpec`]s: a key (used in
//! [`Character::refreshes`]), a label for the ESI page, a refresh interval and an async fetch function.
//!
//! A fetch gets a [`Cx`] (the core, the character id, a fresh access token and the character's data as it was before
//! this refresh) and returns an [`Apply`]: a closure that writes the result into the character. Fetches run
//! concurrently and never hold a lock while waiting on ESI.
//!
//! Return `Err(EsiError::ScopeMissing(..))` (what [`crate::esi::Request::scope`] gives) when the login lacks a scope:
//! the scheduler then stops retrying that data until the character is re-authorized.

use crate::character::Character;
use crate::esi::{Auth, Esi, EsiError, Request};
use crate::universe::Universe;
use crate::Core;
use futures::future::BoxFuture;
use std::sync::Arc;

pub mod info;
pub mod skills;
pub mod clones;
pub mod location;

pub mod social;
pub mod mail;
pub mod contracts;
pub mod wallet;
pub mod assets;
pub mod industry;

pub type Apply = Box<dyn FnOnce(&mut Character) + Send>;
pub type DomainResult = Result<Apply, EsiError>;

pub struct DomainSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub interval_secs: i64,
    pub run: fn(Cx) -> BoxFuture<'static, DomainResult>,
}

/// `domain!("key", "Label", seconds, path::to::async_fn)` builds a [`DomainSpec`] for `async fn f(cx: &Cx) -> DomainResult`.
#[macro_export]
macro_rules! domain {
    ($key:literal, $label:literal, $interval:expr, $f:path) => {
        $crate::domains::DomainSpec {
            key: $key,
            label: $label,
            interval_secs: $interval,
            run: |cx| Box::pin(async move { $f(&cx).await }),
        }
    };
}

/// Everything a fetch needs.
#[derive(Clone)]
pub struct Cx {
    pub core: Arc<Core>,
    pub id: i64,
    pub auth: Auth,
    /// The character's data before this refresh (e.g. to keep killmails already loaded).
    pub prev: Arc<Character>,
}

impl Cx {
    pub fn esi(&self) -> &Esi {
        &self.core.esi
    }
    pub fn universe(&self) -> &Universe {
        &self.core.universe
    }
    /// An authenticated request for this character.
    pub fn get(&self, path: impl Into<String>) -> Request<'_> {
        self.core.esi.get(path).auth(&self.auth)
    }
    pub fn post(&self, path: impl Into<String>) -> Request<'_> {
        self.core.esi.post(path).auth(&self.auth)
    }
    /// `characters/<id>/<rest>`
    pub fn char_path(&self, rest: &str) -> String {
        format!("characters/{}/{}", self.id, rest.trim_start_matches('/'))
    }
}

/// All data kinds, in the order the ESI page lists them.
pub fn all() -> Vec<&'static DomainSpec> {
    [
        info::DOMAINS,
        skills::DOMAINS,
        clones::DOMAINS,
        location::DOMAINS,
        wallet::DOMAINS,
        mail::DOMAINS,
        contracts::DOMAINS,
        assets::DOMAINS,
        industry::DOMAINS,
        social::DOMAINS,
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub fn label(key: &str) -> &'static str {
    all().into_iter().find(|d| d.key == key).map(|d| d.label).unwrap_or("Other")
}
