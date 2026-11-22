//! Attribute remaps: the fastest split of the remap points for a set of skills, and when a plan is long enough
//! for a second remap a year later.

use super::sim::{Item, SkillItem, ATTRIBUTE_ORDER};
use crate::sde::Attribute;
use crate::training::AttributeSet;

pub const MIN_ATTRIBUTE: i32 = 17;
pub const MAX_ATTRIBUTE: i32 = 27;
/// spread over the five attributes on top of 17 each
pub const REMAP_POINTS: i32 = 14;
pub const TOTAL_POINTS: i32 = 5 * MIN_ATTRIBUTE + REMAP_POINTS;
/// between yearly remaps, ms
pub const REMAP_COOLDOWN: f64 = 365.0 * 24.0 * 3600.0 * 1000.0;
/// a second remap is only suggested if it saves at least this much (ms)
const MIN_SECOND_REMAP_SAVING: f64 = 24.0 * 3600.0 * 1000.0;

/// The skills a remap affects: from the remap at `remap_index` (or the start of the plan for None) up to the next
/// remap.
pub fn section_after(queue: &[Item], remap_index: Option<usize>) -> Vec<SkillItem> {
    let start = remap_index.map(|i| i + 1).unwrap_or(0);
    let mut out = vec![];
    for it in queue.iter().skip(start) {
        match it {
            Item::Remap { .. } => break,
            Item::Skill(s) => out.push(s.clone()),
            Item::Note { .. } => {}
        }
    }
    out
}

/// SP still to train, totalled per primary/secondary attribute pair.
pub fn sp_by_pair(skills: &[SkillItem]) -> Vec<(Attribute, Attribute, f64)> {
    let mut pairs: Vec<(Attribute, Attribute, f64)> = vec![];
    for s in skills {
        match pairs.iter_mut().find(|p| p.0 == s.primary && p.1 == s.secondary) {
            Some(p) => p.2 += s.sp,
            None => pairs.push((s.primary, s.secondary, s.sp)),
        }
    }
    pairs
}

fn time_for_pairs(pairs: &[(Attribute, Attribute, f64)], a: &AttributeSet, implants: i32, omega: Option<bool>) -> f64 {
    let imp = implants as f64;
    let mut hours = 0.0;
    for (p, s, sp) in pairs {
        let rate = ((a.get(*p) + imp) + (a.get(*s) + imp) / 2.0) * 60.0;
        // half speed only for a character known to be Alpha
        hours += sp / if omega == Some(false) { rate / 2.0 } else { rate };
    }
    hours * 3_600_000.0
}

/// Training time (ms) of the skills with the given base attributes plus a flat implant bonus.
pub fn training_time(skills: &[SkillItem], a: &AttributeSet, implants: i32, omega: Option<bool>) -> f64 {
    time_for_pairs(&sp_by_pair(skills), a, implants, omega)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Remap {
    pub attributes: AttributeSet,
    /// ms
    pub time: f64,
}

/// Fastest remap for the skills. Every legal split of the 14 points is tried (about a thousand), so the result is
/// the true optimum. Ties (under a second apart) keep the split closest to `current`, so no pointless changes are
/// suggested.
pub fn optimise(skills: &[SkillItem], implants: i32, omega: Option<bool>, current: Option<&AttributeSet>) -> Remap {
    let pairs = sp_by_pair(skills);
    let extra = MAX_ATTRIBUTE - MIN_ATTRIBUTE;
    let distance = |a: &AttributeSet| current.map(|c| Attribute::ALL.iter().map(|x| (a.get(*x) - c.get(*x)).abs()).sum::<f64>()).unwrap_or(0.0);
    let mut best: Option<Remap> = None;
    for p in 0..=extra {
        for m in 0..=extra.min(REMAP_POINTS - p) {
            for w in 0..=extra.min(REMAP_POINTS - p - m) {
                for i in 0..=extra.min(REMAP_POINTS - p - m - w) {
                    let c = REMAP_POINTS - p - m - w - i;
                    if c > extra {
                        continue;
                    }
                    let f = |x: i32| (MIN_ATTRIBUTE + x) as f64;
                    let a = AttributeSet { perception: f(p), memory: f(m), willpower: f(w), intelligence: f(i), charisma: f(c) };
                    let time = time_for_pairs(&pairs, &a, implants, omega);
                    let better = match &best {
                        None => true,
                        Some(b) => time < b.time - 1000.0 || ((time - b.time).abs() <= 1000.0 && distance(&a) < distance(&b.attributes)),
                    };
                    if better {
                        best = Some(Remap { attributes: a, time });
                    }
                }
            }
        }
    }
    best.expect("at least one split")
}

#[derive(Clone, Debug, PartialEq)]
pub struct SecondRemap {
    pub first: Remap,
    pub second: Remap,
    /// index into the skills of the last skill before the second remap
    pub split_after: usize,
    pub first_duration: f64,
    pub time: f64,
    pub saving: f64,
}

/// For skills that take longer than a year even with the best remap: the best place for a second remap, once the
/// yearly remap is available again (at least a year after the first). Every skill boundary from a year in is
/// tried, each side optimised separately; the split with the shortest total wins. None if the plan is under a
/// year or a second remap wouldn't save at least a day.
pub fn optimise_second(skills: &[SkillItem], implants: i32, omega: Option<bool>, current: Option<&AttributeSet>) -> Option<SecondRemap> {
    let single = optimise(skills, implants, omega, current);
    if skills.len() < 2 || single.time <= REMAP_COOLDOWN {
        return None;
    }
    // the first remap's own optimum is fastest for what comes before the split, so no split before the point
    // where even that reaches a year can work; start there
    let mut elapsed = 0.0;
    let mut start = skills.len();
    for (i, s) in skills.iter().enumerate() {
        elapsed += training_time(std::slice::from_ref(s), &single.attributes, implants, omega);
        if elapsed >= REMAP_COOLDOWN {
            start = i;
            break;
        }
    }
    let mut best: Option<(Remap, Remap, usize, f64)> = None;
    for i in start..skills.len().saturating_sub(1) {
        let first = optimise(&skills[..=i], implants, omega, current);
        // the second remap has to wait for the cooldown
        if first.time < REMAP_COOLDOWN {
            continue;
        }
        let second = optimise(&skills[i + 1..], implants, omega, Some(&first.attributes));
        let time = first.time + second.time;
        if best.as_ref().map(|b| time < b.3 - 1000.0).unwrap_or(true) {
            best = Some((first, second, i, time));
        }
    }
    let (first, second, split_after, time) = best?;
    if single.time - time < MIN_SECOND_REMAP_SAVING {
        return None;
    }
    Some(SecondRemap { first, second, split_after, first_duration: first.time, time, saving: single.time - time })
}

/// What changes from `current` to `target`, in EVE's order, e.g. [(Perception, +4), (Charisma, -2)].
pub fn changes(current: &AttributeSet, target: &AttributeSet) -> Vec<(Attribute, i32)> {
    ATTRIBUTE_ORDER
        .iter()
        .map(|a| (*a, (target.get(*a) - current.get(*a)).round() as i32))
        .filter(|(_, d)| *d != 0)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::sim::tests::{attrs, base, id};
    use super::super::sim::{Item, Planner};
    use super::*;

    #[test]
    fn optimum_favours_the_plans_attributes() {
        // Spaceship Command and Caldari ships are Perception/Willpower
        let p = Planner::with_items(&base(), &[Item::wanted(id("Caldari Cruiser"), 5)]);
        let skills = section_after(&p.queue, None);
        let r = optimise(&skills, 0, None, None);
        assert_eq!(r.attributes.perception, 27.0);
        assert_eq!(r.attributes.perception + r.attributes.memory + r.attributes.willpower + r.attributes.intelligence + r.attributes.charisma, 99.0);
        assert!(r.attributes.willpower >= 17.0);
        let flat = training_time(&skills, &attrs(20.0, 20.0, 20.0, 20.0, 19.0), 0, None);
        assert!(r.time < flat);
        // with implants, everything is faster
        assert!(optimise(&skills, 5, None, None).time < r.time);
    }

    #[test]
    fn ties_keep_current() {
        // no skills: every split takes 0, so the current one is kept
        let cur = attrs(19.0, 23.0, 17.0, 21.0, 19.0);
        assert_eq!(optimise(&[], 0, None, Some(&cur)).attributes, cur);
        assert!(changes(&cur, &cur).is_empty());
        assert_eq!(changes(&cur, &attrs(21.0, 23.0, 17.0, 21.0, 17.0)), vec![(Attribute::Perception, 2), (Attribute::Charisma, -2)]);
    }

    #[test]
    fn sections_stop_at_remaps() {
        let mut p = Planner::new(&base());
        p.plan_skill(id("Caldari Frigate"), 2, None);
        p.add_remap(attrs(27.0, 17.0, 21.0, 17.0, 17.0), 0);
        p.plan_skill(id("Mechanics"), 1, None);
        // Spaceship Command I, Caldari Frigate I and II, then the remap
        assert_eq!(section_after(&p.queue, None).len(), 3);
        assert_eq!(section_after(&p.queue, Some(3)).len(), 1);
    }

    #[test]
    fn second_remap_for_long_plans() {
        // ~1.5 years of Per/Wil then ~1.5 years of Int/Mem: two remaps beat one
        let mk = |name: &str, sp: f64, p: Attribute, s: Attribute| SkillItem {
            id: 1,
            level: 1,
            name: name.into(),
            sp,
            sp_total: sp,
            sp_hour: 0.0,
            time: 0.0,
            last_remap: 0.0,
            primary: p,
            secondary: s,
            required: vec![],
            plan: None,
        };
        let year_sp = 2700.0 * 24.0 * 365.0;
        let mut skills = vec![];
        for i in 0..12 {
            skills.push(mk(&format!("a{i}"), year_sp / 8.0, Attribute::Perception, Attribute::Willpower));
        }
        for i in 0..12 {
            skills.push(mk(&format!("b{i}"), year_sp / 8.0, Attribute::Intelligence, Attribute::Memory));
        }
        let r = optimise_second(&skills, 0, None, None).unwrap();
        assert_eq!(r.first.attributes.perception, 27.0);
        assert_eq!(r.second.attributes.intelligence, 27.0);
        assert!(r.first_duration >= REMAP_COOLDOWN);
        assert!(r.saving > MIN_SECOND_REMAP_SAVING);
        // a short plan doesn't get one
        assert!(optimise_second(&skills[..3], 0, None, None).is_none());
    }
}
