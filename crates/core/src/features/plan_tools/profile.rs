//! Training profiles: a plan's time under other implant sets and cerebral accelerators (the implant profiler),
//! and the skillbooks a plan still needs.

use super::accelerator;
use super::sim::{Item, PlanBase, Planner};
use crate::character::Character;
use crate::config;
use crate::sde::{self, Attribute};
use crate::training::AttributeSet;

/// Which implants a profile assumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Implants {
    /// the character's actual implants
    Current,
    /// a uniform +N set (0 = none) in place of them
    Set(i32),
}

/// A clone setup to compare: implants and an optional accelerator {bonus, days} from the start of training.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Profile {
    pub implants: Implants,
    pub accelerator: Option<(f64, f64)>,
}

impl Profile {
    /// "Improved +5 · +10 accel 7d"
    pub fn label(&self) -> String {
        let mut s = implants_label(self.implants);
        if let Some((b, d)) = self.accelerator {
            s.push_str(&format!(" · +{b} accel {d}d"));
        }
        s
    }
}

/// Short name of an implant scenario, e.g. "Improved +5".
pub fn implants_label(i: Implants) -> String {
    match i {
        Implants::Current => "Current clone".into(),
        Implants::Set(0) => "No implants".into(),
        Implants::Set(n) => match sde::IMPLANT_GRADES.iter().find(|g| g.bonus as i32 == n) {
            Some(g) => format!("{} +{n}", g.grade),
            None => format!("+{n} implants"),
        },
    }
}

/// The scenarios the profiler lists: current clone, none, then each implant grade.
pub fn scenarios() -> Vec<Implants> {
    let mut v = vec![Implants::Current, Implants::Set(0)];
    v.extend(sde::IMPLANT_GRADES.iter().map(|g| Implants::Set(g.bonus as i32)));
    v
}

/// Current implant bonus per attribute. EVE reports attributes with implants already included.
pub fn implant_bonuses(c: &Character) -> AttributeSet {
    let mut b = AttributeSet::default();
    for imp in c.implants.as_deref().unwrap_or(&[]) {
        for (a, v) in &imp.bonuses {
            b.set(*a, b.get(*a) + *v as f64);
        }
    }
    b
}

/// The implant set every attribute has (as remaps assume), and whether the implants differ by attribute.
pub fn uniform_implants(c: &Character) -> (i32, bool) {
    let b = implant_bonuses(c);
    let vals: Vec<i32> = Attribute::ALL.iter().map(|a| b.get(*a) as i32).collect();
    let min = vals.iter().copied().min().unwrap_or(0);
    (min, vals.iter().any(|v| *v != min))
}

/// Base (remapped) attributes: ESI's values minus implants and an accelerator ESI's attributes include.
pub fn base_attributes(c: &Character) -> AttributeSet {
    let bonuses = implant_bonuses(c);
    let booster = accelerator::attribute_bonus(c) as f64;
    let mut a = AttributeSet::of(c);
    for x in Attribute::ALL {
        a.set(x, a.get(x) - bonuses.get(x) - booster);
    }
    a
}

/// Per-skill (id, level, ms) with the given implants and an extra flat bonus, in queue order.
fn item_times(c: &Character, base: &PlanBase, queue: &[Item], implants: Implants, extra: f64) -> Vec<(i64, u8, f64)> {
    // the accelerator to compare is applied by the caller, not the one the plan counts on its own
    let mut b = base.clone();
    b.accelerator = None;
    match implants {
        Implants::Current => b.attributes = b.attributes.plus_all(extra),
        Implants::Set(n) => b.attributes = base_attributes(c).plus_all(n as f64 + extra),
    }
    let mut p = Planner::new(&b);
    let mut out = vec![];
    for it in queue {
        match it {
            Item::Remap { attributes, implants: ri, .. } => {
                // a remap sets new base attributes; implants and booster still apply on top
                let imp = match implants {
                    Implants::Current => *ri as f64,
                    Implants::Set(n) => n as f64,
                };
                p.attributes = attributes.plus_all(imp + extra);
                p.last_remap = 0.0;
            }
            Item::Skill(s) => {
                let before = p.queue.len();
                p.plan_skill(s.id, s.level, None);
                out.extend(p.queue[before..].iter().filter_map(|i| i.skill()).map(|q| (q.id, q.level, q.time)));
            }
            Item::Note { .. } => {}
        }
    }
    out
}

/// Per-skill training times of a queue under a profile, in queue order: (id, level, ms) for each skill level.
/// `base` is the character's plan base (its accelerator is ignored: the profile's own counts).
pub fn simulate_items(c: &Character, base: &PlanBase, queue: &[Item], profile: &Profile) -> Vec<(i64, u8, f64)> {
    let without = item_times(c, base, queue, profile.implants, 0.0);
    let Some((bonus, days)) = profile.accelerator.filter(|(b, d)| *b > 0.0 && *d > 0.0) else { return without };
    // SP accrues linearly, so a skill straddling the expiry trains the boosted share at the boosted rate and the
    // rest at the normal rate
    let boosted = item_times(c, base, queue, profile.implants, bonus);
    let window = days * 24.0 * 3_600_000.0;
    let mut elapsed = 0.0;
    boosted
        .iter()
        .enumerate()
        .map(|(i, &(id, lvl, fast))| {
            let left = (window - elapsed).max(0.0);
            let t = if fast <= left {
                fast
            } else {
                let done = if fast > 0.0 { left / fast } else { 1.0 };
                left + (1.0 - done) * without.get(i).map(|w| w.2).unwrap_or(fast)
            };
            elapsed += t;
            (id, lvl, t)
        })
        .collect()
}

/// Total training time (ms) of a queue under a profile.
pub fn simulate(c: &Character, base: &PlanBase, queue: &[Item], profile: &Profile) -> f64 {
    simulate_items(c, base, queue, profile).iter().map(|x| x.2).sum()
}

/// Skills in a queue the character hasn't injected yet, i.e. the skillbooks they still need, by name.
/// Injected but untrained skills are listed by EVE with level 0, so they don't count.
pub fn missing_skillbooks(c: &Character, queue: &[Item]) -> Vec<(i64, String)> {
    let mut out: Vec<(i64, String)> = vec![];
    for s in queue.iter().filter_map(|i| i.skill()) {
        if c.skill(s.id).is_none() && !out.iter().any(|(id, _)| *id == s.id) {
            if let Some(info) = sde::SKILLS.get(s.id) {
                out.push((s.id, info.name.clone()));
            }
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

/// Price to buy and inject a skill straight from the skill window (NPC price plus markup), when it has one.
pub fn skill_window_price(id: i64) -> Option<f64> {
    sde::SKILL_PRICES.get(&id).map(|p| (p * config::SKILL_WINDOW_MARKUP).round())
}

pub fn markup_percent() -> i64 {
    ((config::SKILL_WINDOW_MARKUP - 1.0) * 100.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::super::sim::tests::id;
    use super::super::sim::{Item, PlanBase, Planner};
    use super::*;
    use crate::domains::clones::Implant;
    use crate::domains::skills::{Attributes, Skill, SkillsData};
    use chrono::Utc;

    fn character() -> Character {
        let mut c = Character::new(1);
        c.attributes = Some(Attributes { perception: 25, memory: 21, willpower: 20, intelligence: 20, charisma: 19, ..Default::default() });
        c.implants = Some(vec![Implant { type_id: 1, name: "Ocular Filter - Basic".into(), slot: Some(1), bonuses: vec![(Attribute::Perception, 3)] }]);
        c.skills = Some(SkillsData {
            skills: vec![Skill { skill_id: id("Spaceship Command"), skillpoints_in_skill: 1415, trained_skill_level: 2, active_skill_level: 2, ..Default::default() }],
            total_sp: 1415,
            unallocated_sp: 0,
        });
        c
    }

    #[test]
    fn base_attributes_leave_out_implants() {
        let c = character();
        let b = base_attributes(&c);
        assert_eq!(b.perception, 22.0);
        assert_eq!(b.memory, 21.0);
        assert_eq!(uniform_implants(&c), (0, true));
    }

    #[test]
    fn profiles_compare() {
        let c = character();
        let base = PlanBase::of(&c, Utc::now(), true);
        let p = Planner::with_items(&base, &[Item::wanted(id("Caldari Frigate"), 4)]);
        let cur = simulate(&c, &base, &p.queue, &Profile { implants: Implants::Current, accelerator: None });
        assert!((cur - p.time).abs() < 1.0);
        let none = simulate(&c, &base, &p.queue, &Profile { implants: Implants::Set(0), accelerator: None });
        let five = simulate(&c, &base, &p.queue, &Profile { implants: Implants::Set(5), accelerator: None });
        assert!(none > cur && five < cur);
        let boosted = simulate(&c, &base, &p.queue, &Profile { implants: Implants::Current, accelerator: Some((10.0, 1.0)) });
        let long = simulate(&c, &base, &p.queue, &Profile { implants: Implants::Current, accelerator: Some((10.0, 100.0)) });
        assert!(long < boosted && boosted < cur);
        assert_eq!(Profile { implants: Implants::Set(5), accelerator: Some((10.0, 7.0)) }.label(), "Improved +5 · +10 accel 7d");
    }

    #[test]
    fn skillbooks() {
        let c = character();
        let base = PlanBase::of(&c, Utc::now(), true);
        let p = Planner::with_items(&base, &[Item::wanted(id("Caldari Frigate"), 2), Item::wanted(id("Spaceship Command"), 3)]);
        let books = missing_skillbooks(&c, &p.queue);
        assert_eq!(books, vec![(id("Caldari Frigate"), "Caldari Frigate".to_string())]);
        assert!(skill_window_price(id("Caldari Frigate")).unwrap() > 0.0);
        assert_eq!(markup_percent(), 30);
    }
}
