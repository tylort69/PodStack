//! Detects an active cerebral accelerator and estimates when it runs out.
//!
//! ESI doesn't say which booster a character is using, so the bonus is worked out two ways, and either is enough:
//! - attributes: base (remapped) attributes always add up to 99 (17 each plus 14 to spread), so whatever ESI's
//!   attributes hold beyond that and the implants' bonuses, divided by five, is an accelerator's bonus;
//! - training speed: the skill queue gives the speed the current skill really trains at; if that's faster than the
//!   attributes explain, the difference is an accelerator the attributes don't include.
//!
//! ESI doesn't give the end time either. The start is taken as when the bonus was first seen (or, when it shows in
//! the training speed, when the current skill's training was last recalculated, which happens when one is plugged
//! in), the duration from the accelerator the player picks among those with that bonus, extended by Biology.
//!
//! The state lives in the character's local data under [`STATE_KEY`]; [`refresh`] updates it from the latest ESI
//! data and should run whenever a character's attributes, implants or queue change.

use super::sim::BoostWindow;
use crate::character::Character;
use crate::sde::{self, Accelerator, Attribute};
use crate::Core;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

pub const STATE_KEY: &str = "plans.accelerator";
/// Local key: plans leave the active accelerator out when true.
pub const IGNORE_KEY: &str = "plans.ignore_accelerator";

const BASE_TOTAL: i32 = 5 * 17 + 14;
const BIOLOGY: i64 = 3405;
/// booster duration per Biology level
const BIOLOGY_BONUS: f64 = 0.2;
const DAY_MS: f64 = 24.0 * 3600.0 * 1000.0;
/// A bonus seen this long ago or more can't still be from the same accelerator (the longest lasts 12.5 days, +100%).
const MAX_DURATION_MS: i64 = 25 * 24 * 3600 * 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// ESI's attributes include it
    Attributes,
    /// only the training speed shows it
    Training,
}

/// What's known about the active accelerator. `type_id` and `start` are the player's corrections.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub bonus: i32,
    pub source: Source,
    pub first_seen: DateTime<Utc>,
    pub estimated_start: DateTime<Utc>,
    #[serde(default)]
    pub type_id: Option<i64>,
    #[serde(default)]
    pub start: Option<DateTime<Utc>>,
}

fn implant_bonus(c: &Character) -> i32 {
    c.implants.as_deref().unwrap_or(&[]).iter().flat_map(|i| i.bonuses.iter()).map(|(_, v)| *v).sum()
}

/// (bonus, source) of an accelerator the data shows, or None.
pub fn detect(c: &Character) -> Option<(i32, Source)> {
    let attrs = c.attributes.as_ref()?;
    c.implants.as_ref()?;
    let total: i32 = Attribute::ALL.iter().map(|a| attrs.get(*a)).sum();
    let leftover = total - BASE_TOTAL - implant_bonus(c);
    if leftover > 0 && leftover % 5 == 0 {
        return Some((leftover / 5, Source::Attributes));
    }

    // training faster than the attributes allow (Omega only: Alpha clones train at a reduced speed)
    let q = c.current_skill()?;
    let info = sde::SKILLS.get(q.skill_id)?;
    if c.is_omega() != Some(true) {
        return None;
    }
    let hours = (q.finish_date? - q.start_date?).num_milliseconds() as f64 / 3_600_000.0;
    let sp_per_hour = (q.level_end_sp? - q.training_start_sp?) as f64 / hours;
    let primary = attrs.get(info.primary_attribute) as f64;
    let secondary = attrs.get(info.secondary_attribute) as f64;
    if hours > 0.25 && sp_per_hour > 0.0 && primary > 0.0 {
        let bonus = (sp_per_hour / 60.0 - primary - secondary / 2.0) / 1.5;
        if bonus >= 0.8 && (bonus - bonus.round()).abs() < 0.2 {
            return Some((bonus.round() as i32, Source::Training));
        }
    }
    None
}

/// The state after a refresh: started when a bonus appears (or changes), cleared when it's gone. The player's
/// choice of accelerator and start time are kept while the same bonus stays active.
pub fn update(c: &Character, previous: Option<&State>, now: DateTime<Utc>) -> Option<State> {
    let (bonus, source) = detect(c)?;
    if let Some(p) = previous {
        if p.bonus == bonus && (now - p.estimated_start).num_milliseconds() < MAX_DURATION_MS {
            return Some(State { source, ..p.clone() });
        }
    }
    let mut estimated_start = now;
    if source == Source::Training {
        if let Some(start) = c.current_skill().and_then(|q| q.start_date) {
            if start <= now && (now - start).num_milliseconds() < MAX_DURATION_MS {
                estimated_start = start;
            }
        }
    }
    Some(State { bonus, source, first_seen: now, estimated_start, type_id: None, start: None })
}

pub fn state(c: &Character) -> Option<State> {
    c.local.get(STATE_KEY).and_then(|v| serde_json::from_value(v.clone()).ok())
}

/// Brings the stored state up to date with the character's data; saves only when it changed.
pub fn refresh(core: &Core, id: i64) {
    let Some(c) = core.character(id) else { return };
    let prev = state(&c);
    let next = update(&c, prev.as_ref(), Utc::now());
    if next != prev {
        core.update_character(id, |c| match &next {
            Some(s) => c.set_local(STATE_KEY, s),
            None => {
                c.local.remove(STATE_KEY);
            }
        });
    }
}

/// The player's corrections: which accelerator it is, and when it was plugged in.
pub fn set_choice(core: &Core, id: i64, type_id: Option<i64>, start: Option<DateTime<Utc>>) {
    core.update_character(id, |c| {
        if let Some(mut s) = state(c) {
            if type_id.is_some() {
                s.type_id = type_id;
            }
            if start.is_some() {
                s.start = start;
            }
            c.set_local(STATE_KEY, &s);
        }
    });
}

/// The accelerators giving this bonus, for the player to pick from.
pub fn candidates(bonus: i32) -> Vec<&'static Accelerator> {
    sde::ACCELERATORS.iter().filter(|a| a.bonus as i32 == bonus).collect()
}

pub fn biology_level(c: &Character) -> u8 {
    c.skill_level(BIOLOGY)
}

/// An active accelerator as the player sees it.
#[derive(Clone)]
pub struct Status {
    pub bonus: i32,
    /// the picked or likeliest accelerator
    pub item: Option<&'static Accelerator>,
    pub start: DateTime<Utc>,
    pub end: Option<DateTime<Utc>>,
    /// time left (negative if it should have ended)
    pub remaining: Option<Duration>,
    /// the start wasn't set by the player
    pub estimated: bool,
    pub biology: u8,
}

pub fn status_of(c: &Character, st: &State, now: DateTime<Utc>) -> Status {
    let list = candidates(st.bonus);
    // the player's pick, else the regular store accelerator of that bonus ('Boost'), else the first match
    let item = list
        .iter()
        .find(|a| Some(a.type_id) == st.type_id)
        .or_else(|| list.iter().find(|a| a.name.contains(" 'Boost' ")))
        .or_else(|| list.first())
        .copied();
    let biology = biology_level(c);
    let start = st.start.unwrap_or(st.estimated_start);
    let end = item.map(|a| start + Duration::milliseconds((a.days * DAY_MS * (1.0 + BIOLOGY_BONUS * biology as f64)) as i64));
    Status { bonus: st.bonus, item, start, end, remaining: end.map(|e| e - now), estimated: st.start.is_none(), biology }
}

pub fn status(c: &Character, now: DateTime<Utc>) -> Option<Status> {
    state(c).map(|s| status_of(c, &s, now))
}

/// Bonus the accelerator adds to ESI's attributes (when they include it), so base attributes can be worked out.
pub fn attribute_bonus(c: &Character) -> i32 {
    match state(c) {
        Some(s) if s.source == Source::Attributes => s.bonus,
        _ => 0,
    }
}

pub fn ignored_in_plans(c: &Character) -> bool {
    c.local::<bool>(IGNORE_KEY)
}

/// When EVE's skill queue stops counting the accelerator. EVE times every queued skill with the accelerator until
/// the moment it expects it to run out, and at the normal speed after: the first entry training slower than
/// boosted marks that moment (inside it, when it straddles the end). This needn't match the accelerator's own
/// timer, so plans use it to come out at the same time as EVE's queue. None when the queue doesn't show it.
pub fn queue_boost_end(c: &Character, st: &State, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let attrs = c.attributes.as_ref()?;
    if c.is_omega() == Some(false) {
        return None;
    }
    let booster = if st.source == Source::Attributes { st.bonus } else { 0 };
    let base = |a: Attribute| (attrs.get(a) - booster) as f64;
    let mut entries: Vec<_> = c.queue().iter().filter(|q| q.start_date.is_some() && q.finish_date.map(|f| f > now).unwrap_or(false)).collect();
    entries.sort_by_key(|q| q.start_date);

    for q in entries {
        let (Some(start), Some(finish)) = (q.start_date, q.finish_date) else { continue };
        let Some(info) = sde::SKILLS.get(q.skill_id) else { continue };
        let hours = (finish - start).num_milliseconds() as f64 / 3_600_000.0;
        let sp = (q.level_end_sp.unwrap_or(0) - q.training_start_sp.unwrap_or(0)) as f64;
        // entries of a few minutes are too short to tell the speeds apart (EVE's times are whole seconds)
        if hours <= 0.25 || sp <= 0.0 {
            continue;
        }
        let normal = (base(info.primary_attribute) + base(info.secondary_attribute) / 2.0) * 60.0;
        let boosted = normal + st.bonus as f64 * 1.5 * 60.0;
        let rate = sp / hours;
        if normal <= 0.0 {
            return None;
        }
        if rate >= boosted * 0.995 {
            continue;
        }
        if rate <= normal * 1.005 {
            return Some(start);
        }
        if rate < boosted {
            // straddles the end: boosted hours at the boosted speed, the rest at the normal one
            let boosted_hours = (sp - normal * hours) / (boosted - normal);
            return Some(start + Duration::milliseconds((boosted_hours * 3_600_000.0) as i64));
        }
        return None;
    }
    None
}

/// The active accelerator as plans count it: its bonus, and how long from now it applies (EVE's skill queue's
/// idea of that when it shows it, else the accelerator's estimated end). None when there's none left.
pub fn plan_window(c: &Character, now: DateTime<Utc>) -> Option<BoostWindow> {
    let st = state(c)?;
    let status = status_of(c, &st, now);
    let end = queue_boost_end(c, &st, now).or(status.end)?;
    if end <= now {
        return None;
    }
    Some(BoostWindow { bonus: st.bonus as f64, end, remaining: (end - now).num_milliseconds() as f64 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::clones::Implant;
    use crate::domains::skills::{Attributes, QueueEntry, Skill, SkillsData};

    fn character(extra: i32) -> Character {
        let mut c = Character::new(1);
        c.attributes = Some(Attributes {
            perception: 27 + extra + 3,
            memory: 21 + extra + 3,
            willpower: 17 + extra,
            intelligence: 17 + extra,
            charisma: 17 + extra,
            ..Default::default()
        });
        c.implants = Some(vec![
            Implant { type_id: 1, name: "Ocular".into(), slot: Some(1), bonuses: vec![(Attribute::Perception, 3)] },
            Implant { type_id: 2, name: "Memory".into(), slot: Some(2), bonuses: vec![(Attribute::Memory, 3)] },
        ]);
        c
    }

    #[test]
    fn detects_from_attributes() {
        assert_eq!(detect(&character(0)), None);
        assert_eq!(detect(&character(10)), Some((10, Source::Attributes)));
        let now = Utc::now();
        let st = update(&character(10), None, now).unwrap();
        assert_eq!(st.estimated_start, now);
        // kept (with the player's pick) while the bonus stays
        let picked = State { type_id: Some(72242), ..st.clone() };
        let later = update(&character(10), Some(&picked), now + Duration::days(2)).unwrap();
        assert_eq!(later.type_id, Some(72242));
        assert_eq!(later.estimated_start, now);
        // a different bonus is a new accelerator
        let other = update(&character(8), Some(&picked), now).unwrap();
        assert_eq!(other.type_id, None);
        assert_eq!(update(&character(0), Some(&picked), now), None);
    }

    #[test]
    fn detects_from_training_speed() {
        let mut c = character(0);
        let sc = sde::SKILLS.by_name("Spaceship Command").unwrap();
        let now = Utc::now();
        // Per/Wil with +10 not in the attributes: (30 + 17/2 + 15) × 60 SP/h
        let rate = (30.0 + 8.5 + 15.0) * 60.0;
        let start = now - Duration::hours(1);
        c.skills = Some(SkillsData { skills: vec![Skill { skill_id: sc.type_id, ..Default::default() }], total_sp: 10_000_000, unallocated_sp: 0 });
        c.skill_queue = Some(vec![QueueEntry {
            skill_id: sc.type_id,
            finished_level: 5,
            start_date: Some(start),
            finish_date: Some(start + Duration::hours(10)),
            training_start_sp: Some(0),
            level_end_sp: Some((rate * 10.0) as i64),
            ..Default::default()
        }]);
        assert_eq!(detect(&c), Some((10, Source::Training)));
        let st = update(&c, None, now).unwrap();
        assert_eq!(st.estimated_start, start);
        let s = status_of(&c, &st, now);
        // the 'Boost' one of that bonus is the default guess
        assert_eq!(s.item.unwrap().type_id, 72242);
        assert_eq!(s.end.unwrap(), start + Duration::days(10));
    }

    #[test]
    fn queue_shows_the_boost_end() {
        let mut c = character(10);
        let sc = sde::SKILLS.by_name("Spaceship Command").unwrap();
        let now = Utc::now();
        let st = State { bonus: 10, source: Source::Attributes, first_seen: now, estimated_start: now, type_id: None, start: None };
        let normal: f64 = (30.0 + 17.0 / 2.0) * 60.0;
        let boosted = normal + 15.0 * 60.0;
        let start = now - Duration::hours(1);
        // 2 boosted hours then 3 normal ones
        let sp = boosted * 2.0 + normal * 3.0;
        c.skill_queue = Some(vec![QueueEntry {
            skill_id: sc.type_id,
            finished_level: 5,
            start_date: Some(start),
            finish_date: Some(start + Duration::hours(5)),
            training_start_sp: Some(0),
            level_end_sp: Some(sp.round() as i64),
            ..Default::default()
        }]);
        c.skills = Some(SkillsData { skills: vec![], total_sp: 10_000_000, unallocated_sp: 0 });
        let end = queue_boost_end(&c, &st, now).unwrap();
        assert!(((end - start).num_seconds() - 7200).abs() < 10, "{}", (end - start).num_seconds());
    }

    #[test]
    fn biology_extends() {
        let mut c = character(10);
        let now = Utc::now();
        c.skills = Some(SkillsData {
            skills: vec![Skill { skill_id: BIOLOGY, active_skill_level: 5, trained_skill_level: 5, ..Default::default() }],
            total_sp: 0,
            unallocated_sp: 0,
        });
        let st = State { bonus: 4, source: Source::Attributes, first_seen: now, estimated_start: now, type_id: None, start: None };
        let s = status_of(&c, &st, now);
        // Standard 'Boost' lasts 4 days, doubled by Biology V
        assert_eq!(s.end.unwrap(), now + Duration::days(8));
        assert!(s.estimated);
    }
}
