//! Training time math shared by skill plans, the skill browser, ship browser and farming pages.

use crate::character::Character;
use crate::sde::{self, Attribute, SkillInfo};
use chrono::Duration;

/// Attribute values used for a training calculation (implants and accelerators already included).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AttributeSet {
    pub intelligence: f64,
    pub memory: f64,
    pub perception: f64,
    pub willpower: f64,
    pub charisma: f64,
}

impl AttributeSet {
    pub fn get(&self, a: Attribute) -> f64 {
        match a {
            Attribute::Intelligence => self.intelligence,
            Attribute::Memory => self.memory,
            Attribute::Perception => self.perception,
            Attribute::Willpower => self.willpower,
            Attribute::Charisma => self.charisma,
        }
    }

    pub fn set(&mut self, a: Attribute, v: f64) {
        match a {
            Attribute::Intelligence => self.intelligence = v,
            Attribute::Memory => self.memory = v,
            Attribute::Perception => self.perception = v,
            Attribute::Willpower => self.willpower = v,
            Attribute::Charisma => self.charisma = v,
        }
    }

    /// Every attribute raised by `bonus` (cerebral accelerator).
    pub fn plus_all(mut self, bonus: f64) -> Self {
        for a in Attribute::ALL {
            self.set(a, self.get(a) + bonus);
        }
        self
    }

    /// The character's current attributes (ESI's figures include active implants).
    pub fn of(c: &Character) -> Self {
        let mut s = AttributeSet::default();
        for a in Attribute::ALL {
            s.set(a, c.attribute(a));
        }
        s
    }

    /// The character's attributes without active implant bonuses.
    pub fn base_of(c: &Character) -> Self {
        let mut s = Self::of(c);
        for imp in c.implants.as_deref().unwrap_or(&[]) {
            for (a, v) in &imp.bonuses {
                s.set(*a, s.get(*a) - *v as f64);
            }
        }
        s
    }
}

/// Omega SP per minute for a skill: primary + secondary / 2. Alpha clones train at half this.
pub fn sp_per_minute(skill: &SkillInfo, attrs: &AttributeSet) -> f64 {
    attrs.get(skill.primary_attribute) + attrs.get(skill.secondary_attribute) / 2.0
}

pub fn sp_per_hour(skill: &SkillInfo, attrs: &AttributeSet, omega: bool) -> f64 {
    sp_per_minute(skill, attrs) * 60.0 * if omega { 1.0 } else { 0.5 }
}

/// Time to train `sp` skill points of `skill`.
pub fn time_for_sp(skill: &SkillInfo, attrs: &AttributeSet, sp: i64, omega: bool) -> Duration {
    let rate = sp_per_hour(skill, attrs, omega);
    if rate <= 0.0 || sp <= 0 {
        return Duration::zero();
    }
    Duration::milliseconds((sp as f64 / rate * 3_600_000.0).round() as i64)
}

/// SP still needed for `c` to have `skill` at `level` (counting SP already in the skill).
pub fn sp_missing(c: &Character, skill_id: i64, level: u8) -> i64 {
    let Some(info) = sde::SKILLS.get(skill_id) else { return 0 };
    let have = c.skill(skill_id).map(|s| s.skillpoints_in_skill).unwrap_or(0);
    (info.sp_for_level(level) - have).max(0)
}

/// Time for `c` (at current attributes) to bring `skill` to `level`, or zero if already there.
pub fn time_to_level(c: &Character, skill_id: i64, level: u8) -> Duration {
    let Some(info) = sde::SKILLS.get(skill_id) else { return Duration::zero() };
    time_for_sp(info, &AttributeSet::of(c), sp_missing(c, skill_id, level), c.is_omega().unwrap_or(true))
}

/// A skill and every prerequisite (recursively), each at the highest level needed, prerequisites first.
pub fn with_prerequisites(skill_id: i64, level: u8) -> Vec<(i64, u8)> {
    fn walk(id: i64, lvl: u8, out: &mut Vec<(i64, u8)>, depth: u8) {
        if depth > 20 {
            return;
        }
        if let Some(info) = sde::SKILLS.get(id) {
            for r in &info.required_skills {
                walk(r.id, r.level, out, depth + 1);
            }
        }
        match out.iter_mut().find(|(i, _)| *i == id) {
            Some(e) => e.1 = e.1.max(lvl),
            None => out.push((id, lvl)),
        }
    }
    let mut out = Vec::new();
    walk(skill_id, level, &mut out, 0);
    out
}

/// What `c` still has to train for a list of (skill, level): each missing level in order, prerequisites first.
pub fn missing_levels(c: &Character, wanted: &[(i64, u8)]) -> Vec<(i64, u8)> {
    let mut all: Vec<(i64, u8)> = Vec::new();
    for &(id, lvl) in wanted {
        for (pid, plvl) in with_prerequisites(id, lvl) {
            match all.iter_mut().find(|(i, _)| *i == pid) {
                Some(e) => e.1 = e.1.max(plvl),
                None => all.push((pid, plvl)),
            }
        }
    }
    let mut out = Vec::new();
    for (id, lvl) in all {
        for l in (c.trained_skill_level(id) + 1)..=lvl {
            out.push((id, l));
        }
    }
    out
}

/// Roman numeral for a skill level.
pub fn roman(level: u8) -> &'static str {
    ["0", "I", "II", "III", "IV", "V"].get(level as usize).copied().unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prerequisites_first() {
        let cruiser = sde::SKILLS.by_name("Caldari Cruiser").unwrap().type_id;
        let list = with_prerequisites(cruiser, 1);
        assert_eq!(list.last().unwrap().0, cruiser);
        assert!(list.len() > 1);
    }

    #[test]
    fn training_time() {
        let s = sde::SKILLS.by_name("Caldari Cruiser").unwrap();
        let attrs = AttributeSet { intelligence: 20.0, memory: 20.0, perception: 27.0, willpower: 21.0, charisma: 19.0 };
        let t = time_for_sp(s, &attrs, s.sp_for_level(1), true);
        assert!(t > Duration::minutes(1));
    }
}
