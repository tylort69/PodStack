//! Static data from EVE's static data export, bundled at build time (see `data/` and
//! `tools/convert_legacy_data.py`): skills and skill groups, ships, Alpha clone skill caps, skillbook prices,
//! and attribute implants / cerebral accelerators.

use serde::Deserialize;
use std::collections::HashMap;
use crate::static_data::{self, Live};
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

/// A type sold on the market.
#[derive(Debug, Clone)]
pub struct MarketType {
    pub id: i64,
    pub name: String,
    pub group: i64,
    /// Packaged m³.
    pub volume: f64,
    /// 1 Tech I, 2 Tech II, 3 Storyline, 4 Faction, 5 Officer, 6 Deadspace, 14 Tech III, …
    pub meta_group: Option<i64>,
}

/// A node of the market's group tree: groups hold either subgroups or types.
#[derive(Debug, Clone)]
pub struct MarketGroup {
    pub id: i64,
    pub parent: Option<i64>,
    pub name: String,
    pub children: Vec<i64>,
    pub types: Vec<i64>,
}

/// Every type on the market and the market's group tree, from data/market_types.json and data/market_groups.json
/// (converted by `static_data`).
pub struct MarketTypes {
    pub types: Vec<MarketType>,
    by_id: HashMap<i64, usize>,
    by_name: HashMap<String, usize>,
    pub groups: HashMap<i64, MarketGroup>,
    pub roots: Vec<i64>,
}

impl MarketTypes {
    pub fn get(&self, id: i64) -> Option<&MarketType> {
        self.by_id.get(&id).map(|&i| &self.types[i])
    }

    /// Exact name, ignoring case and surrounding spaces.
    pub fn by_name(&self, name: &str) -> Option<&MarketType> {
        self.by_name.get(&name.trim().to_lowercase()).map(|&i| &self.types[i])
    }

    /// Types whose name contains every word of `query`: exact matches first, then names starting with the query,
    /// then shorter names.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&MarketType> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<(u8, usize, &MarketType)> = self
            .types
            .iter()
            .filter_map(|t| {
                let n = t.name.to_lowercase();
                if !words.iter().all(|w| n.contains(w)) {
                    return None;
                }
                let rank = if n == q { 0 } else if n.starts_with(&q) { 1 } else if n.contains(&q) { 2 } else { 3 };
                Some((rank, t.name.len(), t))
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.name.cmp(&b.2.name)));
        hits.into_iter().take(limit).map(|h| h.2).collect()
    }

    /// Group names from the root down to `group` ("Manufacture & Research", "Materials", "Minerals").
    pub fn path(&self, group: i64) -> Vec<&MarketGroup> {
        let mut out = Vec::new();
        let mut cur = Some(group);
        while let Some(id) = cur {
            let Some(g) = self.groups.get(&id) else { break };
            out.push(g);
            cur = g.parent;
            if out.len() > 16 {
                break;
            }
        }
        out.reverse();
        out
    }
}

/// Market types and groups: from CCP's latest static data once downloaded, else the bundled snapshot.
pub static MARKET: Live<MarketTypes> = Live::new(load_market);

fn load_market() -> MarketTypes {
    let raw_types: Vec<(i64, String, i64, f64, Option<i64>)> =
        serde_json::from_str(&static_data::read("market_types.json", include_str!("../../../data/market_types.json")))
            .expect("market_types.json");
    let raw_groups: Vec<(i64, Option<i64>, String)> =
        serde_json::from_str(&static_data::read("market_groups.json", include_str!("../../../data/market_groups.json")))
            .expect("market_groups.json");
    let types: Vec<MarketType> = raw_types
        .into_iter()
        .map(|(id, name, group, volume, meta_group)| MarketType { id, name, group, volume, meta_group })
        .collect();
    let mut groups: HashMap<i64, MarketGroup> = raw_groups
        .into_iter()
        .map(|(id, parent, name)| (id, MarketGroup { id, parent, name, children: Vec::new(), types: Vec::new() }))
        .collect();
    let ids: Vec<(i64, Option<i64>)> = groups.values().map(|g| (g.id, g.parent)).collect();
    let mut roots = Vec::new();
    for (id, parent) in ids {
        match parent.filter(|p| groups.contains_key(p)) {
            Some(p) => groups.get_mut(&p).unwrap().children.push(id),
            None => roots.push(id),
        }
    }
    for t in &types {
        if let Some(g) = groups.get_mut(&t.group) {
            g.types.push(t.id);
        }
    }
    let names: HashMap<i64, String> = groups.values().map(|g| (g.id, g.name.clone())).collect();
    let type_names: HashMap<i64, &str> = types.iter().map(|t| (t.id, t.name.as_str())).collect();
    for g in groups.values_mut() {
        g.children.sort_by(|a, b| names[a].cmp(&names[b]));
        g.types.sort_by(|a, b| type_names[a].cmp(type_names[b]));
    }
    roots.sort_by(|a, b| names[a].cmp(&names[b]));
    let by_id = types.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    let by_name = types.iter().enumerate().map(|(i, t)| (t.name.to_lowercase(), i)).collect();
    MarketTypes { types, by_id, by_name, groups, roots }
}

/// A known-space solar system.
#[derive(Debug, Clone)]
pub struct SolarSystem {
    pub id: i64,
    pub name: String,
    pub region: i64,
    /// True security status; EVE shows it rounded to one decimal (see `display_security`).
    pub security: f64,
}

impl SolarSystem {
    /// Security as EVE shows it: rounded to one decimal, except that anything just above 0 counts as 0.1.
    pub fn display_security(&self) -> f64 {
        if self.security > 0.0 && self.security < 0.05 {
            0.1
        } else {
            (self.security * 10.0).round() / 10.0
        }
    }
    pub fn security_class(&self) -> SecurityClass {
        let s = self.display_security();
        if s >= 0.5 {
            SecurityClass::High
        } else if s > 0.0 {
            SecurityClass::Low
        } else {
            SecurityClass::Null
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, Deserialize)]
pub enum SecurityClass {
    High,
    Low,
    Null,
}

/// Known-space regions and systems, from data/universe.json.
pub struct Map {
    /// (id, name), by name.
    pub regions: Vec<(i64, String)>,
    systems: HashMap<i64, SolarSystem>,
}

impl Map {
    pub fn system(&self, id: i64) -> Option<&SolarSystem> {
        self.systems.get(&id)
    }
    pub fn region_name(&self, id: i64) -> Option<&str> {
        self.regions.iter().find(|r| r.0 == id).map(|r| r.1.as_str())
    }
    /// Regions players trade in: known space without the Jove regions nobody can reach.
    pub fn market_regions(&self) -> impl Iterator<Item = &(i64, String)> {
        self.regions.iter().filter(|(id, _)| !UNREACHABLE_REGIONS.contains(id))
    }
}

/// Jove regions: no stargate leads in, so ESI answers for them but nothing trades there.
const UNREACHABLE_REGIONS: &[i64] = &[10000004, 10000017, 10000019];

/// Regions and systems: from CCP's latest static data once downloaded, else the bundled snapshot.
pub static MAP: Live<Map> = Live::new(load_map);

fn load_map() -> Map {
    #[derive(Deserialize)]
    struct File {
        regions: Vec<(i64, String)>,
        systems: Vec<(i64, String, i64, f64)>,
    }
    let f: File = serde_json::from_str(&static_data::read("universe.json", include_str!("../../../data/universe.json")))
        .expect("universe.json");
    let mut regions = f.regions;
    regions.sort_by(|a, b| a.1.cmp(&b.1));
    let systems = f
        .systems
        .into_iter()
        .map(|(id, name, region, security)| (id, SolarSystem { id, name, region, security }))
        .collect();
    Map { regions, systems }
}

/// The five empire trade hubs: (name, station, system, region).
pub const TRADE_HUBS: [(&str, i64, i64, i64); 5] = [
    ("Jita", 60003760, 30000142, 10000002),
    ("Amarr", 60008494, 30002187, 10000043),
    ("Dodixie", 60011866, 30002659, 10000032),
    ("Rens", 60004588, 30002510, 10000030),
    ("Hek", 60005686, 30002053, 10000042),
];

#[cfg(test)]
mod market_tests {
    use super::*;

    #[test]
    fn market_data() {
        let trit = MARKET.get(34).unwrap();
        assert_eq!(trit.name, "Tritanium");
        let path: Vec<&str> = MARKET.path(trit.group).iter().map(|g| g.name.as_str()).collect();
        assert_eq!(path, ["Manufacture & Research", "Materials", "Minerals"]);
        assert_eq!(MARKET.by_name(" tritanium ").map(|t| t.id), Some(34));
        assert_eq!(MARKET.search("tritan", 5)[0].id, 34);
        assert!(MARKET.roots.iter().any(|r| MARKET.groups[r].name == "Ships"));
        let jita = MAP.system(30000142).unwrap();
        assert_eq!(jita.name, "Jita");
        assert_eq!(jita.security_class(), SecurityClass::High);
        assert_eq!(MAP.region_name(10000002), Some("The Forge"));
        for (_, station, system, region) in TRADE_HUBS {
            assert_eq!(MAP.system(system).unwrap().region, region, "{station}");
        }
    }
}
