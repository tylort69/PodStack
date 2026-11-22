//! Static data from EVE's static data export, bundled at build time (see `data/` and
//! `tools/convert_legacy_data.py`): skills and skill groups, ships, Alpha clone skill caps, skillbook prices,
//! and attribute implants / cerebral accelerators.

use serde::Deserialize;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, serde::Serialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Attribute {
    Intelligence,
    Memory,
    Perception,
    Willpower,
    Charisma,
}

impl Attribute {
    pub const ALL: [Attribute; 5] =
        [Attribute::Intelligence, Attribute::Memory, Attribute::Perception, Attribute::Willpower, Attribute::Charisma];

    pub fn name(self) -> &'static str {
        match self {
            Attribute::Intelligence => "Intelligence",
            Attribute::Memory => "Memory",
            Attribute::Perception => "Perception",
            Attribute::Willpower => "Willpower",
            Attribute::Charisma => "Charisma",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Attribute::Intelligence => "intelligence",
            Attribute::Memory => "memory",
            Attribute::Perception => "perception",
            Attribute::Willpower => "willpower",
            Attribute::Charisma => "charisma",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RequiredSkill {
    pub id: i64,
    pub level: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillInfo {
    pub type_id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub market_group_id: i64,
    #[serde(default)]
    pub market_group_name: String,
    pub primary_attribute: Attribute,
    pub secondary_attribute: Attribute,
    pub training_time_multiplier: u32,
    #[serde(default)]
    pub required_skills: Vec<RequiredSkill>,
}

/// SP needed to reach a level: 250 × rank × √32^(level−1).
pub fn sp_for_level(rank: u32, level: u8) -> i64 {
    if level == 0 {
        return 0;
    }
    (250.0 * rank as f64 * 32f64.sqrt().powi(level as i32 - 1)).round() as i64
}

impl SkillInfo {
    pub fn sp_for_level(&self, level: u8) -> i64 {
        sp_for_level(self.training_time_multiplier, level)
    }
    /// SP per minute for given attribute values (full Omega speed).
    pub fn sp_per_minute(&self, primary: f64, secondary: f64) -> f64 {
        primary + secondary / 2.0
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SkillGroup {
    pub market_group_id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub types: Vec<i64>,
}

#[derive(Deserialize)]
struct SkillsFile {
    skills: HashMap<String, SkillInfo>,
    groups: Vec<SkillGroup>,
}

pub struct Skills {
    pub by_id: HashMap<i64, SkillInfo>,
    pub by_name: HashMap<String, i64>,
    pub groups: Vec<SkillGroup>,
}

impl Skills {
    pub fn get(&self, id: i64) -> Option<&SkillInfo> {
        self.by_id.get(&id)
    }
    pub fn by_name(&self, name: &str) -> Option<&SkillInfo> {
        self.by_name.get(&name.to_lowercase()).and_then(|id| self.by_id.get(id))
    }
    pub fn group_of(&self, skill_id: i64) -> Option<&SkillGroup> {
        let mg = self.by_id.get(&skill_id)?.market_group_id;
        self.groups.iter().find(|g| g.market_group_id == mg)
    }
}

pub static SKILLS: LazyLock<Skills> = LazyLock::new(|| {
    let f: SkillsFile = serde_json::from_str(include_str!("../../../data/skills.json")).expect("data/skills.json");
    let by_id: HashMap<i64, SkillInfo> = f.skills.into_values().map(|s| (s.type_id, s)).collect();
    let by_name = by_id.values().map(|s| (s.name.to_lowercase(), s.type_id)).collect();
    let mut groups = f.groups;
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    Skills { by_id, by_name, groups }
});

/// The highest level an Alpha clone can train each skill to, by skill name.
pub static ALPHA_CAPS: LazyLock<HashMap<String, u8>> =
    LazyLock::new(|| serde_json::from_str(include_str!("../../../data/alpha_caps.json")).expect("data/alpha_caps.json"));

pub fn alpha_cap(skill_name: &str) -> u8 {
    ALPHA_CAPS.get(skill_name).copied().unwrap_or(0)
}

/// NPC base price of each skillbook, by type id.
pub static SKILL_PRICES: LazyLock<HashMap<i64, f64>> = LazyLock::new(|| {
    let m: HashMap<String, f64> =
        serde_json::from_str(include_str!("../../../data/skill_prices.json")).expect("data/skill_prices.json");
    m.into_iter().filter_map(|(k, v)| k.parse().ok().map(|k| (k, v))).collect()
});

#[derive(Debug, Clone, Deserialize)]
pub struct ShipInfo {
    pub type_id: i64,
    pub name: String,
    pub group_id: i64,
    pub group: String,
    #[serde(default)]
    pub race_id: Option<i64>,
    #[serde(default)]
    pub meta: String,
    #[serde(default)]
    pub skills: Vec<RequiredSkill>,
    #[serde(default)]
    pub skins: Vec<ShipSkin>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ShipSkin {
    pub id: i64,
    pub name: String,
}

#[derive(Deserialize)]
struct ShipsFile {
    ships: HashMap<String, ShipInfo>,
    races: HashMap<String, String>,
}

pub struct Ships {
    pub by_id: HashMap<i64, ShipInfo>,
    pub races: HashMap<i64, String>,
}

pub static SHIPS: LazyLock<Ships> = LazyLock::new(|| {
    let f: ShipsFile = serde_json::from_str(include_str!("../../../data/ships.json")).expect("data/ships.json");
    Ships {
        by_id: f.ships.into_values().map(|s| (s.type_id, s)).collect(),
        races: f.races.into_iter().filter_map(|(k, v)| k.parse().ok().map(|k| (k, v))).collect(),
    }
});

/// Attribute implant slots 1-5 and the attribute each boosts.
pub const IMPLANT_SLOTS: [(u8, Attribute, &str); 5] = [
    (1, Attribute::Perception, "Ocular Filter"),
    (2, Attribute::Memory, "Memory Augmentation"),
    (3, Attribute::Willpower, "Neural Boost"),
    (4, Attribute::Intelligence, "Cybernetic Subprocessor"),
    (5, Attribute::Charisma, "Social Adaptation Chip"),
];

pub struct ImplantGrade {
    pub bonus: u8,
    pub grade: &'static str,
}

pub const IMPLANT_GRADES: [ImplantGrade; 5] = [
    ImplantGrade { bonus: 1, grade: "Limited" },
    ImplantGrade { bonus: 2, grade: "Limited Beta" },
    ImplantGrade { bonus: 3, grade: "Basic" },
    ImplantGrade { bonus: 4, grade: "Standard" },
    ImplantGrade { bonus: 5, grade: "Improved" },
];

/// Name of an attribute implant, e.g. ("Ocular Filter", 3) -> "Ocular Filter - Basic".
pub fn implant_name(base: &str, bonus: u8) -> String {
    match bonus {
        1 => format!("Limited {base}"),
        2 => format!("Limited {base} - Beta"),
        3 => format!("{base} - Basic"),
        4 => format!("{base} - Standard"),
        _ => format!("{base} - Improved"),
    }
}

/// Dogma attribute ids for implant attribute bonuses (charisma, intelligence, memory, perception, willpower).
pub const DOGMA_ATTRIBUTE_BONUS: [(i64, Attribute); 5] = [
    (175, Attribute::Charisma),
    (176, Attribute::Intelligence),
    (177, Attribute::Memory),
    (178, Attribute::Perception),
    (179, Attribute::Willpower),
];
/// Dogma attribute: booster duration (ms).
pub const DOGMA_BOOSTER_DURATION: i64 = 330;
/// Dogma attribute: implant slot.
pub const DOGMA_IMPLANT_SLOT: i64 = 331;

pub struct Accelerator {
    pub type_id: i64,
    pub name: &'static str,
    pub bonus: u8,
    pub days: f64,
}

/// Cerebral accelerators with a fixed duration, weakest first (see legacy/resources/clone_items.js).
pub const ACCELERATORS: &[Accelerator] = &[
    Accelerator { type_id: 57242, name: "Festival-only Outpost Cerebral Accelerator I", bonus: 1, days: 1.0 },
    Accelerator { type_id: 57243, name: "Festival-only Outpost Cerebral Accelerator III", bonus: 1, days: 3.0 },
    Accelerator { type_id: 57244, name: "Festival-only Outpost Cerebral Accelerator VII", bonus: 1, days: 7.0 },
    Accelerator { type_id: 72238, name: "Basic 'Boost' Cerebral Accelerator", bonus: 2, days: 2.0 },
    Accelerator { type_id: 57245, name: "Festival-only Vanguard Cerebral Accelerator I", bonus: 3, days: 1.0 },
    Accelerator { type_id: 57246, name: "Festival-only Vanguard Cerebral Accelerator III", bonus: 3, days: 3.0 },
    Accelerator { type_id: 57247, name: "Festival-only Vanguard Cerebral Accelerator VII", bonus: 3, days: 7.0 },
    Accelerator { type_id: 72239, name: "Standard 'Boost' Cerebral Accelerator", bonus: 4, days: 4.0 },
    Accelerator { type_id: 57248, name: "Festival-only Commander Cerebral Accelerator I", bonus: 5, days: 1.0 },
    Accelerator { type_id: 57249, name: "Festival-only Commander Cerebral Accelerator III", bonus: 5, days: 3.0 },
    Accelerator { type_id: 57250, name: "Festival-only Commander Cerebral Accelerator VII", bonus: 5, days: 7.0 },
    Accelerator { type_id: 72240, name: "Advanced 'Boost' Cerebral Accelerator", bonus: 6, days: 6.0 },
    Accelerator { type_id: 72241, name: "Specialist 'Boost' Cerebral Accelerator", bonus: 8, days: 8.0 },
    Accelerator { type_id: 55826, name: "Expert Cerebral Accelerator", bonus: 8, days: 12.5 },
    Accelerator { type_id: 57251, name: "Festival-only Marshal Cerebral Accelerator I", bonus: 10, days: 1.0 },
    Accelerator { type_id: 48582, name: "Master-at-Arms Cerebral Accelerator", bonus: 10, days: 1.0 },
    Accelerator { type_id: 57252, name: "Festival-only Marshal Cerebral Accelerator III", bonus: 10, days: 3.0 },
    Accelerator { type_id: 57253, name: "Festival-only Marshal Cerebral Accelerator VII", bonus: 10, days: 7.0 },
    Accelerator { type_id: 72242, name: "Expert 'Boost' Cerebral Accelerator", bonus: 10, days: 10.0 },
    Accelerator { type_id: 77919, name: "Genius 'Boost' Cerebral Accelerator", bonus: 12, days: 12.0 },
    Accelerator { type_id: 56659, name: "Festival-only New Eden Vanguard Pack", bonus: 3, days: 7.0 },
    Accelerator { type_id: 77934, name: "Heimatar Rise Accelerator", bonus: 6, days: 2.0 },
    Accelerator { type_id: 49753, name: "Onslaught Accelerator", bonus: 10, days: 3.0 },
    Accelerator { type_id: 56661, name: "Festival-only New Eden Commander Pack", bonus: 10, days: 7.0 },
    Accelerator { type_id: 85279, name: "Totality Day Accelerator", bonus: 13, days: 3.0 },
    Accelerator { type_id: 56662, name: "Festival-only New Eden Marshal Pack", bonus: 15, days: 7.0 },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_static_data() {
        assert!(SKILLS.by_id.len() > 400);
        assert!(SKILLS.by_name("Caldari Cruiser").is_some());
        assert!(SHIPS.by_id.len() > 300);
        assert!(!ALPHA_CAPS.is_empty());
        assert!(!SKILL_PRICES.is_empty());
    }

    #[test]
    fn sp_table() {
        assert_eq!(sp_for_level(1, 5), 256_000);
        assert_eq!(sp_for_level(1, 1), 250);
        assert_eq!(sp_for_level(5, 4), 226_274);
    }
}
