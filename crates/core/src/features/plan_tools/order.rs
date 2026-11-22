//! Reordering plans: fastest skills first ("Sort"), and the order that finishes the whole plan soonest
//! ("Optimise").

use super::remap::REMAP_COOLDOWN;
use super::sim::{Item, Planner, SkillItem};
use crate::sde;
use crate::training::AttributeSet;
use std::collections::HashSet;

fn prerequisites(id: i64) -> Vec<(i64, u8)> {
    sde::SKILLS.get(id).map(|s| s.required_skills.iter().map(|r| (r.id, r.level)).collect()).unwrap_or_default()
}

/// For each skill, the positions of the skills it must come after: the level below it, and its prerequisites up
/// to the level it needs. Non-skills have none and block nothing.
fn blockers_of(items: &[Item]) -> Vec<HashSet<usize>> {
    items
        .iter()
        .enumerate()
        .map(|(i, it)| {
            let Item::Skill(s) = it else { return HashSet::new() };
            let pre = prerequisites(s.id);
            items
                .iter()
                .enumerate()
                .filter(|(j, o)| {
                    *j != i
                        && matches!(o, Item::Skill(o) if (o.id == s.id && o.level < s.level) || pre.iter().any(|(rid, rl)| *rid == o.id && o.level <= *rl))
                })
                .map(|(j, _)| j)
                .collect()
        })
        .collect()
}

/// Topological sort of one section's skills, always picking the quickest skill whose requirements are met.
/// Attributes don't change within a section, so each skill's training time doesn't depend on the order.
fn sort_section(items: Vec<Item>) -> Vec<Item> {
    let blockers = blockers_of(&items);
    let mut done: HashSet<usize> = HashSet::new();
    let mut result = Vec::with_capacity(items.len());
    let time = |i: usize| items[i].skill().map(|s| s.time).unwrap_or(0.0);
    while result.len() < items.len() {
        let mut next: Option<usize> = None;
        #[allow(clippy::needless_range_loop)]
        for i in 0..items.len() {
            if done.contains(&i) || blockers[i].iter().any(|j| !done.contains(j)) {
                continue;
            }
            // ties keep the original order
            if next.map(|n| time(i) < time(n)).unwrap_or(true) {
                next = Some(i);
            }
        }
        let Some(n) = next else {
            // can't happen with a valid plan, but never drop skills: keep the rest in their current order
            for (i, it) in items.iter().enumerate() {
                if !done.contains(&i) {
                    result.push(it.clone());
                }
            }
            break;
        };
        done.insert(n);
        result.push(items[n].clone());
    }
    result
}

/// Reorders a plan so the skills that train fastest come first, never putting a skill ahead of its prerequisites
/// or a level ahead of the level below it. Remaps and notes stay where they are and split the plan into sections
/// sorted separately: a remap changes training speed from that point on, and notes usually head a group of
/// skills (e.g. a fit). The planner is rebuilt in place.
pub fn sort_by_training_time(p: &mut Planner) {
    let mut sorted = vec![];
    let mut section = vec![];
    for it in &p.queue {
        if it.is_skill() {
            section.push(it.clone());
        } else {
            sorted.extend(sort_section(std::mem::take(&mut section)));
            sorted.push(it.clone());
        }
    }
    sorted.extend(sort_section(section));
    p.rebuild(&sorted);
}

/// A faster order found by [`optimise_order`].
#[derive(Clone, Debug)]
pub struct Optimised {
    /// the new order (add to a reset planner)
    pub queue: Vec<Item>,
    pub time: f64,
    pub before: f64,
    /// skills now in another remap section
    pub moved: usize,
}

/// Looks for an order of the plan that trains it all sooner, keeping every skill after its prerequisites and the
/// level below it. With the same attributes throughout, order makes no difference (each level takes its SP over
/// its speed wherever it is), so only two things can save time:
/// - remaps in the plan: each skill goes in the remap section that trains it fastest (remaps keep their order);
/// - an active accelerator: its bonus adds the same SP/hour to every skill, which helps the slowest skills most,
///   so those go first while it lasts.
///
/// Notes move with the item below them, except those heading the plan. A yearly remap is never pulled to less
/// than a year after the last one. None when no order is at least a minute faster.
pub fn optimise_order(p: &Planner) -> Option<Optimised> {
    let original = p.queue.clone();
    let before = p.time;

    // units: skills and remaps, each with the notes just above it; notes heading the plan (e.g. where it was
    // imported from) and at the very end stay there
    let mut units: Vec<(Item, Vec<Item>)> = vec![];
    let mut leading = vec![];
    let mut notes = vec![];
    for it in &original {
        if matches!(it, Item::Note { .. }) {
            if units.is_empty() {
                leading.push(it.clone());
            } else {
                notes.push(it.clone());
            }
        } else {
            units.push((it.clone(), std::mem::take(&mut notes)));
        }
    }
    let trailing = notes;

    let items: Vec<Item> = units.iter().map(|u| u.0.clone()).collect();
    let remaps: Vec<usize> = items.iter().enumerate().filter(|(_, i)| matches!(i, Item::Remap { .. })).map(|(i, _)| i).collect();
    let sections = remaps.len() + 1;
    let original_section: Vec<usize> = (0..items.len()).map(|i| remaps.iter().filter(|r| **r < i).count()).collect();

    // attributes of each section: the character's own (without an accelerator), then each remap's with its implants
    let mut section_attrs: Vec<AttributeSet> = vec![p.base().attributes];
    for &r in &remaps {
        if let Item::Remap { attributes, implants, .. } = &items[r] {
            section_attrs.push(attributes.plus_all(*implants as f64));
        }
    }
    let skill = |i: usize| -> &SkillItem { items[i].skill().expect("skill") };
    let rate = |i: usize, k: usize| {
        let s = skill(i);
        section_attrs[k].get(s.primary) + section_attrs[k].get(s.secondary) / 2.0
    };
    let cost = |i: usize, k: usize| skill(i).sp / rate(i, k);

    let blockers = blockers_of(&items);
    let dependents: Vec<Vec<usize>> = (0..items.len()).map(|i| (0..items.len()).filter(|j| blockers[*j].contains(&i)).collect()).collect();
    let skill_idx: Vec<usize> = (0..items.len()).filter(|i| items[*i].is_skill()).collect();

    // best section among the allowed ones; ties keep the current section, else the earliest
    let pick = |i: usize, from: usize, to: usize| -> usize {
        let mut best: Option<usize> = None;
        for k in from..=to {
            best = match best {
                None => Some(k),
                Some(b) if cost(i, k) < cost(i, b) - 1e-9 || ((cost(i, k) - cost(i, b)).abs() <= 1e-9 && k == original_section[i]) => Some(k),
                b => b,
            };
        }
        best.unwrap_or(from)
    };

    // two greedy passes (prerequisites first, and dependents first) plus the plan's own sections
    let mut forward = original_section.clone();
    for &i in &skill_idx {
        let from = blockers[i].iter().map(|j| forward[*j]).max().unwrap_or(0);
        forward[i] = pick(i, from.min(sections - 1), sections - 1);
    }
    let mut backward = original_section.clone();
    for &i in skill_idx.iter().rev() {
        let to = dependents[i].iter().map(|j| backward[*j]).min().unwrap_or(sections - 1).min(sections - 1);
        backward[i] = pick(i, 0, to);
    }

    let slowest_first = p.accelerator().is_some();
    let build = |sec: &[usize]| -> Option<Vec<Item>> {
        let mut order = vec![];
        for k in 0..sections {
            if k > 0 {
                let (r, n) = &units[remaps[k - 1]];
                order.extend(n.iter().cloned());
                order.push(r.clone());
            }
            // within a section: requirements first; while an accelerator lasts (section 0), slowest skills first
            let in_section: Vec<usize> = skill_idx.iter().copied().filter(|i| sec[*i] == k).collect();
            let mut done: HashSet<usize> = skill_idx.iter().copied().filter(|i| sec[*i] < k).collect();
            while in_section.iter().any(|i| !done.contains(i)) {
                let mut next: Option<usize> = None;
                for &i in &in_section {
                    if done.contains(&i) || blockers[i].iter().any(|j| !done.contains(j)) {
                        continue;
                    }
                    if next.map(|n| k == 0 && slowest_first && rate(i, 0) < rate(n, 0)).unwrap_or(true) {
                        next = Some(i);
                    }
                }
                // a requirement ended up later: not a valid order
                let n = next?;
                done.insert(n);
                order.extend(units[n].1.iter().cloned());
                order.push(items[n].clone());
            }
        }
        let mut all = leading.clone();
        all.extend(order);
        all.extend(trailing.iter().cloned());
        Some(all)
    };

    // when each remap happens: time since the last one, just before it
    let remap_gaps = |queue: &[Item]| -> Vec<f64> {
        let mut gaps = vec![];
        let mut last = p.last_remap_at_start();
        for it in queue {
            match it {
                Item::Remap { .. } => gaps.push(last),
                Item::Skill(s) => last = s.last_remap,
                _ => {}
            }
        }
        gaps
    };
    let original_gaps = remap_gaps(&original);
    let skill_count = p.skill_count();

    let mut best: Option<(Planner, Vec<usize>)> = None;
    for sec in [&original_section, &forward, &backward] {
        let Some(order) = build(sec) else { continue };
        let trial = Planner::with_items(p.base(), &order);
        // same skills (nothing pulled in as a prerequisite), no yearly remap made too early
        let gaps = remap_gaps(&trial.queue);
        let valid = trial.skill_count() == skill_count
            && gaps.iter().enumerate().all(|(r, g)| !(original_gaps.get(r).map(|o| *o >= REMAP_COOLDOWN).unwrap_or(false) && *g < REMAP_COOLDOWN));
        if valid && best.as_ref().map(|b| trial.time < b.0.time).unwrap_or(true) {
            best = Some((trial, sec.clone()));
        }
    }

    // worth it only if it saves at least a minute
    let (trial, sec) = best?;
    if trial.time > before - 60_000.0 {
        return None;
    }
    Some(Optimised {
        moved: skill_idx.iter().filter(|i| sec[**i] != original_section[**i]).count(),
        queue: trial.queue,
        time: trial.time,
        before,
    })
}

#[cfg(test)]
mod tests {
    use super::super::sim::tests::{attrs, base, id};
    use super::super::sim::{BoostWindow, Item, Planner};
    use super::*;
    use chrono::Utc;

    fn titles(p: &Planner) -> Vec<String> {
        p.queue.iter().map(|i| i.title()).collect()
    }

    #[test]
    fn sort_puts_fast_skills_first_after_prerequisites() {
        let mut p = Planner::with_items(&base(), &[Item::wanted(id("Caldari Cruiser"), 1), Item::wanted(id("Mechanics"), 1)]);
        let n = p.queue.len();
        sort_by_training_time(&mut p);
        assert_eq!(p.queue.len(), n);
        let t = titles(&p);
        let pos = |x: &str| t.iter().position(|y| y == x).unwrap();
        assert!(pos("Caldari Frigate I") < pos("Caldari Frigate II"));
        assert!(pos("Caldari Frigate III") < pos("Caldari Cruiser I"));
        // rank 1 level I trains before the long ones
        assert!(pos("Mechanics I") < pos("Caldari Cruiser I"));
    }

    #[test]
    fn sort_keeps_notes_and_remaps_in_place() {
        let mut p = Planner::new(&base());
        p.plan_skill(id("Caldari Frigate"), 3, None);
        p.add_note("Second", "");
        p.plan_skill(id("Mechanics"), 1, None);
        sort_by_training_time(&mut p);
        // Spaceship Command I and Caldari Frigate I-III, the note, Mechanics I
        assert_eq!(p.queue[4].title(), "Note - Second");
        assert_eq!(p.queue[5].title(), "Mechanics I");
    }

    #[test]
    fn no_faster_order_without_remaps_or_accelerator() {
        let p = Planner::with_items(&base(), &[Item::wanted(id("Caldari Cruiser"), 2), Item::wanted(id("Mechanics"), 3)]);
        assert!(optimise_order(&p).is_none());
    }

    #[test]
    fn moves_skills_to_the_remap_that_suits_them() {
        let b = base();
        let mut p = Planner::new(&b);
        // a Per/Wil remap first and an Int/Mem one later, with skills on the wrong sides
        p.add_remap(attrs(27.0, 17.0, 21.0, 17.0, 17.0), 0);
        p.plan_skill(id("Mechanics"), 4, None);
        p.add_remap(attrs(17.0, 21.0, 17.0, 27.0, 17.0), 0);
        p.plan_skill(id("Spaceship Command"), 4, None);
        let r = optimise_order(&p).unwrap();
        assert!(r.time < r.before);
        assert!(r.moved > 0);
        let q = Planner::with_items(&b, &r.queue);
        let t = titles(&q);
        let first_remap = t.iter().position(|x| x.starts_with("Remap")).unwrap();
        let second = t.iter().rposition(|x| x.starts_with("Remap")).unwrap();
        let sc = t.iter().position(|x| x == "Spaceship Command IV").unwrap();
        let mech = t.iter().position(|x| x == "Mechanics IV").unwrap();
        assert!(first_remap < sc && sc < second);
        assert!(mech > second);
    }

    #[test]
    fn accelerator_trains_slow_skills_first() {
        let mut b = base();
        b.accelerator = Some(BoostWindow { bonus: 10.0, end: Utc::now(), remaining: 24.0 * 3_600_000.0 });
        // Charisma skills train slowest with these attributes
        let p = Planner::with_items(&b, &[Item::wanted(id("Mechanics"), 4), Item::wanted(id("Social"), 4)]);
        let r = optimise_order(&p).unwrap();
        assert!(Planner::with_items(&b, &r.queue).queue[0].title().starts_with("Social"));
    }
}
