//! Loyalty points, standings, research agents, EVE notifications, calendar, killmails, medals, contacts and factional
//! warfare.
//!
//! Each fetch turns ESI's records into display-ready ones (names resolved) with a pure `build_*` function, so the
//! shaping can be tested against sample ESI JSON without a network.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const DOMAINS: &[DomainSpec] = &[
    domain!("loyalty_points", "Loyalty Points", 3600, refresh_loyalty),
    domain!("standings", "Standings", 3600, refresh_standings),
    domain!("research_agents", "Research Agents", 3600, refresh_research),
    // ESI caches notifications for 10 minutes and the calendar for 5
    domain!("notifications", "EVE Notifications", 600, refresh_notifications),
    domain!("calendar", "Calendar", 1800, refresh_calendar),
    domain!("killmails", "Killmails", 3600, refresh_killmails),
    domain!("medals", "Medals", 21600, refresh_medals),
    domain!("contacts", "Contacts", 3600, refresh_contacts),
    domain!("fw_stats", "Factional Warfare", 3600, refresh_fw),
];

/// How many recent killmails are kept.
pub const KILLMAIL_LIMIT: usize = 50;

// ---------- loyalty points ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct LoyaltyPoints {
    pub corporation_id: i64,
    pub corporation_name: String,
    pub loyalty_points: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LoyaltyData {
    /// Corporations the character has LP with (more than 0), by name.
    pub points: Vec<LoyaltyPoints>,
}

#[derive(Deserialize)]
struct RawLoyalty {
    corporation_id: i64,
    loyalty_points: i64,
}

fn build_loyalty(raw: Vec<RawLoyalty>, names: &HashMap<i64, String>) -> LoyaltyData {
    let mut points: Vec<LoyaltyPoints> = raw
        .into_iter()
        .filter(|l| l.loyalty_points > 0)
        .map(|l| LoyaltyPoints {
            corporation_id: l.corporation_id,
            corporation_name: names.get(&l.corporation_id).cloned().unwrap_or_else(|| format!("Corporation #{}", l.corporation_id)),
            loyalty_points: l.loyalty_points,
        })
        .collect();
    points.sort_by_key(|p| p.corporation_name.to_lowercase());
    LoyaltyData { points }
}

async fn refresh_loyalty(cx: &Cx) -> DomainResult {
    let raw: Vec<RawLoyalty> = cx.get(cx.char_path("loyalty/points")).scope("esi-characters.read_loyalty.v1").json().await?;
    let names = cx.universe().names(cx.esi(), raw.iter().map(|l| l.corporation_id).collect::<Vec<_>>()).await;
    let data = build_loyalty(raw, &names);
    Ok(Box::new(move |c| c.loyalty = Some(data)))
}

// ---------- standings ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Standing {
    pub from_id: i64,
    /// "agent", "npc_corp" or "faction"
    pub from_type: String,
    pub standing: f64,
    pub name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StandingsData {
    pub standings: Vec<Standing>,
}

#[derive(Deserialize)]
struct RawStanding {
    from_id: i64,
    from_type: String,
    standing: f64,
}

fn build_standings(raw: Vec<RawStanding>, names: &HashMap<i64, String>) -> StandingsData {
    let standings = raw
        .into_iter()
        .map(|s| Standing {
            name: names.get(&s.from_id).cloned().unwrap_or_else(|| format!("#{}", s.from_id)),
            from_id: s.from_id,
            from_type: s.from_type,
            standing: s.standing,
        })
        .collect();
    StandingsData { standings }
}

async fn refresh_standings(cx: &Cx) -> DomainResult {
    let raw: Vec<RawStanding> = cx.get(cx.char_path("standings")).scope("esi-characters.read_standings.v1").json().await?;
    let names = cx.universe().names(cx.esi(), raw.iter().map(|s| s.from_id).collect::<Vec<_>>()).await;
    let data = build_standings(raw, &names);
    Ok(Box::new(move |c| c.standings = Some(data)))
}

// ---------- research agents ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ResearchAgent {
    pub agent_id: i64,
    pub agent_name: String,
    pub skill_type_id: i64,
    pub skill_name: String,
    pub started_at: DateTime<Utc>,
    pub points_per_day: f64,
    /// Research points there were when the research started.
    #[serde(default)]
    pub remainder_points: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResearchData {
    pub agents: Vec<ResearchAgent>,
}

#[derive(Deserialize)]
struct RawAgent {
    agent_id: i64,
    skill_type_id: i64,
    started_at: DateTime<Utc>,
    points_per_day: f64,
    #[serde(default)]
    remainder_points: f64,
}

fn build_research(raw: Vec<RawAgent>, names: &HashMap<i64, String>) -> ResearchData {
    let agents = raw
        .into_iter()
        .map(|a| ResearchAgent {
            agent_name: names.get(&a.agent_id).cloned().unwrap_or_else(|| format!("Agent #{}", a.agent_id)),
            skill_name: crate::sde::SKILLS
                .get(a.skill_type_id)
                .map(|s| s.name.clone())
                .or_else(|| names.get(&a.skill_type_id).cloned())
                .unwrap_or_else(|| format!("Skill #{}", a.skill_type_id)),
            agent_id: a.agent_id,
            skill_type_id: a.skill_type_id,
            started_at: a.started_at,
            points_per_day: a.points_per_day,
            remainder_points: a.remainder_points,
        })
        .collect();
    ResearchData { agents }
}

async fn refresh_research(cx: &Cx) -> DomainResult {
    let raw: Vec<RawAgent> =
        cx.get(cx.char_path("agents_research")).scope("esi-characters.read_agents_research.v1").json().await?;
    let names = cx.universe().names(cx.esi(), raw.iter().flat_map(|a| [a.agent_id, a.skill_type_id]).collect::<Vec<_>>()).await;
    let data = build_research(raw, &names);
    Ok(Box::new(move |c| c.research = Some(data)))
}

// ---------- notifications ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Notification {
    pub notification_id: i64,
    /// EVE's notification type, e.g. "StructureUnderAttack"
    #[serde(rename = "type")]
    pub kind: String,
    pub timestamp: DateTime<Utc>,
    #[serde(default)]
    pub is_read: bool,
    #[serde(default)]
    pub sender_id: i64,
    /// "character", "corporation", "alliance", "faction" or "other"
    #[serde(default)]
    pub sender_type: String,
    #[serde(default)]
    pub sender_name: Option<String>,
    /// The details, as YAML.
    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotificationsData {
    /// Newest first (ESI returns the latest few hundred).
    pub notifications: Vec<Notification>,
}

#[derive(Deserialize)]
struct RawNotification {
    notification_id: i64,
    #[serde(rename = "type")]
    kind: String,
    timestamp: DateTime<Utc>,
    #[serde(default)]
    is_read: Option<bool>,
    #[serde(default)]
    sender_id: i64,
    #[serde(default)]
    sender_type: String,
    #[serde(default)]
    text: Option<String>,
}

/// Senders universe/names knows; a structure or "other" sender would fail the whole lookup.
fn nameable_sender(sender_type: &str) -> bool {
    matches!(sender_type, "character" | "corporation" | "alliance" | "faction")
}

fn build_notifications(raw: Vec<RawNotification>, names: &HashMap<i64, String>) -> NotificationsData {
    let mut notifications: Vec<Notification> = raw
        .into_iter()
        .map(|n| Notification {
            sender_name: if nameable_sender(&n.sender_type) { names.get(&n.sender_id).cloned() } else { None },
            notification_id: n.notification_id,
            kind: n.kind,
            timestamp: n.timestamp,
            is_read: n.is_read == Some(true),
            sender_id: n.sender_id,
            sender_type: n.sender_type,
            text: n.text,
        })
        .collect();
    notifications.sort_by_key(|n| std::cmp::Reverse(n.timestamp));
    NotificationsData { notifications }
}

async fn refresh_notifications(cx: &Cx) -> DomainResult {
    let raw: Vec<RawNotification> =
        cx.get(cx.char_path("notifications")).scope("esi-characters.read_notifications.v1").json().await?;
    let ids: Vec<i64> = raw.iter().filter(|n| nameable_sender(&n.sender_type)).map(|n| n.sender_id).collect();
    let names = cx.universe().names(cx.esi(), ids).await;
    let data = build_notifications(raw, &names);
    Ok(Box::new(move |c| c.notifications = Some(data)))
}

// ---------- calendar ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct CalendarEvent {
    pub event_id: i64,
    pub event_date: DateTime<Utc>,
    pub title: String,
    /// 1 = marked important
    #[serde(default)]
    pub importance: i32,
    /// "accepted", "declined", "tentative" or "not_responded"
    #[serde(default)]
    pub event_response: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CalendarData {
    /// Upcoming events (ESI gives the next 50), soonest first.
    pub events: Vec<CalendarEvent>,
}

async fn refresh_calendar(cx: &Cx) -> DomainResult {
    let mut events: Vec<CalendarEvent> =
        cx.get(cx.char_path("calendar")).scope("esi-calendar.read_calendar_events.v1").json().await?;
    events.sort_by_key(|e| e.event_date);
    Ok(Box::new(move |c| c.calendar = Some(CalendarData { events })))
}

// ---------- killmails ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct KillVictim {
    #[serde(default)]
    pub character_id: Option<i64>,
    #[serde(default)]
    pub corporation_id: Option<i64>,
    /// The pilot, else the owning corporation (structures), else "Structure".
    pub name: String,
    #[serde(default)]
    pub corporation: Option<String>,
    #[serde(default)]
    pub alliance: Option<String>,
    pub ship_type_id: i64,
    pub ship: String,
    #[serde(default)]
    pub damage_taken: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct FinalBlow {
    /// The pilot, else the corporation, else "NPC".
    pub name: String,
    #[serde(default)]
    pub ship: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Killmail {
    pub killmail_id: i64,
    pub hash: String,
    pub time: DateTime<Utc>,
    /// The character was the victim.
    pub loss: bool,
    pub victim: KillVictim,
    pub final_blow: FinalBlow,
    pub attackers: usize,
    pub system_id: i64,
    pub system: String,
    #[serde(default)]
    pub security: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KillmailsData {
    /// The latest [`KILLMAIL_LIMIT`] kills and losses, newest first.
    pub kills: Vec<Killmail>,
}

#[derive(Deserialize)]
struct RawRecent {
    killmail_id: i64,
    killmail_hash: String,
}

#[derive(Deserialize, Default)]
struct RawVictim {
    #[serde(default)]
    character_id: Option<i64>,
    #[serde(default)]
    corporation_id: Option<i64>,
    #[serde(default)]
    alliance_id: Option<i64>,
    ship_type_id: i64,
    #[serde(default)]
    damage_taken: i64,
}

#[derive(Deserialize, Default, Clone)]
struct RawAttacker {
    #[serde(default)]
    character_id: Option<i64>,
    #[serde(default)]
    corporation_id: Option<i64>,
    #[serde(default)]
    ship_type_id: Option<i64>,
    #[serde(default)]
    final_blow: bool,
}

#[derive(Deserialize)]
struct RawKillmail {
    killmail_id: i64,
    killmail_time: DateTime<Utc>,
    solar_system_id: i64,
    victim: RawVictim,
    #[serde(default)]
    attackers: Vec<RawAttacker>,
}

impl RawKillmail {
    fn final_blow(&self) -> RawAttacker {
        self.attackers.iter().find(|a| a.final_blow).or(self.attackers.first()).cloned().unwrap_or_default()
    }

    /// Ids whose names the killmail shows.
    fn name_ids(&self) -> Vec<i64> {
        let fb = self.final_blow();
        [
            self.victim.character_id,
            self.victim.corporation_id,
            self.victim.alliance_id,
            Some(self.victim.ship_type_id),
            Some(self.solar_system_id),
            fb.character_id,
            fb.corporation_id,
            fb.ship_type_id,
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

fn build_killmail(km: RawKillmail, hash: String, character_id: i64, names: &HashMap<i64, String>) -> Killmail {
    let name = |id: Option<i64>| id.and_then(|i| names.get(&i).cloned());
    let fb = km.final_blow();
    Killmail {
        killmail_id: km.killmail_id,
        hash,
        time: km.killmail_time,
        loss: km.victim.character_id == Some(character_id),
        victim: KillVictim {
            character_id: km.victim.character_id,
            corporation_id: km.victim.corporation_id,
            name: name(km.victim.character_id).or_else(|| name(km.victim.corporation_id)).unwrap_or_else(|| "Structure".into()),
            corporation: name(km.victim.corporation_id),
            alliance: name(km.victim.alliance_id),
            ship_type_id: km.victim.ship_type_id,
            ship: name(Some(km.victim.ship_type_id)).unwrap_or_else(|| format!("Type #{}", km.victim.ship_type_id)),
            damage_taken: km.victim.damage_taken,
        },
        final_blow: FinalBlow {
            name: name(fb.character_id).or_else(|| name(fb.corporation_id)).unwrap_or_else(|| "NPC".into()),
            ship: name(fb.ship_type_id),
        },
        attackers: km.attackers.len(),
        system_id: km.solar_system_id,
        system: name(Some(km.solar_system_id)).unwrap_or_else(|| format!("System #{}", km.solar_system_id)),
        security: None,
    }
}

/// The newest [`KILLMAIL_LIMIT`] of the recent list.
fn latest(mut recent: Vec<RawRecent>) -> Vec<RawRecent> {
    recent.sort_by_key(|r| std::cmp::Reverse(r.killmail_id));
    recent.truncate(KILLMAIL_LIMIT);
    recent
}

// A killmail never changes, so ones already loaded are kept as they are and only new ones are fetched.
async fn refresh_killmails(cx: &Cx) -> DomainResult {
    let recent: Vec<RawRecent> =
        cx.get(cx.char_path("killmails/recent")).scope("esi-killmails.read_killmails.v1").all_pages().await?;
    let recent = latest(recent);
    let mut known: HashMap<i64, Killmail> =
        cx.prev.killmails.as_ref().map(|k| k.kills.iter().map(|k| (k.killmail_id, k.clone())).collect()).unwrap_or_default();

    let new: Vec<&RawRecent> = recent.iter().filter(|r| !known.contains_key(&r.killmail_id)).collect();
    let fetched: Vec<(RawKillmail, String)> = futures::future::join_all(new.iter().map(|r| async move {
        cx.esi()
            .get(format!("killmails/{}/{}", r.killmail_id, r.killmail_hash))
            .json::<RawKillmail>()
            .await
            .ok()
            .map(|km| (km, r.killmail_hash.clone()))
    }))
    .await
    .into_iter()
    .flatten()
    .collect();

    let names = cx.universe().names(cx.esi(), fetched.iter().flat_map(|(km, _)| km.name_ids()).collect::<Vec<_>>()).await;
    let systems: HashSet<i64> = fetched.iter().map(|(km, _)| km.solar_system_id).collect();
    let mut security = HashMap::new();
    for s in systems {
        if let Ok(info) = cx.universe().system(cx.esi(), s).await {
            security.insert(s, info.display_security());
        }
    }
    for (km, hash) in fetched {
        let mut k = build_killmail(km, hash, cx.id, &names);
        k.security = security.get(&k.system_id).copied();
        known.insert(k.killmail_id, k);
    }
    let kills: Vec<Killmail> = recent.iter().filter_map(|r| known.remove(&r.killmail_id)).collect();
    Ok(Box::new(move |c| c.killmails = Some(KillmailsData { kills })))
}

// ---------- medals ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Medal {
    pub medal_id: i64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub reason: String,
    pub date: DateTime<Utc>,
    /// "public" or "private"
    #[serde(default)]
    pub status: String,
    pub corporation_id: i64,
    #[serde(default)]
    pub corporation: Option<String>,
    #[serde(default)]
    pub issuer: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MedalsData {
    /// Newest first.
    pub medals: Vec<Medal>,
}

#[derive(Deserialize)]
struct RawMedal {
    medal_id: i64,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    reason: String,
    date: DateTime<Utc>,
    #[serde(default)]
    status: String,
    corporation_id: i64,
    #[serde(default)]
    issuer_id: Option<i64>,
}

fn build_medals(raw: Vec<RawMedal>, names: &HashMap<i64, String>) -> MedalsData {
    let mut medals: Vec<Medal> = raw
        .into_iter()
        .map(|m| Medal {
            corporation: names.get(&m.corporation_id).cloned(),
            issuer: m.issuer_id.and_then(|i| names.get(&i).cloned()),
            medal_id: m.medal_id,
            title: m.title,
            description: m.description,
            reason: m.reason,
            date: m.date,
            status: m.status,
            corporation_id: m.corporation_id,
        })
        .collect();
    medals.sort_by_key(|m| std::cmp::Reverse(m.date));
    MedalsData { medals }
}

async fn refresh_medals(cx: &Cx) -> DomainResult {
    let raw: Vec<RawMedal> = cx.get(cx.char_path("medals")).scope("esi-characters.read_medals.v1").json().await?;
    let ids: Vec<i64> = raw.iter().flat_map(|m| [Some(m.corporation_id), m.issuer_id]).flatten().collect();
    let names = cx.universe().names(cx.esi(), ids).await;
    let data = build_medals(raw, &names);
    Ok(Box::new(move |c| c.medals = Some(data)))
}

// ---------- contacts ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Contact {
    pub contact_id: i64,
    /// "character", "corporation", "alliance" or "faction"
    pub contact_type: String,
    pub name: String,
    pub standing: f64,
    #[serde(default)]
    pub is_watched: bool,
    #[serde(default)]
    pub is_blocked: bool,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContactsData {
    pub contacts: Vec<Contact>,
}

#[derive(Deserialize)]
struct RawContact {
    contact_id: i64,
    contact_type: String,
    standing: f64,
    #[serde(default)]
    is_watched: Option<bool>,
    #[serde(default)]
    is_blocked: Option<bool>,
    #[serde(default)]
    label_ids: Vec<i64>,
}

#[derive(Deserialize)]
struct RawLabel {
    label_id: i64,
    label_name: String,
}

fn build_contacts(raw: Vec<RawContact>, labels: Vec<RawLabel>, names: &HashMap<i64, String>) -> ContactsData {
    let label_names: HashMap<i64, String> = labels.into_iter().map(|l| (l.label_id, l.label_name)).collect();
    let contacts = raw
        .into_iter()
        .map(|c| Contact {
            name: names.get(&c.contact_id).cloned().unwrap_or_else(|| format!("#{}", c.contact_id)),
            labels: c.label_ids.iter().filter_map(|id| label_names.get(id).cloned()).collect(),
            contact_id: c.contact_id,
            contact_type: c.contact_type,
            standing: c.standing,
            is_watched: c.is_watched == Some(true),
            is_blocked: c.is_blocked == Some(true),
        })
        .collect();
    ContactsData { contacts }
}

async fn refresh_contacts(cx: &Cx) -> DomainResult {
    const SCOPE: &str = "esi-characters.read_contacts.v1";
    let raw: Vec<RawContact> = cx.get(cx.char_path("contacts")).scope(SCOPE).all_pages().await?;
    // labels are a nicety: contacts still show without them
    let labels: Vec<RawLabel> = cx.get(cx.char_path("contacts/labels")).scope(SCOPE).json().await.unwrap_or_default();
    let names = cx.universe().names(cx.esi(), raw.iter().map(|c| c.contact_id).collect::<Vec<_>>()).await;
    let data = build_contacts(raw, labels, &names);
    Ok(Box::new(move |c| c.contacts = Some(data)))
}

// ---------- factional warfare ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct FwCounts {
    #[serde(default)]
    pub total: i64,
    #[serde(default)]
    pub last_week: i64,
    #[serde(default)]
    pub yesterday: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FwData {
    /// Missing when not enlisted.
    #[serde(default)]
    pub faction_id: Option<i64>,
    #[serde(default)]
    pub faction: Option<String>,
    #[serde(default)]
    pub enlisted_on: Option<DateTime<Utc>>,
    #[serde(default)]
    pub current_rank: Option<i32>,
    #[serde(default)]
    pub highest_rank: Option<i32>,
    #[serde(default)]
    pub kills: FwCounts,
    #[serde(default)]
    pub victory_points: FwCounts,
}

impl FwData {
    pub fn enlisted(&self) -> bool {
        self.faction_id.is_some()
    }
}

async fn refresh_fw(cx: &Cx) -> DomainResult {
    let mut fw: FwData = cx.get(cx.char_path("fw/stats")).scope("esi-characters.read_fw_stats.v1").json().await?;
    if let Some(f) = fw.faction_id {
        fw.faction = cx.universe().name(cx.esi(), f).await;
    }
    Ok(Box::new(move |c| c.fw = Some(fw)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[(i64, &str)]) -> HashMap<i64, String> {
        list.iter().map(|(i, n)| (*i, n.to_string())).collect()
    }

    #[test]
    fn loyalty_points() {
        let raw: Vec<RawLoyalty> = serde_json::from_str(
            r#"[{"corporation_id":1000035,"loyalty_points":12000},{"corporation_id":1000180,"loyalty_points":0},
               {"corporation_id":1000125,"loyalty_points":500}]"#,
        )
        .unwrap();
        let d = build_loyalty(raw, &names(&[(1000035, "Caldari Navy"), (1000125, "CONCORD")]));
        assert_eq!(d.points.len(), 2);
        assert_eq!(d.points[0].corporation_name, "Caldari Navy");
        assert_eq!(d.points[0].loyalty_points, 12000);
        assert_eq!(d.points[1].corporation_name, "CONCORD");
    }

    #[test]
    fn standings() {
        let raw: Vec<RawStanding> = serde_json::from_str(
            r#"[{"from_id":500001,"from_type":"faction","standing":4.25},{"from_id":3008416,"from_type":"agent","standing":-1.5}]"#,
        )
        .unwrap();
        let d = build_standings(raw, &names(&[(500001, "Caldari State")]));
        assert_eq!(d.standings[0].name, "Caldari State");
        assert_eq!(d.standings[1].name, "#3008416");
        assert_eq!(d.standings[1].from_type, "agent");
    }

    #[test]
    fn research() {
        let raw: Vec<RawAgent> = serde_json::from_str(
            r#"[{"agent_id":3009358,"points_per_day":53.5,"remainder_points":12.25,"skill_type_id":11433,
                "started_at":"2025-03-01T10:00:00Z"}]"#,
        )
        .unwrap();
        let d = build_research(raw, &names(&[(3009358, "Kaitsu Uchi")]));
        assert_eq!(d.agents[0].agent_name, "Kaitsu Uchi");
        assert_eq!(d.agents[0].points_per_day, 53.5);
        assert!(!d.agents[0].skill_name.is_empty());
    }

    #[test]
    fn notifications() {
        let raw: Vec<RawNotification> = serde_json::from_str(
            r#"[{"notification_id":1,"sender_id":1000125,"sender_type":"corporation","text":"amount: 1000\n",
                 "timestamp":"2026-01-01T00:00:00Z","type":"BountyClaimMsg"},
                {"notification_id":2,"sender_id":1035466617946,"sender_type":"other","timestamp":"2026-02-01T00:00:00Z",
                 "type":"StructureUnderAttack","is_read":true}]"#,
        )
        .unwrap();
        let d = build_notifications(raw, &names(&[(1000125, "CONCORD"), (1035466617946, "Nope")]));
        assert_eq!(d.notifications[0].notification_id, 2, "newest first");
        assert!(d.notifications[0].is_read);
        assert_eq!(d.notifications[0].sender_name, None, "'other' senders aren't named");
        assert_eq!(d.notifications[1].sender_name.as_deref(), Some("CONCORD"));
        assert!(!d.notifications[1].is_read);
        // round trip keeps ESI's "type" key
        let json = serde_json::to_string(&d.notifications[1]).unwrap();
        assert!(json.contains("\"type\":\"BountyClaimMsg\""));
    }

    #[test]
    fn calendar() {
        let events: Vec<CalendarEvent> = serde_json::from_str(
            r#"[{"event_date":"2026-12-01T19:00:00Z","event_id":7,"event_response":"accepted","importance":1,"title":"Fleet"}]"#,
        )
        .unwrap();
        assert_eq!(events[0].importance, 1);
        assert_eq!(events[0].event_response, "accepted");
    }

    const KILLMAIL: &str = r#"{"killmail_id":130000001,"killmail_time":"2026-11-01T12:30:00Z","solar_system_id":30002187,
        "victim":{"character_id":95465499,"corporation_id":98000001,"alliance_id":99000001,"ship_type_id":587,
                  "damage_taken":2500,"items":[],"position":{"x":0,"y":0,"z":0}},
        "attackers":[{"character_id":2112000001,"corporation_id":98000002,"ship_type_id":11999,"damage_done":2000,
                      "final_blow":false,"security_status":0.5,"weapon_type_id":3},
                     {"corporation_id":1000125,"damage_done":500,"final_blow":true,"security_status":0}]}"#;

    #[test]
    fn killmail_loss_and_final_blow() {
        let km: RawKillmail = serde_json::from_str(KILLMAIL).unwrap();
        let n = names(&[(95465499, "Me"), (98000001, "My Corp"), (99000001, "My Alliance"), (587, "Rifter"),
            (30002187, "Amarr"), (1000125, "CONCORD")]);
        let ids = km.name_ids();
        assert!(ids.contains(&1000125) && !ids.contains(&2112000001));
        let k = build_killmail(km, "abc".into(), 95465499, &n);
        assert!(k.loss);
        assert_eq!(k.victim.name, "Me");
        assert_eq!(k.victim.ship, "Rifter");
        assert_eq!(k.victim.alliance.as_deref(), Some("My Alliance"));
        assert_eq!(k.final_blow.name, "CONCORD");
        assert_eq!(k.final_blow.ship, None);
        assert_eq!(k.attackers, 2);
        assert_eq!(k.system, "Amarr");
        // seen from the attacker's side it's a kill
        let km: RawKillmail = serde_json::from_str(KILLMAIL).unwrap();
        assert!(!build_killmail(km, "abc".into(), 2112000001, &n).loss);
    }

    #[test]
    fn killmail_without_names() {
        let km: RawKillmail = serde_json::from_str(
            r#"{"killmail_id":1,"killmail_time":"2026-11-01T12:30:00Z","solar_system_id":30000142,
                "victim":{"ship_type_id":35832,"damage_taken":1},"attackers":[]}"#,
        )
        .unwrap();
        let k = build_killmail(km, "h".into(), 1, &HashMap::new());
        assert_eq!(k.victim.name, "Structure");
        assert_eq!(k.final_blow.name, "NPC");
        assert_eq!(k.victim.ship, "Type #35832");
    }

    #[test]
    fn latest_killmails() {
        let recent: Vec<RawRecent> = (0..60).map(|i| RawRecent { killmail_id: i, killmail_hash: String::new() }).collect();
        let l = latest(recent);
        assert_eq!(l.len(), KILLMAIL_LIMIT);
        assert_eq!(l[0].killmail_id, 59);
    }

    #[test]
    fn medals() {
        let raw: Vec<RawMedal> = serde_json::from_str(
            r#"[{"corporation_id":98000001,"date":"2024-01-01T00:00:00Z","description":"For valour","graphics":[],
                 "issuer_id":2112000001,"medal_id":5,"reason":"Held the line","status":"private","title":"Iron Cross"},
                {"corporation_id":98000001,"date":"2025-01-01T00:00:00Z","description":"","graphics":[],
                 "issuer_id":2112000001,"medal_id":6,"reason":"","status":"public","title":"Newer"}]"#,
        )
        .unwrap();
        let d = build_medals(raw, &names(&[(98000001, "My Corp"), (2112000001, "The CEO")]));
        assert_eq!(d.medals[0].title, "Newer");
        assert_eq!(d.medals[1].issuer.as_deref(), Some("The CEO"));
        assert_eq!(d.medals[1].status, "private");
    }

    #[test]
    fn contacts() {
        let raw: Vec<RawContact> = serde_json::from_str(
            r#"[{"contact_id":2112000001,"contact_type":"character","standing":10.0,"is_watched":true,"label_ids":[1,9]},
                {"contact_id":99000001,"contact_type":"alliance","standing":-10.0,"is_blocked":true}]"#,
        )
        .unwrap();
        let labels: Vec<RawLabel> = serde_json::from_str(r#"[{"label_id":1,"label_name":"Friends"}]"#).unwrap();
        let d = build_contacts(raw, labels, &names(&[(2112000001, "Buddy")]));
        assert_eq!(d.contacts[0].name, "Buddy");
        assert!(d.contacts[0].is_watched && !d.contacts[0].is_blocked);
        assert_eq!(d.contacts[0].labels, vec!["Friends".to_string()]);
        assert_eq!(d.contacts[1].name, "#99000001");
        assert!(d.contacts[1].is_blocked);
    }

    #[test]
    fn fw_stats() {
        let fw: FwData = serde_json::from_str(
            r#"{"enlisted_on":"2025-05-01T00:00:00Z","faction_id":500001,"current_rank":3,"highest_rank":4,
                "kills":{"last_week":2,"total":120,"yesterday":0},"victory_points":{"last_week":500,"total":40000,"yesterday":20}}"#,
        )
        .unwrap();
        assert!(fw.enlisted());
        assert_eq!(fw.kills.total, 120);
        assert_eq!(fw.victory_points.yesterday, 20);
        let none: FwData = serde_json::from_str(r#"{"kills":{"last_week":0,"total":0,"yesterday":0},
            "victory_points":{"last_week":0,"total":0,"yesterday":0}}"#).unwrap();
        assert!(!none.enlisted());
    }
}
