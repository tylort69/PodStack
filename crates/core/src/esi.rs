//! ESI client with rate limiting.
//!
//! Keeps PodStack inside ESI's limits (https://developers.eveonline.com/docs/services/esi/rate-limiting/):
//! - at most [`MAX_IN_FLIGHT`] requests at once, so many characters refreshing together queue instead of bursting;
//! - per-group token buckets, learned from the `X-Ratelimit-*` headers: when a bucket runs low, requests to it
//!   are spaced out so it refills instead of hitting 429;
//! - 429/420 wait for `Retry-After`; the legacy error limit (`X-ESI-Error-Limit-*`) pauses everything when low;
//! - one retry after a server error or dropped connection. Other 4xx aren't retried: they'd fail again and cost
//!   more of the error budget.
//!
//! ```ignore
//! let skills: Skills = core.esi.get(format!("characters/{id}/skills")).auth(&auth).json().await?;
//! ```

use crate::config;
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;

pub const MAX_IN_FLIGHT: usize = 8;
const SAFETY_SHARE: f64 = 0.15;
const ERROR_LIMIT_FLOOR: i64 = 20;

#[derive(Debug, thiserror::Error)]
pub enum EsiError {
    #[error("the login lacks scope {0}")]
    ScopeMissing(String),
    #[error("ESI returned {status}: {message}")]
    Http { status: u16, message: String },
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("unexpected ESI response: {0}")]
    Decode(String),
    /// The character's token couldn't be refreshed; `permanent` when it never will be (revoked / wrong client).
    #[error("token problem: {message}")]
    Token { message: String, permanent: bool, client: bool },
    #[error("character not authorized")]
    NotAuthorized,
}

impl EsiError {
    pub fn status(&self) -> Option<u16> {
        match self {
            EsiError::Http { status, .. } => Some(*status),
            _ => None,
        }
    }
    pub fn is_not_found(&self) -> bool {
        self.status() == Some(404)
    }
    pub fn is_forbidden(&self) -> bool {
        self.status() == Some(403)
    }
}

pub type Result<T, E = EsiError> = std::result::Result<T, E>;

/// A character's access for one batch of requests (see [`crate::auth::AuthStore::auth`]).
#[derive(Clone, Debug)]
pub struct Auth {
    pub character_id: i64,
    pub token: String,
    pub scopes: Arc<Vec<String>>,
}

impl Auth {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

#[derive(Default)]
struct Bucket {
    limit: i64,
    window: Duration,
    remaining: i64,
    blocked_until: Option<Instant>,
}

#[derive(Default)]
struct LimiterState {
    buckets: HashMap<String, Bucket>,      // "group|who"
    route_groups: HashMap<String, String>, // route pattern -> group
    pause_until: Option<Instant>,
}

/// Snapshot of the limiter for the status bar.
#[derive(Clone, Debug, Default)]
pub struct LimiterStatus {
    pub paused_for: Option<Duration>,
    pub in_flight: usize,
}

pub struct Esi {
    pub http: reqwest::Client,
    sem: Arc<Semaphore>,
    state: Mutex<LimiterState>,
}

/// "characters/123/skills" -> "characters/{id}/skills"
pub fn route_pattern(path: &str) -> String {
    path.trim_matches('/')
        .split('/')
        .map(|seg| if !seg.is_empty() && seg.chars().all(|c| c.is_ascii_digit()) { "{id}" } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

/// "150/15m" -> (150, 15 minutes)
pub fn parse_limit(v: &str) -> Option<(i64, Duration)> {
    let (n, w) = v.trim().split_once('/')?;
    let n: i64 = n.parse().ok()?;
    let (num, unit) = w.split_at(w.len().checked_sub(1)?);
    let num: u64 = num.parse().ok()?;
    let secs = match unit {
        "s" => num,
        "m" => num * 60,
        "h" => num * 3600,
        "d" => num * 86400,
        _ => return None,
    };
    Some((n, Duration::from_secs(secs)))
}

impl Esi {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(config::user_agent())
            .gzip(true)
            .timeout(Duration::from_secs(60))
            .build()
            .expect("http client");
        Esi { http, sem: Arc::new(Semaphore::new(MAX_IN_FLIGHT)), state: Mutex::new(LimiterState::default()) }
    }

    pub fn status(&self) -> LimiterStatus {
        let st = self.state.lock();
        LimiterStatus {
            paused_for: st.pause_until.and_then(|t| t.checked_duration_since(Instant::now())),
            in_flight: MAX_IN_FLIGHT - self.sem.available_permits(),
        }
    }

    pub fn get(&self, path: impl Into<String>) -> Request<'_> {
        Request::new(self, reqwest::Method::GET, path.into())
    }

    pub fn post(&self, path: impl Into<String>) -> Request<'_> {
        Request::new(self, reqwest::Method::POST, path.into())
    }

    fn wait_needed(&self, route: &str, who: &str) -> Option<Duration> {
        let st = self.state.lock();
        let now = Instant::now();
        let mut wait = st.pause_until.and_then(|t| t.checked_duration_since(now));
        if let Some(group) = st.route_groups.get(route) {
            if let Some(b) = st.buckets.get(&format!("{group}|{who}")) {
                if let Some(until) = b.blocked_until.and_then(|t| t.checked_duration_since(now)) {
                    wait = Some(wait.map_or(until, |w| w.max(until)));
                } else if b.limit > 0 && (b.remaining as f64) < b.limit as f64 * SAFETY_SHARE {
                    // spend at the rate tokens come back
                    let pace = b.window / (b.limit.max(1) as u32);
                    wait = Some(wait.map_or(pace, |w| w.max(pace)));
                }
            }
        }
        wait
    }

    fn record(&self, route: &str, who: &str, status: u16, headers: &reqwest::header::HeaderMap) -> Option<Duration> {
        let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(str::to_string);
        let mut st = self.state.lock();
        let now = Instant::now();
        let retry_after = h("retry-after").and_then(|v| v.parse::<u64>().ok()).map(Duration::from_secs);

        if let Some(group) = h("x-ratelimit-group") {
            st.route_groups.insert(route.to_string(), group.clone());
            let b = st.buckets.entry(format!("{group}|{who}")).or_default();
            if let Some((limit, window)) = h("x-ratelimit-limit").as_deref().and_then(parse_limit) {
                b.limit = limit;
                b.window = window;
            }
            if let Some(rem) = h("x-ratelimit-remaining").and_then(|v| v.parse().ok()) {
                b.remaining = rem;
            }
            if status == 429 {
                b.blocked_until = Some(now + retry_after.unwrap_or(Duration::from_secs(60)));
            }
        }

        let err_remain = h("x-esi-error-limit-remain").and_then(|v| v.parse::<i64>().ok());
        let err_reset = h("x-esi-error-limit-reset").and_then(|v| v.parse::<u64>().ok());
        if status == 420 {
            st.pause_until = Some(now + Duration::from_secs(err_reset.unwrap_or(60)));
        } else if let (Some(rem), Some(reset)) = (err_remain, err_reset) {
            if rem < ERROR_LIMIT_FLOOR {
                tracing::warn!("ESI error limit low ({rem} left), pausing {reset}s");
                st.pause_until = Some(now + Duration::from_secs(reset + 1));
            }
        }

        match status {
            429 | 420 => Some(retry_after.unwrap_or(Duration::from_secs(60))),
            _ => None,
        }
    }
}

impl Default for Esi {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Request<'a> {
    esi: &'a Esi,
    method: reqwest::Method,
    path: String,
    auth: Option<&'a Auth>,
    scope: Option<&'a str>,
    query: Vec<(String, String)>,
    body: Option<serde_json::Value>,
}

pub struct Response {
    pub body: String,
    pub pages: u32,
}

impl<'a> Request<'a> {
    fn new(esi: &'a Esi, method: reqwest::Method, path: String) -> Self {
        Request { esi, method, path, auth: None, scope: None, query: vec![], body: None }
    }

    pub fn auth(mut self, auth: &'a Auth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// Fails with [`EsiError::ScopeMissing`] (without calling ESI) unless the login has this scope.
    pub fn scope(mut self, scope: &'a str) -> Self {
        self.scope = Some(scope);
        self
    }

    pub fn query(mut self, key: &str, value: impl ToString) -> Self {
        self.query.push((key.to_string(), value.to_string()));
        self
    }

    pub fn body(mut self, body: impl serde::Serialize) -> Self {
        self.body = Some(serde_json::to_value(body).unwrap_or_default());
        self
    }

    fn clone_with_page(&self, page: u32) -> Request<'a> {
        let mut query: Vec<_> = self.query.iter().filter(|(k, _)| k != "page").cloned().collect();
        query.push(("page".into(), page.to_string()));
        Request {
            esi: self.esi,
            method: self.method.clone(),
            path: self.path.clone(),
            auth: self.auth,
            scope: self.scope,
            query,
            body: self.body.clone(),
        }
    }

    pub async fn send(&self) -> Result<Response> {
        if let Some(scope) = self.scope {
            let ok = self.auth.map(|a| a.has_scope(scope)).unwrap_or(false);
            if !ok {
                return Err(EsiError::ScopeMissing(scope.to_string()));
            }
        }
        let route = route_pattern(&self.path);
        let who = self.auth.map(|a| a.character_id.to_string()).unwrap_or_else(|| "public".into());
        let url = format!("{}/{}", config::ESI_BASE, self.path.trim_matches('/'));

        let mut attempt = 0;
        loop {
            attempt += 1;
            while let Some(wait) = self.esi.wait_needed(&route, &who) {
                tokio::time::sleep(wait.min(Duration::from_secs(120))).await;
            }
            let _permit = self.esi.sem.acquire().await.expect("semaphore");
            let mut req = self
                .esi
                .http
                .request(self.method.clone(), &url)
                .header("Accept", "application/json")
                .header("X-Compatibility-Date", config::ESI_COMPATIBILITY_DATE)
                .header("X-Tenant", config::ESI_TENANT)
                .query(&self.query);
            if let Some(a) = self.auth {
                req = req.bearer_auth(&a.token);
            }
            if let Some(b) = &self.body {
                req = req.json(b);
            }
            tracing::debug!("ESI {} {}", self.method, self.path);
            let res = match req.send().await {
                Ok(r) => r,
                Err(err) => {
                    drop(_permit);
                    if attempt >= 2 {
                        return Err(err.into());
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
            };
            let status = res.status().as_u16();
            let retry = self.esi.record(&route, &who, status, res.headers());
            let pages = res
                .headers()
                .get("x-pages")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse().ok())
                .unwrap_or(1);
            let body = res.text().await.unwrap_or_default();
            drop(_permit);

            if (200..300).contains(&status) {
                return Ok(Response { body, pages });
            }
            let retryable = status >= 500 || retry.is_some();
            if retryable && attempt < 2 {
                tracing::warn!("ESI {} {} failed ({status}), retrying", self.method, self.path);
                tokio::time::sleep(retry.unwrap_or(Duration::from_secs(2))).await;
                continue;
            }
            let message = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|v| v["error"].as_str().map(String::from))
                .unwrap_or(body);
            return Err(EsiError::Http { status, message });
        }
    }

    pub async fn json<T: DeserializeOwned>(self) -> Result<T> {
        let res = self.send().await?;
        parse_body(&res.body)
    }

    /// GETs every page of a paginated endpoint (ESI reports the count in X-Pages) and concatenates them.
    pub async fn all_pages<T: DeserializeOwned>(self) -> Result<Vec<T>> {
        let first = self.clone_with_page(1).send().await?;
        let mut out: Vec<T> = parse_body(&first.body)?;
        for page in 2..=first.pages {
            let res = self.clone_with_page(page).send().await?;
            out.extend(parse_body::<Vec<T>>(&res.body)?);
        }
        Ok(out)
    }
}

fn parse_body<T: DeserializeOwned>(body: &str) -> Result<T> {
    let text = if body.trim().is_empty() { "null" } else { body };
    serde_json::from_str(text)
        .or_else(|e| if text == "null" { serde_json::from_str("[]") } else { Err(e) })
        .map_err(|e| EsiError::Decode(format!("{e}: {}", &text[..text.len().min(200)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_patterns() {
        assert_eq!(route_pattern("/characters/123/skills/"), "characters/{id}/skills");
        assert_eq!(route_pattern("universe/names"), "universe/names");
    }

    #[test]
    fn limits() {
        assert_eq!(parse_limit("150/15m"), Some((150, Duration::from_secs(900))));
        assert_eq!(parse_limit("bad"), None);
    }
}
