//! zKillboard's API: a character's killmails, kills they only took part in included.
//!
//! The game hands a killmail only to the victim and to the pilot with the final blow, so ESI's
//! `characters/{id}/killmails/recent` misses every kill a character helped with. zKillboard collects mails from
//! everyone and lists them per character: `GET /api/characterID/{id}/page/{n}/`, newest first, 200 a page, each
//! the mail in ESI's own format plus a `zkb` object with its hash.
//!
//! zKillboard is a volunteer service that publishes no number for its rate limit, only asks clients to avoid bursts,
//! space requests out, send gzip and a descriptive User-Agent, and honor `Retry-After`. So every request here, for
//! all of the user's characters together, goes through one shared pace: one at a time, two seconds apart, and after
//! a 429 or a server error none at all until a growing pause (or the server's `Retry-After`) has passed.

use crate::config;
use crate::features::killboard::EsiKillmail;
use serde::Deserialize;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

const BASE: &str = "https://zkillboard.com/api";
/// Mails per page.
pub const PAGE_SIZE: usize = 200;
/// The deepest page zKillboard serves (20,000 mails).
pub const LAST_PAGE: u32 = 100;
const SPACING: Duration = Duration::from_secs(2);
const FIRST_PAUSE: Duration = Duration::from_secs(60);
const MAX_PAUSE: Duration = Duration::from_secs(3600);

/// One mail of a list: its id and hash, and the mail itself when zKillboard sent it whole.
#[derive(Clone, Debug, PartialEq)]
pub struct ZkillMail {
    pub killmail_id: i64,
    pub hash: String,
    pub mail: Option<EsiKillmail>,
}

#[derive(Deserialize)]
struct Zkb {
    hash: String,
}

#[derive(Deserialize)]
struct Head {
    killmail_id: i64,
    zkb: Zkb,
}

/// A page of zKillboard's list. Entries without an id or hash are skipped; an entry whose mail doesn't parse (or
/// isn't there: older API versions sent only ids) keeps just its id and hash, for fetching from ESI.
pub fn parse_page(body: &str) -> Result<Vec<ZkillMail>, String> {
    let list: Vec<serde_json::Value> = serde_json::from_str(body).map_err(|e| format!("zKillboard sent something else: {e}"))?;
    Ok(list
        .into_iter()
        .filter_map(|v| {
            let head: Head = serde_json::from_value(v.clone()).ok()?;
            let mail = serde_json::from_value::<EsiKillmail>(v).ok().filter(|m| m.killmail_id == head.killmail_id);
            Some(ZkillMail { killmail_id: head.killmail_id, hash: head.zkb.hash, mail })
        })
        .collect())
}

pub fn page_url(character_id: i64, page: u32) -> String {
    if page <= 1 {
        format!("{BASE}/characterID/{character_id}/")
    } else {
        format!("{BASE}/characterID/{character_id}/page/{page}/")
    }
}

#[derive(Debug, PartialEq)]
pub enum PageError {
    /// zKillboard asked us to slow down or is having trouble: try again on a later refresh.
    Busy,
    /// Anything else (network, a 4xx, a page that doesn't parse).
    Failed(String),
}

#[derive(Default)]
struct Pace {
    last: Option<Instant>,
    blocked_until: Option<Instant>,
    pause: Option<Duration>,
}

impl Pace {
    /// The next pause after a refusal: the server's word if it gave one, else double the last (from a minute, up to
    /// an hour).
    fn refused(&mut self, now: Instant, retry_after: Option<Duration>) {
        let pause = retry_after.unwrap_or_else(|| self.pause.map(|p| (p * 2).min(MAX_PAUSE)).unwrap_or(FIRST_PAUSE));
        self.pause = Some(pause);
        self.blocked_until = Some(now + pause);
    }
}

struct Client {
    http: reqwest::Client,
    pace: tokio::sync::Mutex<Pace>,
}

static CLIENT: LazyLock<Client> = LazyLock::new(|| Client {
    http: reqwest::Client::builder().user_agent(config::user_agent()).build().unwrap_or_default(),
    pace: tokio::sync::Mutex::new(Pace::default()),
});

/// One page of a character's mails (page 1 is the newest).
pub async fn page(character_id: i64, page: u32) -> Result<Vec<ZkillMail>, PageError> {
    let c = &*CLIENT;
    // held for the whole request, so requests never overlap and stay a second apart
    let mut pace = c.pace.lock().await;
    let now = Instant::now();
    if pace.blocked_until.is_some_and(|t| t > now) {
        return Err(PageError::Busy);
    }
    if let Some(wait) = pace.last.map(|l| (l + SPACING).saturating_duration_since(now)) {
        tokio::time::sleep(wait).await;
    }
    pace.last = Some(Instant::now());
    let resp = c.http.get(page_url(character_id, page)).timeout(Duration::from_secs(30)).send().await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => return Err(PageError::Failed(e.to_string())),
    };
    let status = resp.status();
    if status.as_u16() == 429 || status.is_server_error() {
        let retry = resp.headers().get("retry-after").and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
        pace.refused(Instant::now(), retry.map(Duration::from_secs));
        tracing::info!("zKillboard answered {status}: pausing for {:?}", pace.pause.unwrap_or_default());
        return Err(PageError::Busy);
    }
    if !status.is_success() {
        return Err(PageError::Failed(format!("zKillboard answered {status}")));
    }
    pace.pause = None;
    let body = resp.text().await.map_err(|e| PageError::Failed(e.to_string()))?;
    parse_page(&body).map_err(PageError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whole_mails_and_bare_ids() {
        let body = r#"[
            {"attackers":[{"character_id":7,"corporation_id":8,"damage_done":100,"final_blow":true,"security_status":0.1,
                           "ship_type_id":587,"weapon_type_id":587}],
             "killmail_id":130000002,"killmail_time":"2026-11-20T20:49:00Z","solar_system_id":30002537,
             "victim":{"character_id":5,"corporation_id":6,"damage_taken":100,"items":[
                 {"flag":27,"item_type_id":2873,"quantity_destroyed":1,"singleton":0}],
                 "position":{"x":1.0,"y":2.0,"z":3.0},"ship_type_id":626},
             "zkb":{"locationID":40000001,"hash":"abc","fittedValue":1.0,"droppedValue":0,"destroyedValue":2.0,
                    "totalValue":3.0,"points":1,"npc":false,"solo":true,"awox":false,"labels":["solo","pvp"]}},
            {"killmail_id":130000001,"zkb":{"hash":"def","totalValue":1.0}},
            {"zkb":{"hash":"no id"}}
        ]"#;
        let list = parse_page(body).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].killmail_id, 130000002);
        assert_eq!(list[0].hash, "abc");
        let m = list[0].mail.as_ref().unwrap();
        assert_eq!(m.victim.ship_type_id, 626);
        assert_eq!(m.victim.items[0].item_type_id, 2873);
        assert!(m.attackers[0].final_blow);
        assert_eq!(list[1], ZkillMail { killmail_id: 130000001, hash: "def".into(), mail: None });
        assert!(parse_page("<html>busy</html>").is_err());
        assert_eq!(parse_page("[]").unwrap(), vec![]);
    }

    #[test]
    fn page_urls() {
        assert_eq!(page_url(5, 1), "https://zkillboard.com/api/characterID/5/");
        assert_eq!(page_url(5, 3), "https://zkillboard.com/api/characterID/5/page/3/");
    }

    #[test]
    fn pauses_grow_after_refusals() {
        let now = Instant::now();
        let mut p = Pace::default();
        p.refused(now, None);
        assert_eq!(p.pause, Some(FIRST_PAUSE));
        p.refused(now, None);
        assert_eq!(p.pause, Some(FIRST_PAUSE * 2));
        for _ in 0..10 {
            p.refused(now, None);
        }
        assert_eq!(p.pause, Some(MAX_PAUSE));
        p.refused(now, Some(Duration::from_secs(5)));
        assert_eq!(p.blocked_until, Some(now + Duration::from_secs(5)));
    }
}
