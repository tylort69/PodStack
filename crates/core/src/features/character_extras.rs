//! Calculations behind the character pages: NPC standings after skills, research points, notification titles and
//! read state, jump fatigue, implant values, and what loyalty points are worth in a corporation's LP store.

use crate::character::Character;
use crate::domains::skills::QueueEntry;
use crate::domains::social::{Contact, Notification, ResearchAgent};
use crate::esi::Esi;
use crate::Core;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

// ---------- standings ----------

pub const CONNECTIONS: i64 = 3359;
pub const DIPLOMACY: i64 = 3357;

/// The standing EVE uses with NPCs: Connections raises a positive standing, Diplomacy a negative one, each by 4% per
/// level of the distance to 10.
pub fn effective_standing(standing: f64, connections: u8, diplomacy: u8) -> f64 {
    let level = if standing >= 0.0 { connections } else { diplomacy };
    standing + (10.0 - standing) * 0.04 * level as f64
}

/// (Connections, Diplomacy) levels.
pub fn standing_skills(c: &Character) -> (u8, u8) {
    (c.skill_level(CONNECTIONS), c.skill_level(DIPLOMACY))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StandingTier {
    Excellent,
    Good,
    Neutral,
    Bad,
    Terrible,
}

pub fn standing_tier(v: f64) -> StandingTier {
    if v >= 5.0 {
        StandingTier::Excellent
    } else if v > 0.0 {
        StandingTier::Good
    } else if v <= -5.0 {
        StandingTier::Terrible
    } else if v < 0.0 {
        StandingTier::Bad
    } else {
        StandingTier::Neutral
    }
}

/// "+5.25", "-1.00", "0.00"
pub fn standing_text(v: f64) -> String {
    if v > 0.0 {
        format!("+{v:.2}")
    } else {
        format!("{v:.2}")
    }
}

/// Contacts search: name, type or any label contains the query (case-insensitive).
pub fn contact_matches(c: &Contact, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || c.name.to_lowercase().contains(&q)
        || c.contact_type.to_lowercase().contains(&q)
        || c.labels.iter().any(|l| l.to_lowercase().contains(&q))
}

// ---------- research ----------

/// Research points build up with an agent from when the research started (remainder_points is what was there then).
pub fn research_points(a: &ResearchAgent, now: DateTime<Utc>) -> f64 {
    let days = (now - a.started_at).num_seconds() as f64 / 86400.0;
    a.remainder_points + a.points_per_day * days.max(0.0)
}

// ---------- notifications ----------

/// Local state key: notification ids marked read in PodStack.
pub const NOTIFICATIONS_SEEN: &str = "notifications.seen";

/// "StructureUnderAttack" -> "Structure Under Attack", "FWAllianceWarningMsg" -> "FW Alliance Warning Msg"
pub fn notification_title(kind: &str) -> String {
    let chars: Vec<char> = kind.chars().collect();
    let mut out = String::with_capacity(kind.len() + 8);
    for (i, &ch) in chars.iter().enumerate() {
        if i > 0 && ch.is_ascii_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).map(|n| n.is_ascii_lowercase()).unwrap_or(false);
            if prev.is_ascii_lowercase() || (prev.is_ascii_uppercase() && next_lower) {
                out.push(' ');
            }
        }
        out.push(ch);
    }
    out
}

/// The notification's details as "key: value" lines (ESI gives them as YAML).
pub fn notification_details(text: Option<&str>) -> Vec<String> {
    text.unwrap_or("").lines().map(|l| l.trim_end().to_string()).filter(|l| !l.is_empty() && l != "{}").collect()
}

/// Notification search over title, sender and details.
pub fn notification_matches(n: &Notification, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    q.is_empty()
        || notification_title(&n.kind).to_lowercase().contains(&q)
        || n.sender_name.as_deref().unwrap_or("").to_lowercase().contains(&q)
        || n.text.as_deref().unwrap_or("").to_lowercase().contains(&q)
}

/// Unread in EVE and not marked read in PodStack. ESI can't mark notifications read, so "Mark all read" remembers
/// which were seen; a new one counts again.
pub fn is_notification_unread(n: &Notification, seen: &HashSet<i64>) -> bool {
    !n.is_read && !seen.contains(&n.notification_id)
}

pub fn seen_notifications(c: &Character) -> HashSet<i64> {
    c.local::<Vec<i64>>(NOTIFICATIONS_SEEN).into_iter().collect()
}

pub fn unread_notification_count(c: &Character) -> usize {
    let seen = seen_notifications(c);
    c.notifications.as_ref().map(|d| d.notifications.iter().filter(|n| is_notification_unread(n, &seen)).count()).unwrap_or(0)
}

pub fn mark_all_notifications_read(core: &Core, id: i64) {
    core.update_character(id, |c| {
        let ids: Vec<i64> = c
            .notifications
            .as_ref()
            .map(|d| d.notifications.iter().filter(|n| !n.is_read).map(|n| n.notification_id).collect())
            .unwrap_or_default();
        c.set_local(NOTIFICATIONS_SEEN, &ids);
    });
}

// ---------- calendar ----------

/// Events still worth showing: the ones starting later than an hour ago.
pub fn is_upcoming(event_date: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    event_date > now - Duration::hours(1)
}

// ---------- jump fatigue, clones, training ----------

#[derive(Clone, Debug, PartialEq)]
pub struct FatigueInfo {
    pub last_jump: DateTime<Utc>,
    /// Jump fatigue ("blue timer") expiry, None when it has run out.
    pub blue_timer: Option<DateTime<Utc>>,
    /// Jump activation ("red timer") expiry, None when it has run out.
    pub red_timer: Option<DateTime<Utc>>,
}

/// None until the character has jumped.
pub fn fatigue_info(c: &Character, now: DateTime<Utc>) -> Option<FatigueInfo> {
    let f = c.fatigue.as_ref()?;
    Some(FatigueInfo {
        last_jump: f.last_jump_date?,
        blue_timer: f.jump_fatigue_expire_date.filter(|d| *d > now),
        red_timer: f.last_update_date.filter(|d| *d > now),
    })
}

/// When the next clone jump is possible: None = now.
pub fn clone_jump_wait(c: &Character, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    c.next_clone_jump().filter(|d| *d > now)
}

/// Fraction (0-1) of a queue entry trained so far.
pub fn training_progress(q: &QueueEntry, now: DateTime<Utc>) -> f64 {
    match (q.start_date, q.finish_date) {
        (Some(s), Some(e)) if e > s => ((now - s).num_milliseconds() as f64 / (e - s).num_milliseconds() as f64).clamp(0.0, 1.0),
        _ => 0.0,
    }
}

/// How long a queued skill takes from now (or from its start, if that's later).
pub fn queue_entry_length(q: &QueueEntry, now: DateTime<Utc>) -> Option<Duration> {
    let end = q.finish_date?;
    let start = q.start_date.map(|s| s.max(now)).unwrap_or(now);
    Some(if end > start { end - start } else { start - end })
}

/// Implants' value at given prices: (ISK, how many have no price).
pub fn implants_value(type_ids: impl IntoIterator<Item = i64>, prices: &HashMap<i64, Option<f64>>) -> (f64, usize) {
    let mut value = 0.0;
    let mut unpriced = 0;
    for id in type_ids {
        match prices.get(&id).copied().flatten() {
            Some(p) => value += p,
            None => unpriced += 1,
        }
    }
    (value, unpriced)
}

pub fn zkillboard_url(killmail_id: i64) -> String {
    format!("https://zkillboard.com/kill/{killmail_id}/")
}

// ---------- loyalty point stores ----------

/// Offers repriced at Jita per store, picked by their value at EVE's average prices.
const LP_CANDIDATES: usize = 10;
/// A Jita price this far above EVE's average is a lone overpriced order, not what the item sells for.
const LP_MAX_ABOVE_AVERAGE: f64 = 3.0;
const LP_CACHE_HOURS: i64 = 6;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct RequiredItem {
    pub type_id: i64,
    pub quantity: i64,
    #[serde(default)]
    pub name: String,
}

/// An LP store offer, as ESI gives it (plus names and its value once priced).
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct LpOffer {
    pub offer_id: i64,
    pub type_id: i64,
    #[serde(default = "one")]
    pub quantity: i64,
    pub lp_cost: i64,
    #[serde(default)]
    pub isk_cost: f64,
    #[serde(default)]
    pub ak_cost: Option<i64>,
    #[serde(default)]
    pub required_items: Vec<RequiredItem>,
    #[serde(default)]
    pub name: String,
    /// ISK per LP at Jita 4-4 sell, after the offer's ISK and item costs.
    #[serde(default)]
    pub isk_per_lp: f64,
}

fn one() -> i64 {
    1
}

/// ISK per LP of an offer at these prices, None when something in it has no price.
pub fn isk_per_lp(o: &LpOffer, prices: &HashMap<i64, f64>) -> Option<f64> {
    let product = *prices.get(&o.type_id)?;
    if o.lp_cost <= 0 {
        return None;
    }
    let mut cost = o.isk_cost;
    for item in &o.required_items {
        cost += prices.get(&item.type_id)? * item.quantity as f64;
    }
    Some((product * o.quantity as f64 - cost) / o.lp_cost as f64)
}

/// Picks a store's best offers: screened at average prices, the top candidates priced at Jita (`jita`), best first.
/// Blueprint copies and offers needing Analysis Kredits are left out (no meaningful market price).
pub fn rank_offers(offers: Vec<LpOffer>, averages: &HashMap<i64, f64>, jita: &HashMap<i64, f64>) -> Vec<LpOffer> {
    let mut best: Vec<LpOffer> = screen_offers(offers, averages)
        .into_iter()
        .filter_map(|mut o| {
            let v = isk_per_lp(&o, jita)?;
            let too_high = jita.get(&o.type_id).copied().unwrap_or(0.0) > averages.get(&o.type_id).copied().unwrap_or(0.0) * LP_MAX_ABOVE_AVERAGE;
            (v > 0.0 && !too_high).then(|| {
                o.isk_per_lp = v;
                o
            })
        })
        .collect();
    best.sort_by(|a, b| b.isk_per_lp.total_cmp(&a.isk_per_lp));
    best
}

/// The offers worth pricing at Jita: the best [`LP_CANDIDATES`] at average prices.
fn screen_offers(offers: Vec<LpOffer>, averages: &HashMap<i64, f64>) -> Vec<LpOffer> {
    let mut scored: Vec<(f64, LpOffer)> = offers
        .into_iter()
        .filter(|o| !(o.ak_cost.unwrap_or(0) > 0) && !o.name.ends_with(" Blueprint"))
        .filter_map(|o| isk_per_lp(&o, averages).filter(|v| *v > 0.0).map(|v| (v, o)))
        .collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    scored.into_iter().take(LP_CANDIDATES).map(|(_, o)| o).collect()
}

/// Best offers per corporation, for the session.
type OfferCache = HashMap<i64, (DateTime<Utc>, Vec<LpOffer>)>;
static LP_CACHE: LazyLock<Mutex<OfferCache>> = LazyLock::new(Default::default);

/// A corporation's best LP store offers if they were priced recently (no network).
pub fn cached_best_offers(corporation_id: i64) -> Option<Vec<LpOffer>> {
    LP_CACHE
        .lock()
        .get(&corporation_id)
        .filter(|(d, _)| *d > Utc::now() - Duration::hours(LP_CACHE_HOURS))
        .map(|(_, o)| o.clone())
}

/// A corporation's LP store offers worth the most ISK per LP at Jita 4-4 sell prices, best first. Empty when the
/// store can't be loaded or nothing in it has a reliable price.
pub async fn best_offers(core: &Core, corporation_id: i64) -> Vec<LpOffer> {
    if let Some(o) = cached_best_offers(corporation_id) {
        return o;
    }
    let offers = load_offers(core, corporation_id).await;
    LP_CACHE.lock().insert(corporation_id, (Utc::now(), offers.clone()));
    offers
}

async fn load_offers(core: &Core, corporation_id: i64) -> Vec<LpOffer> {
    let esi: &Esi = &core.esi;
    let mut offers: Vec<LpOffer> = match esi.get(format!("loyalty/stores/{corporation_id}/offers")).json().await {
        Ok(o) => o,
        Err(err) => {
            tracing::warn!("LP store of {corporation_id}: {err}");
            return vec![];
        }
    };
    let ids = offers.iter().flat_map(|o| std::iter::once(o.type_id).chain(o.required_items.iter().map(|i| i.type_id)));
    let names = core.universe.names(esi, ids.collect::<Vec<_>>()).await;
    let name_of = |id: i64| names.get(&id).cloned().unwrap_or_else(|| format!("Type #{id}"));
    for o in &mut offers {
        o.name = name_of(o.type_id);
        for i in &mut o.required_items {
            i.name = name_of(i.type_id);
        }
    }
    let averages = core.market.average_prices(esi).await;
    let candidates = screen_offers(offers, &averages);
    let wanted: Vec<i64> = candidates
        .iter()
        .flat_map(|o| std::iter::once(o.type_id).chain(o.required_items.iter().map(|i| i.type_id)))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let jita: HashMap<i64, f64> =
        core.market.jita_sell(esi, &wanted, false).await.into_iter().filter_map(|(k, v)| v.map(|v| (k, v))).collect();
    rank_offers(candidates, &averages, &jita)
}

/// What a corporation's LP are worth spent on its best offer.
pub fn lp_value(loyalty_points: i64, offers: &[LpOffer]) -> Option<f64> {
    offers.first().map(|o| o.isk_per_lp * loyalty_points as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::location::Fatigue;
    use chrono::TimeZone;

    #[test]
    fn effective_standings() {
        assert!((effective_standing(0.0, 5, 0) - 2.0).abs() < 1e-9);
        assert!((effective_standing(5.0, 4, 0) - 5.8).abs() < 1e-9);
        assert!((effective_standing(-4.0, 5, 3) - (-4.0 + 14.0 * 0.12)).abs() < 1e-9);
        assert_eq!(effective_standing(10.0, 5, 5), 10.0);
        assert_eq!(effective_standing(3.0, 0, 5), 3.0);
    }

    #[test]
    fn tiers_and_text() {
        assert_eq!(standing_tier(5.0), StandingTier::Excellent);
        assert_eq!(standing_tier(0.1), StandingTier::Good);
        assert_eq!(standing_tier(0.0), StandingTier::Neutral);
        assert_eq!(standing_tier(-0.5), StandingTier::Bad);
        assert_eq!(standing_tier(-5.0), StandingTier::Terrible);
        assert_eq!(standing_text(5.25), "+5.25");
        assert_eq!(standing_text(-1.0), "-1.00");
        assert_eq!(standing_text(0.0), "0.00");
    }

    #[test]
    fn titles() {
        assert_eq!(notification_title("StructureUnderAttack"), "Structure Under Attack");
        assert_eq!(notification_title("FWAllianceWarningMsg"), "FW Alliance Warning Msg");
        assert_eq!(notification_title("CorpAppNewMsg"), "Corp App New Msg");
        assert_eq!(notification_title(""), "");
    }

    #[test]
    fn details() {
        assert_eq!(notification_details(Some("a: 1\n\nb: 2  \n{}\n")), vec!["a: 1", "b: 2"]);
        assert!(notification_details(None).is_empty());
    }

    fn notification(id: i64, read: bool) -> Notification {
        Notification { notification_id: id, kind: "BountyClaimMsg".into(), is_read: read, ..Default::default() }
    }

    #[test]
    fn unread_state() {
        let mut c = Character::new(1);
        c.notifications = Some(crate::domains::social::NotificationsData {
            notifications: vec![notification(1, false), notification(2, true), notification(3, false)],
        });
        assert_eq!(unread_notification_count(&c), 2);
        c.set_local(NOTIFICATIONS_SEEN, &vec![1i64]);
        assert_eq!(unread_notification_count(&c), 1);
        let seen = seen_notifications(&c);
        assert!(!is_notification_unread(&notification(1, false), &seen));
        assert!(is_notification_unread(&notification(3, false), &seen));
        assert!(notification_matches(&notification(3, false), "bounty claim"));
        assert!(!notification_matches(&notification(3, false), "structure"));
    }

    #[test]
    fn research() {
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let a = ResearchAgent { started_at: start, points_per_day: 50.0, remainder_points: 10.0, ..Default::default() };
        assert!((research_points(&a, start + Duration::days(2)) - 110.0).abs() < 1e-9);
        assert!((research_points(&a, start - Duration::days(2)) - 10.0).abs() < 1e-9);
    }

    #[test]
    fn fatigue() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let mut c = Character::new(1);
        assert_eq!(fatigue_info(&c, now), None);
        c.fatigue = Some(Fatigue {
            jump_fatigue_expire_date: Some(now + Duration::hours(3)),
            last_jump_date: Some(now - Duration::hours(1)),
            last_update_date: Some(now - Duration::minutes(5)),
        });
        let f = fatigue_info(&c, now).unwrap();
        assert_eq!(f.blue_timer, Some(now + Duration::hours(3)));
        assert_eq!(f.red_timer, None);
    }

    #[test]
    fn progress() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let q = QueueEntry { start_date: Some(now - Duration::hours(1)), finish_date: Some(now + Duration::hours(3)), ..Default::default() };
        assert!((training_progress(&q, now) - 0.25).abs() < 1e-9);
        assert_eq!(queue_entry_length(&q, now), Some(Duration::hours(3)));
        let later = QueueEntry { start_date: Some(now + Duration::hours(3)), finish_date: Some(now + Duration::hours(5)), ..Default::default() };
        assert_eq!(queue_entry_length(&later, now), Some(Duration::hours(2)));
    }

    #[test]
    fn implant_values() {
        let prices: HashMap<i64, Option<f64>> = [(1, Some(100.0)), (2, None), (3, Some(50.0))].into_iter().collect();
        assert_eq!(implants_value([1, 2, 3, 4], &prices), (150.0, 2));
    }

    fn offer(id: i64, type_id: i64, lp: i64, isk: f64, items: Vec<(i64, i64)>, name: &str) -> LpOffer {
        LpOffer {
            offer_id: id,
            type_id,
            quantity: 1,
            lp_cost: lp,
            isk_cost: isk,
            ak_cost: None,
            required_items: items.into_iter().map(|(t, q)| RequiredItem { type_id: t, quantity: q, name: String::new() }).collect(),
            name: name.into(),
            isk_per_lp: 0.0,
        }
    }

    #[test]
    fn lp_maths() {
        let prices: HashMap<i64, f64> = [(10, 1_000_000.0), (20, 50_000.0)].into_iter().collect();
        // (1m - 100k - 2 × 50k) / 1000 LP
        let o = offer(1, 10, 1000, 100_000.0, vec![(20, 2)], "Navy Thing");
        assert_eq!(isk_per_lp(&o, &prices), Some(800.0));
        assert_eq!(isk_per_lp(&offer(2, 99, 1000, 0.0, vec![], "x"), &prices), None);
        assert_eq!(isk_per_lp(&offer(3, 10, 1000, 0.0, vec![(99, 1)], "x"), &prices), None);
        assert_eq!(isk_per_lp(&offer(4, 10, 0, 0.0, vec![], "x"), &prices), None);
    }

    #[test]
    fn lp_ranking() {
        let averages: HashMap<i64, f64> = [(10, 1_000_000.0), (11, 2_000_000.0), (12, 500_000.0), (13, 9_000_000.0)].into_iter().collect();
        let jita: HashMap<i64, f64> = [(10, 1_100_000.0), (11, 1_500_000.0), (12, 5_000_000.0), (13, 9_000_000.0)].into_iter().collect();
        let mut bp = offer(4, 13, 1000, 0.0, vec![], "Ship Blueprint");
        bp.quantity = 1;
        let mut ak = offer(5, 13, 1000, 0.0, vec![], "AK Thing");
        ak.ak_cost = Some(10);
        let offers = vec![
            offer(1, 10, 1000, 0.0, vec![], "A"),
            offer(2, 11, 1000, 0.0, vec![], "B"),
            // Jita price 10× the average: a lone overpriced order, dropped
            offer(3, 12, 1000, 0.0, vec![], "C"),
            bp,
            ak,
        ];
        let best = rank_offers(offers, &averages, &jita);
        assert_eq!(best.iter().map(|o| o.offer_id).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(best[0].isk_per_lp, 1500.0);
        assert_eq!(lp_value(10_000, &best), Some(15_000_000.0));
        assert_eq!(lp_value(10_000, &[]), None);
    }

    #[test]
    fn offers_parse() {
        let o: Vec<LpOffer> = serde_json::from_str(
            r#"[{"offer_id":1,"type_id":10,"quantity":5,"lp_cost":100,"isk_cost":1000,"ak_cost":0,
                 "required_items":[{"type_id":20,"quantity":1}]}]"#,
        )
        .unwrap();
        assert_eq!(o[0].quantity, 5);
        assert_eq!(o[0].required_items[0].type_id, 20);
    }

    #[test]
    fn contact_search() {
        let c = Contact { name: "Buddy".into(), contact_type: "character".into(), labels: vec!["Friends".into()], ..Default::default() };
        assert!(contact_matches(&c, ""));
        assert!(contact_matches(&c, "bud"));
        assert!(contact_matches(&c, "CHAR"));
        assert!(contact_matches(&c, "friend"));
        assert!(!contact_matches(&c, "enemy"));
    }
}
