//! Desktop alert rules (configured in Settings → Alerts), ported from the original app's AlertHelper.
//!
//! Every check lists the conditions that are true right now, each with a stable key (e.g. `queue_empty:<id>`,
//! `new_mail:<mail id>`). An alert is shown only for keys that weren't in the previous check's list, so each event
//! alerts once, a condition that clears and comes back alerts again, and nothing alerts for what was already true
//! the very first time PodStack checked (that check only records the baseline). The list is saved in the
//! `alert_state` document, so restarting doesn't repeat alerts. Types that are switched off are still tracked, so
//! switching one on doesn't alert for everything that happened while it was off.
//!
//! Mail, contracts, industry jobs, planets, market orders, notifications and calendar events are read through
//! their JSON form (`serde_json::to_value(&c.mail)`), field names as ESI sends them, so these rules don't depend on
//! how each domain module shapes its types.

use crate::character::Character;
use crate::{fmt, store, Core, Event};
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Settings key (`core.setting::<AlertSettings>(SETTINGS_KEY)`).
pub const SETTINGS_KEY: &str = "alerts";
/// Document holding what already fired and the recent-alerts log.
pub const STATE_DOC: &str = "alert_state";
/// How often the alerts service checks.
pub const CHECK_INTERVAL_SECS: u64 = 60;
const LOG_SIZE: usize = 50;
/// Above this many new alerts at once, one summary notification is shown instead.
pub const MAX_SEPARATE: usize = 3;

pub struct AlertType {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub default_on: bool,
}

pub const ALERT_TYPES: &[AlertType] = &[
    AlertType { id: "queue_empty", label: "Training stopped", description: "A skill queue runs out.", default_on: true },
    AlertType {
        id: "queue_low",
        label: "Skill queue running low",
        description: "A skill queue has less time left than the threshold.",
        default_on: true,
    },
    AlertType { id: "omega_lapsed", label: "Lapsed to Alpha", description: "A character that was Omega is now Alpha.", default_on: true },
    AlertType {
        id: "injector_ready",
        label: "Injector ready",
        description: "An SP farm has enough SP for another injector.",
        default_on: true,
    },
    AlertType { id: "contract_finished", label: "Contract completed", description: "One of your contracts is completed.", default_on: true },
    AlertType { id: "new_mail", label: "New EVE mail", description: "An unread mail arrives.", default_on: false },
    AlertType { id: "fatigue_ended", label: "Jump fatigue ended", description: "A character's jump fatigue has worn off.", default_on: true },
    AlertType {
        id: "industry_ready",
        label: "Industry job ready",
        description: "An industry job has finished and can be delivered.",
        default_on: true,
    },
    AlertType {
        id: "extractor_expired",
        label: "PI extractor stopped",
        description: "A planetary extractor has finished its cycle.",
        default_on: true,
    },
    AlertType {
        id: "market_order",
        label: "Market order finished",
        description: "A buy or sell order is filled or expires.",
        default_on: true,
    },
    AlertType {
        id: "eve_notification",
        label: "New EVE notification",
        description: "An unread in-game notification arrives (structure attacks, war declarations, …).",
        default_on: false,
    },
    AlertType {
        id: "calendar_event",
        label: "Calendar event soon",
        description: "A calendar event starts within the hour.",
        default_on: true,
    },
];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AlertSettings {
    /// Show desktop notifications at all (conditions are tracked either way).
    pub enabled: bool,
    pub queue_low_hours: u32,
    /// Per alert type; missing = the type's default.
    pub types: BTreeMap<String, bool>,
}

impl Default for AlertSettings {
    fn default() -> Self {
        AlertSettings { enabled: true, queue_low_hours: 24, types: BTreeMap::new() }
    }
}

impl AlertSettings {
    pub fn is_on(&self, id: &str) -> bool {
        self.types
            .get(id)
            .copied()
            .unwrap_or_else(|| ALERT_TYPES.iter().find(|t| t.id == id).map(|t| t.default_on).unwrap_or(false))
    }

    pub fn set(&mut self, id: &str, on: bool) {
        self.types.insert(id.to_string(), on);
    }
}

/// Something that's true right now.
#[derive(Clone, Debug, PartialEq)]
pub struct Condition {
    /// `<type>:<what>`, stable while the condition lasts.
    pub key: String,
    pub kind: &'static str,
    pub character_id: i64,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct LogEntry {
    pub time: DateTime<Utc>,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub character_id: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AlertState {
    /// Keys true at the last check; `None` before the first check.
    #[serde(default)]
    pub active: Option<Vec<String>>,
    /// Characters known at the last check (a newly added one only records its baseline).
    #[serde(default)]
    pub characters: Vec<i64>,
    /// Recent alerts, newest first.
    #[serde(default)]
    pub log: Vec<LogEntry>,
}

/// What a check found to tell the user.
#[derive(Clone, Debug, PartialEq)]
pub enum Notice {
    /// One notification per alert.
    Separate(Vec<Condition>),
    /// Too many at once: a single summary.
    Summary { title: String, body: String },
}

/// Inputs from other features: SP farms (character id → base SP).
#[derive(Clone, Debug, Default)]
pub struct Extras {
    pub farms: HashMap<i64, i64>,
}

fn countdown(d: Duration) -> String {
    fmt::duration(d, 2)
}

/// A domain's data as JSON (records in it are read by field name).
fn json<T: Serialize>(v: &Option<T>) -> Value {
    v.as_ref().and_then(|v| serde_json::to_value(v).ok()).unwrap_or(Value::Null)
}

/// The list of records in a domain's JSON: the value itself when it's a list, else the first of `keys` that holds
/// a list, else its first list field.
fn records<'a>(v: &'a Value, keys: &[&str]) -> Vec<&'a Value> {
    match v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(o) => keys
            .iter()
            .find_map(|k| o.get(*k).and_then(|x| x.as_array()))
            .or_else(|| o.values().find_map(|x| x.as_array()))
            .map(|a| a.iter().collect())
            .unwrap_or_default(),
        _ => vec![],
    }
}

fn s<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_str())).filter(|x| !x.is_empty())
}

fn id_of(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| match v.get(*k) {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    })
}

fn date(v: &Value, keys: &[&str]) -> Option<DateTime<Utc>> {
    s(v, keys).and_then(|x| DateTime::parse_from_rfc3339(x).ok()).map(|d| d.with_timezone(&Utc))
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(|x| x.as_bool()).unwrap_or(false)
}

fn int_list(v: &Value, key: &str) -> Vec<i64> {
    v.get(key).and_then(|x| x.as_array()).map(|a| a.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default()
}

/// Ids a character's page marked as seen (`Character::local` key holding a list of ids).
// TODO(mail/notifications pages): the key names are a guess until those pages decide where "Mark all read" is kept.
fn seen_ids(c: &Character, keys: &[&str]) -> HashSet<String> {
    keys.iter()
        .filter_map(|k| c.local.get(*k))
        .filter_map(|v| v.as_array())
        .flatten()
        .filter_map(|x| match x {
            Value::Number(n) => Some(n.to_string()),
            Value::String(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

fn industry_activity(id: i64) -> &'static str {
    match id {
        1 => "Manufacturing",
        3 => "TE research",
        4 => "ME research",
        5 => "Copying",
        7 => "Reverse engineering",
        8 => "Invention",
        9 | 11 => "Reaction",
        _ => "Industry",
    }
}

/// The conditions true now for one character.
pub fn character_conditions(c: &Character, settings: &AlertSettings, extras: &Extras, now: DateTime<Utc>) -> Vec<Condition> {
    let mut out = vec![];
    if !c.has_basic_info() {
        return out;
    }
    let name = c.name();
    let mut add = |kind: &'static str, what: String, title: String, body: String| {
        out.push(Condition { key: format!("{kind}:{what}"), kind, character_id: c.id, title, body });
    };

    // skill queue: empty, or ending within the threshold
    let queue_end = c.queue().iter().filter_map(|q| q.finish_date).filter(|f| *f > now).max();
    match queue_end {
        None => {
            if c.skill_queue.as_ref().map(|q| !q.is_empty()).unwrap_or(false) || c.is_omega() == Some(true) {
                add("queue_empty", c.id.to_string(), format!("{name}: training stopped"), "The skill queue is empty.".into());
            }
        }
        Some(end) => {
            let left = end - now;
            if left < Duration::hours(settings.queue_low_hours as i64) {
                add("queue_low", c.id.to_string(), format!("{name}: skill queue running low"), format!("Training ends in {}.", countdown(left)));
            }
        }
    }

    if c.is_omega() == Some(false) {
        add("omega_lapsed", c.id.to_string(), format!("{name} is now Alpha"), "Omega has lapsed, so training has slowed or stopped.".into());
    }

    // one key per injector, so extracting (fewer ready) doesn't alert but each new one does
    if let Some(base) = extras.farms.get(&c.id) {
        let ready = c.injectors_ready(*base);
        for n in 1..=ready {
            add(
                "injector_ready",
                format!("{}:{n}", c.id),
                format!("{name}: injector ready"),
                format!("{ready} injector{} can be extracted.", if ready == 1 { "" } else { "s" }),
            );
        }
    }

    let contracts = json(&c.contracts);
    for k in records(&contracts, &["contracts", "items"]) {
        let status = s(k, &["status"]).unwrap_or("");
        let Some(id) = id_of(k, &["contract_id"]) else { continue };
        if matches!(status, "finished" | "finished_issuer" | "finished_contractor") {
            let kind = s(k, &["type"]).unwrap_or("item_exchange").replace('_', " ");
            let body = match s(k, &["title"]) {
                Some(t) => format!("\"{t}\" ({kind})"),
                None => format!("A {kind} contract."),
            };
            add("contract_finished", id, format!("{name}: contract completed"), body);
        }
    }

    let mail = json(&c.mail);
    let seen_mail = seen_ids(c, &["mail.seen", "mail.read"]);
    for m in records(&mail, &["mails", "mail", "headers", "items"]) {
        let Some(id) = id_of(m, &["mail_id"]) else { continue };
        // label 2 is "Sent": the character's own mail
        let unread = !flag(m, "is_read") && !int_list(m, "labels").contains(&2) && !seen_mail.contains(&id);
        if unread {
            let from = s(m, &["from_name", "sender_name"]).unwrap_or("someone");
            add("new_mail", id, format!("{name}: new mail from {from}"), s(m, &["subject"]).unwrap_or("(no subject)").to_string());
        }
    }

    // fatigue that wore off in the last day (the expiry date is part of the key, so each jump alerts once)
    if let Some(expiry) = c.fatigue.as_ref().and_then(|f| f.jump_fatigue_expire_date) {
        if expiry <= now && now - expiry < Duration::hours(24) {
            add("fatigue_ended", format!("{}:{}", c.id, expiry.timestamp()), format!("{name}: jump fatigue ended"), "Ready to jump again.".into());
        }
    }

    // ESI keeps a job "active" until it's delivered, even after its end date
    let industry = json(&c.industry);
    for j in records(&industry, &["jobs", "items"]) {
        let Some(id) = id_of(j, &["job_id"]) else { continue };
        let status = s(j, &["status"]).unwrap_or("");
        let ended = date(j, &["end_date"]).map(|e| e <= now).unwrap_or(false);
        if status == "ready" || (status == "active" && ended) {
            let what = s(j, &["product_name", "product_type_name", "blueprint_name", "blueprint_type_name"]).unwrap_or("job");
            let activity = industry_activity(j.get("activity_id").and_then(|x| x.as_i64()).unwrap_or(0));
            let runs = j.get("runs").and_then(|x| x.as_i64()).unwrap_or(1);
            add("industry_ready", id, format!("{name}: industry job ready"), format!("{activity}: {what} × {runs}"));
        }
    }

    // orders that ended in the last week
    let orders = json(&c.market_orders);
    for o in records(&orders, &["orders", "history", "items"]) {
        let Some(id) = id_of(o, &["order_id"]) else { continue };
        let state = s(o, &["state"]).unwrap_or("");
        if !matches!(state, "filled" | "expired") {
            continue;
        }
        let ended = date(o, &["issued"]).map(|i| i + Duration::days(o.get("duration").and_then(|d| d.as_i64()).unwrap_or(0)));
        if state == "filled" || ended.map(|e| now - e < Duration::days(7)).unwrap_or(false) {
            let side = if flag(o, "is_buy_order") { "buy" } else { "sell" };
            let item = s(o, &["type_name", "name"]).unwrap_or("An order");
            let body = match s(o, &["location_name", "location"]) {
                Some(l) => format!("{item} in {l}"),
                None => item.to_string(),
            };
            add("market_order", id, format!("{name}: {side} order {state}"), body);
        }
    }

    let notifications = json(&c.notifications);
    let seen_notes = seen_ids(c, &["notifications.seen", "notifications.read"]);
    for n in records(&notifications, &["notifications", "items"]) {
        let Some(id) = id_of(n, &["notification_id"]) else { continue };
        if !flag(n, "is_read") && !seen_notes.contains(&id) {
            let kind = s(n, &["title", "type"]).unwrap_or("New notification");
            let body = match s(n, &["sender_name"]) {
                Some(from) => format!("From {from}"),
                None => "New in-game notification.".into(),
            };
            add("eve_notification", id, format!("{name}: {kind}"), body);
        }
    }

    let calendar = json(&c.calendar);
    for e in records(&calendar, &["events", "items"]) {
        let (Some(id), Some(when)) = (id_of(e, &["event_id"]), date(e, &["event_date"])) else { continue };
        let left = when - now;
        if left > Duration::zero() && left <= Duration::hours(1) && s(e, &["event_response"]) != Some("declined") {
            add("calendar_event", id, format!("{name}: {}", s(e, &["title"]).unwrap_or("Calendar event")), format!("Starts in {}.", countdown(left)));
        }
    }

    // colonies with their extractors: either an `extractors` list, or ESI's pins with extractor details
    let planets = json(&c.planets);
    for colony in records(&planets, &["colonies", "planets", "items"]) {
        let planet_id = id_of(colony, &["planet_id"]).unwrap_or_default();
        let planet = s(colony, &["planet_name", "name"]).unwrap_or("a planet");
        let extractors: Vec<&Value> = match colony.get("extractors").and_then(|x| x.as_array()) {
            Some(a) => a.iter().collect(),
            None => colony
                .get("pins")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter(|p| p.get("extractor_details").is_some()).collect())
                .unwrap_or_default(),
        };
        for x in extractors {
            let Some(expiry) = date(x, &["expiry_time"]) else { continue };
            if expiry <= now {
                let pin = id_of(x, &["pin_id"]).unwrap_or_default();
                let product = s(x, &["product_name", "product_type_name"]).unwrap_or("Extractor");
                add(
                    "extractor_expired",
                    format!("{planet_id}:{pin}:{}", expiry.timestamp()),
                    format!("{name}: extractor stopped"),
                    format!("{product} on {planet}."),
                );
            }
        }
    }

    out
}

/// Compares conditions with the previous check's and returns the new ones to alert for (one per type and
/// character), updating `state`. `known_ids` are all current characters.
pub fn evaluate(state: &mut AlertState, conditions: Vec<Condition>, settings: &AlertSettings, known_ids: &[i64], now: DateTime<Utc>) -> Vec<Condition> {
    let previous = state.active.replace(conditions.iter().map(|c| c.key.clone()).collect());
    let known_before: HashSet<i64> = std::mem::replace(&mut state.characters, known_ids.to_vec()).into_iter().collect();
    // first check: only record what's already true
    let Some(previous) = previous else { return vec![] };
    let seen: HashSet<String> = previous.into_iter().collect();

    let mut fresh: Vec<Condition> = vec![];
    for c in conditions {
        // likewise for a newly added character
        if seen.contains(&c.key) || !settings.is_on(c.kind) || !known_before.contains(&c.character_id) {
            continue;
        }
        // one alert per type and character per check (three new injectors are one alert)
        if let Some(i) = fresh.iter().position(|f| f.kind == c.kind && f.character_id == c.character_id) {
            fresh[i] = c;
        } else {
            fresh.push(c);
        }
    }
    let mut entries: Vec<LogEntry> =
        fresh.iter().map(|c| LogEntry { time: now, title: c.title.clone(), body: c.body.clone(), character_id: Some(c.character_id) }).collect();
    entries.append(&mut state.log);
    entries.truncate(LOG_SIZE);
    state.log = entries;
    fresh
}

/// How to show fresh alerts.
pub fn notice(fresh: Vec<Condition>) -> Option<Notice> {
    match fresh.len() {
        0 => None,
        n if n > MAX_SEPARATE => Some(Notice::Summary {
            title: format!("PodStack: {n} alerts"),
            body: fresh.iter().take(5).map(|c| c.title.as_str()).collect::<Vec<_>>().join("\n"),
        }),
        _ => Some(Notice::Separate(fresh)),
    }
}

/// SP farms from the SP farming feature's document, read loosely: `{ "<id>": {"base_sp": n} }`, or a list of
/// `{character_id, base_sp}`, under the document itself or a `farms` field.
// TODO(SP farming): switch to its typed API once it exists.
pub fn farms_from_doc(doc: &Value) -> HashMap<i64, i64> {
    let mut out = HashMap::new();
    let doc = doc.get("farms").unwrap_or(doc);
    let base = |v: &Value| {
        ["base_sp", "baseSp"]
            .iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_i64()))
            .or_else(|| v.as_i64())
            .unwrap_or(crate::character::DEFAULT_FARM_BASE_SP)
    };
    match doc {
        Value::Object(o) => {
            for (k, v) in o {
                if let Ok(id) = k.parse::<i64>() {
                    out.insert(id, base(v));
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                if let Some(id) = ["character_id", "id"].iter().find_map(|k| v.get(*k).and_then(|x| x.as_i64())) {
                    out.insert(id, base(v));
                }
            }
        }
        _ => {}
    }
    out
}

static STATE_LOCK: Mutex<()> = Mutex::new(());

pub fn load_state() -> AlertState {
    let _g = STATE_LOCK.lock();
    store::load_doc(STATE_DOC)
}

/// Runs one check over every character: saves what's true now and returns what to notify (nothing when
/// notifications are off, though the log still records them).
pub fn check(core: &Core, now: DateTime<Utc>) -> Option<Notice> {
    let settings: AlertSettings = core.setting(SETTINGS_KEY);
    let extras = Extras { farms: farms_from_doc(&store::load_doc::<Value>("farms")) };
    let chars = core.characters();
    let conditions: Vec<Condition> = chars.iter().flat_map(|c| character_conditions(c, &settings, &extras, now)).collect();
    let ids: Vec<i64> = chars.iter().map(|c| c.id).collect();
    let fresh = {
        let _g = STATE_LOCK.lock();
        let mut state: AlertState = store::load_doc(STATE_DOC);
        let fresh = evaluate(&mut state, conditions, &settings, &ids, now);
        store::save_doc(STATE_DOC, &state);
        fresh
    };
    if fresh.is_empty() {
        return None;
    }
    core.emit(Event::Doc(STATE_DOC.into()));
    if settings.enabled {
        notice(fresh)
    } else {
        None
    }
}

pub fn clear_log(core: &Core) {
    {
        let _g = STATE_LOCK.lock();
        let mut state: AlertState = store::load_doc(STATE_DOC);
        state.log.clear();
        store::save_doc(STATE_DOC, &state);
    }
    core.emit(Event::Doc(STATE_DOC.into()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::{info, location, skills};

    fn character(id: i64) -> Character {
        let mut c = Character::new(id);
        c.info = Some(info::Info { name: format!("Pilot {id}"), ..Default::default() });
        c
    }

    fn training(c: &mut Character, hours_left: i64) {
        let now = Utc::now();
        c.skill_queue = Some(vec![skills::QueueEntry {
            skill_id: 3300,
            finished_level: 3,
            start_date: Some(now - Duration::hours(1)),
            finish_date: Some(now + Duration::hours(hours_left)),
            training_start_sp: Some(0),
            level_end_sp: Some(8000),
            ..Default::default()
        }]);
    }

    fn kinds(v: &[Condition]) -> Vec<&str> {
        v.iter().map(|c| c.kind).collect()
    }

    #[test]
    fn queue_rules() {
        let s = AlertSettings::default();
        let x = Extras::default();
        let now = Utc::now();
        let mut c = character(1);
        training(&mut c, 100);
        assert!(!kinds(&character_conditions(&c, &s, &x, now)).contains(&"queue_low"));
        training(&mut c, 5);
        assert!(kinds(&character_conditions(&c, &s, &x, now)).contains(&"queue_low"));
        // finished queue entries left over = training stopped
        c.skill_queue.as_mut().unwrap()[0].finish_date = Some(now - Duration::hours(1));
        assert!(kinds(&character_conditions(&c, &s, &x, now)).contains(&"queue_empty"));
        // never loaded = no alert
        c.skill_queue = None;
        assert!(character_conditions(&c, &s, &x, now).is_empty());
    }

    #[test]
    fn fires_once_and_not_on_first_check() {
        let s = AlertSettings::default();
        let now = Utc::now();
        let mut state = AlertState::default();
        let mut c = character(1);
        training(&mut c, 2);
        let conds = character_conditions(&c, &s, &Extras::default(), now);
        // baseline
        assert!(evaluate(&mut state, conds.clone(), &s, &[1], now).is_empty());
        assert!(evaluate(&mut state, conds.clone(), &s, &[1], now).is_empty());
        // clears, comes back: alerts once
        evaluate(&mut state, vec![], &s, &[1], now);
        let fresh = evaluate(&mut state, conds.clone(), &s, &[1], now);
        assert_eq!(kinds(&fresh), vec!["queue_low"]);
        assert_eq!(state.log.len(), 1);
        assert!(evaluate(&mut state, conds, &s, &[1], now).is_empty());
    }

    #[test]
    fn new_character_and_disabled_types_only_record() {
        let mut s = AlertSettings::default();
        let now = Utc::now();
        let mut state = AlertState::default();
        evaluate(&mut state, vec![], &s, &[1], now);
        let mut c2 = character(2);
        training(&mut c2, 2);
        let conds = character_conditions(&c2, &s, &Extras::default(), now);
        // character 2 just added: baseline only
        assert!(evaluate(&mut state, conds.clone(), &s, &[1, 2], now).is_empty());
        // switched off while it happened, then on: no alert for the old condition
        s.set("queue_low", false);
        evaluate(&mut state, vec![], &s, &[1, 2], now);
        assert!(evaluate(&mut state, conds.clone(), &s, &[1, 2], now).is_empty());
        s.set("queue_low", true);
        assert!(evaluate(&mut state, conds, &s, &[1, 2], now).is_empty());
    }

    #[test]
    fn one_alert_per_type_and_character() {
        let s = AlertSettings::default();
        let now = Utc::now();
        let mut state = AlertState { active: Some(vec![]), characters: vec![1], log: vec![] };
        let mk = |n: i64| Condition { key: format!("injector_ready:1:{n}"), kind: "injector_ready", character_id: 1, title: "t".into(), body: format!("{n}") };
        let fresh = evaluate(&mut state, vec![mk(1), mk(2), mk(3)], &s, &[1], now);
        assert_eq!(fresh.len(), 1);
        assert!(matches!(notice(fresh), Some(Notice::Separate(v)) if v.len() == 1));
    }

    #[test]
    fn summary_when_many() {
        let many: Vec<Condition> = (0..5)
            .map(|i| Condition { key: format!("k{i}"), kind: "queue_low", character_id: i, title: format!("T{i}"), body: String::new() })
            .collect();
        assert!(matches!(notice(many), Some(Notice::Summary { title, .. }) if title == "PodStack: 5 alerts"));
    }

    #[test]
    fn fatigue_ended_within_a_day() {
        let s = AlertSettings::default();
        let now = Utc::now();
        let mut c = character(1);
        c.fatigue = Some(location::Fatigue { jump_fatigue_expire_date: Some(now - Duration::hours(2)), ..Default::default() });
        assert!(kinds(&character_conditions(&c, &s, &Extras::default(), now)).contains(&"fatigue_ended"));
        c.fatigue = Some(location::Fatigue { jump_fatigue_expire_date: Some(now + Duration::hours(2)), ..Default::default() });
        assert!(!kinds(&character_conditions(&c, &s, &Extras::default(), now)).contains(&"fatigue_ended"));
    }

    #[test]
    fn reads_records_loosely() {
        let now = Utc::now();
        let ended = (now - Duration::hours(1)).to_rfc3339();
        let jobs = serde_json::json!({"jobs": [
            {"job_id": 7, "status": "active", "end_date": ended, "activity_id": 1, "product_name": "Rifter", "runs": 2},
            {"job_id": 8, "status": "delivered", "end_date": ended},
        ]});
        assert_eq!(records(&jobs, &["jobs"]).len(), 2);
        let mails = serde_json::json!([{"mail_id": 1, "is_read": false, "subject": "Hi"}, {"mail_id": 2, "is_read": true}]);
        assert_eq!(records(&mails, &[]).len(), 2);
        assert_eq!(id_of(&mails[0], &["mail_id"]).as_deref(), Some("1"));
        assert!(date(&jobs["jobs"][0], &["end_date"]).unwrap() <= now);
    }

    #[test]
    fn farm_documents() {
        let a = farms_from_doc(&serde_json::json!({"123": {"base_sp": 6000000}}));
        assert_eq!(a.get(&123), Some(&6_000_000));
        let b = farms_from_doc(&serde_json::json!({"farms": [{"character_id": 5}]}));
        assert_eq!(b.get(&5), Some(&crate::character::DEFAULT_FARM_BASE_SP));
    }

    #[test]
    fn settings_defaults() {
        let s: AlertSettings = serde_json::from_str("{}").unwrap();
        assert!(s.enabled);
        assert!(s.is_on("queue_empty"));
        assert!(!s.is_on("new_mail"));
    }
}
