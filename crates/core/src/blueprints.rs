//! Industry static data: every blueprint and reaction formula with its activities, the structures and rigs that
//! change jobs, invention decryptors and the per-level bonuses of industry skills. All of it comes from CCP's static
//! data (see `static_data`), so balance changes arrive with CCP's next export rather than a PodStack release.

use crate::static_data::{self, IndustryFile, Live};
use std::collections::HashMap;

/// The industry activities blueprints have.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum Activity {
    Manufacturing,
    Reaction,
    Invention,
    Copying,
    ResearchMaterial,
    ResearchTime,
}

impl Activity {
    pub const ALL: [Activity; 6] =
        [Activity::Manufacturing, Activity::Reaction, Activity::Invention, Activity::Copying, Activity::ResearchMaterial, Activity::ResearchTime];

    /// The name the static data uses.
    pub fn key(self) -> &'static str {
        match self {
            Activity::Manufacturing => "manufacturing",
            Activity::Reaction => "reaction",
            Activity::Invention => "invention",
            Activity::Copying => "copying",
            Activity::ResearchMaterial => "research_material",
            Activity::ResearchTime => "research_time",
        }
    }

    pub fn from_key(key: &str) -> Option<Activity> {
        Activity::ALL.into_iter().find(|a| a.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Activity::Manufacturing => "Manufacturing",
            Activity::Reaction => "Reaction",
            Activity::Invention => "Invention",
            Activity::Copying => "Copying",
            Activity::ResearchMaterial => "ME research",
            Activity::ResearchTime => "TE research",
        }
    }

    /// The name of this activity's cost index in ESI's `industry/systems`.
    pub fn index_key(self) -> &'static str {
        match self {
            Activity::Manufacturing => "manufacturing",
            Activity::Reaction => "reaction",
            Activity::Invention => "invention",
            Activity::Copying => "copying",
            Activity::ResearchMaterial => "researching_material_efficiency",
            Activity::ResearchTime => "researching_time_efficiency",
        }
    }

    /// ESI's activity id on industry jobs.
    pub fn from_job_activity(id: i64) -> Option<Activity> {
        match id {
            1 => Some(Activity::Manufacturing),
            3 => Some(Activity::ResearchTime),
            4 => Some(Activity::ResearchMaterial),
            5 => Some(Activity::Copying),
            8 => Some(Activity::Invention),
            9 | 11 => Some(Activity::Reaction),
            _ => None,
        }
    }
}

/// What a bonus changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BonusKind {
    Material,
    Time,
    Cost,
}

impl BonusKind {
    fn from_key(k: &str) -> Option<BonusKind> {
        match k {
            "material" => Some(BonusKind::Material),
            "time" => Some(BonusKind::Time),
            "cost" => Some(BonusKind::Cost),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Product {
    pub type_id: i64,
    pub quantity: i64,
    /// Base chance of success, for invention.
    pub probability: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct ActivityData {
    /// Seconds per run, before any bonus.
    pub time: i64,
    /// (type, quantity per run), before material efficiency.
    pub materials: Vec<(i64, i64)>,
    pub products: Vec<Product>,
    /// (skill, level) needed to start the job.
    pub skills: Vec<(i64, i64)>,
}

#[derive(Clone, Debug)]
pub struct Blueprint {
    pub id: i64,
    /// Most runs a copy can have.
    pub max_runs: i64,
    pub activities: HashMap<Activity, ActivityData>,
}

impl Blueprint {
    pub fn activity(&self, a: Activity) -> Option<&ActivityData> {
        self.activities.get(&a)
    }

    /// The activity that makes things (manufacturing or reaction) and its first product.
    pub fn output(&self) -> Option<(Activity, &ActivityData, &Product)> {
        [Activity::Manufacturing, Activity::Reaction]
            .into_iter()
            .find_map(|a| self.activities.get(&a).and_then(|d| d.products.first().map(|p| (a, d, p))))
    }

    /// Reaction formulas are blueprints of the Reaction activity; they can't be researched or copied.
    pub fn is_reaction(&self) -> bool {
        self.activities.contains_key(&Activity::Reaction)
    }
}

#[derive(Clone, Debug)]
pub struct IndustryType {
    pub id: i64,
    pub name: String,
    pub group: i64,
    pub published: bool,
}

#[derive(Clone, Debug)]
pub struct Structure {
    pub id: i64,
    pub name: String,
    pub group: i64,
    /// Which rigs fit: 2 medium, 3 large, 4 extra large.
    pub rig_size: i64,
    /// (activity, kind, multiplier such as 0.99)
    pub bonuses: Vec<(Activity, BonusKind, f64)>,
}

impl Structure {
    /// The structure's own multiplier for an activity (1.0 when it has none).
    pub fn multiplier(&self, activity: Activity, kind: BonusKind) -> f64 {
        self.bonuses.iter().filter(|b| b.0 == activity && b.1 == kind).map(|b| b.2).product()
    }

    pub fn supports(&self, activity: Activity) -> bool {
        // Refineries only react; engineering complexes and citadels do everything else. The static data lists the
        // activities a structure has bonuses for; citadels have no reaction bonus and can't react.
        match activity {
            Activity::Reaction => self.group == crate::blueprints::REFINERY_GROUP,
            _ => self.group != crate::blueprints::REFINERY_GROUP || self.bonuses.iter().any(|b| b.0 == activity),
        }
    }
}

pub const ENGINEERING_COMPLEX_GROUP: i64 = 1404;
pub const REFINERY_GROUP: i64 = 1406;
pub const CITADEL_GROUP: i64 = 1657;

#[derive(Clone, Debug)]
pub struct Rig {
    pub id: i64,
    pub name: String,
    /// 2 medium, 3 large, 4 extra large (the structure's rig size).
    pub size: i64,
    pub tech: i64,
    /// Bonus multipliers in high, low and null security; `None` where the rig has no effect.
    pub security: [Option<f64>; 3],
    /// (activity, kind, percent such as -2.0, target filter)
    pub bonuses: Vec<(Activity, BonusKind, f64, Option<i64>)>,
}

#[derive(Clone, Debug)]
pub struct Filter {
    pub id: i64,
    pub name: String,
    pub categories: Vec<i64>,
    pub groups: Vec<i64>,
}

#[derive(Clone, Debug)]
pub struct Decryptor {
    pub id: i64,
    pub name: String,
    pub probability: f64,
    pub me: i64,
    pub te: i64,
    pub runs: i64,
}

pub struct IndustryData {
    pub blueprints: HashMap<i64, Blueprint>,
    /// Product type → (blueprint, activity) that makes it.
    made_by: HashMap<i64, (i64, Activity)>,
    /// Invented blueprint → the blueprints it can be invented from.
    invented_from: HashMap<i64, Vec<i64>>,
    types: HashMap<i64, IndustryType>,
    /// group → (category, name)
    groups: HashMap<i64, (i64, String)>,
    pub filters: HashMap<i64, Filter>,
    pub structures: Vec<Structure>,
    pub rigs: Vec<Rig>,
    pub decryptors: Vec<Decryptor>,
    /// skill → (attribute, value per level)
    skill_bonuses: HashMap<i64, Vec<(String, f64)>>,
}

impl IndustryData {
    pub fn blueprint(&self, id: i64) -> Option<&Blueprint> {
        self.blueprints.get(&id)
    }

    /// The blueprint or formula that makes `product`, with the activity.
    pub fn maker(&self, product: i64) -> Option<(&Blueprint, Activity)> {
        self.made_by.get(&product).and_then(|(b, a)| self.blueprints.get(b).map(|b| (b, *a)))
    }

    /// The blueprints `blueprint` can be invented from (a T2 blueprint from its T1 version, T3 from relics).
    pub fn invention_sources(&self, blueprint: i64) -> Vec<&Blueprint> {
        self.invented_from.get(&blueprint).map(|v| v.iter().filter_map(|b| self.blueprints.get(b)).collect()).unwrap_or_default()
    }

    pub fn get_type(&self, id: i64) -> Option<&IndustryType> {
        self.types.get(&id)
    }

    pub fn type_name(&self, id: i64) -> String {
        self.types
            .get(&id)
            .map(|t| t.name.clone())
            .or_else(|| crate::sde::MARKET.get(id).map(|t| t.name.clone()))
            .unwrap_or_else(|| format!("Type #{id}"))
    }

    pub fn group_name(&self, group: i64) -> Option<&str> {
        self.groups.get(&group).map(|g| g.1.as_str())
    }

    pub fn category_of(&self, type_id: i64) -> Option<i64> {
        self.types.get(&type_id).and_then(|t| self.groups.get(&t.group)).map(|g| g.0)
    }

    /// Whether a rig bonus with this filter applies to jobs making `product` (no filter: every product).
    pub fn filter_matches(&self, filter: Option<i64>, product: i64) -> bool {
        let Some(id) = filter else { return true };
        let Some(f) = self.filters.get(&id) else { return false };
        let group = self.types.get(&product).map(|t| t.group);
        group.is_some_and(|g| f.groups.contains(&g)) || self.category_of(product).is_some_and(|c| f.categories.contains(&c))
    }

    /// A skill's bonus per level for a dogma attribute (e.g. Industry's "manufacturingTimeBonus" is -4).
    pub fn skill_bonus(&self, skill: i64, attribute: &str) -> Option<f64> {
        self.skill_bonuses.get(&skill)?.iter().find(|(a, _)| a == attribute).map(|(_, v)| *v)
    }

    /// Every skill with a bonus for `attribute`, with the bonus per level.
    pub fn skills_with(&self, attribute: &str) -> Vec<(i64, f64)> {
        let mut out: Vec<(i64, f64)> = self
            .skill_bonuses
            .iter()
            .filter_map(|(s, list)| list.iter().find(|(a, _)| a == attribute).map(|(_, v)| (*s, *v)))
            .collect();
        out.sort_by_key(|s| s.0);
        out
    }

    pub fn structure(&self, id: i64) -> Option<&Structure> {
        self.structures.iter().find(|s| s.id == id)
    }

    pub fn rig(&self, id: i64) -> Option<&Rig> {
        self.rigs.iter().find(|r| r.id == id)
    }

    pub fn decryptor(&self, id: i64) -> Option<&Decryptor> {
        self.decryptors.iter().find(|d| d.id == id)
    }

    /// Products of manufacturing and reactions whose name holds every word of `query`, best matches first:
    /// (product, blueprint, activity).
    pub fn search_products(&self, query: &str, limit: usize) -> Vec<(i64, i64, Activity)> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let words: Vec<&str> = q.split_whitespace().collect();
        let mut hits: Vec<(u8, usize, String, i64, i64, Activity)> = self
            .made_by
            .iter()
            .filter_map(|(product, (bp, act))| {
                let name = self.type_name(*product);
                let n = name.to_lowercase();
                if !words.iter().all(|w| n.contains(w)) {
                    return None;
                }
                let rank = if n == q { 0 } else if n.starts_with(&q) { 1 } else { 2 };
                Some((rank, name.len(), name, *product, *bp, *act))
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then_with(|| a.2.cmp(&b.2)));
        hits.into_iter().take(limit).map(|h| (h.3, h.4, h.5)).collect()
    }
}

/// Blueprints and everything else industry needs: CCP's latest static data once downloaded, else the bundled
/// snapshot.
pub static INDUSTRY: Live<IndustryData> = Live::new(load);

fn load() -> IndustryData {
    let text = static_data::read("industry.json", include_str!("../../../data/industry.json"));
    let f: IndustryFile = serde_json::from_str(&text).expect("industry.json");
    from_file(f)
}

pub fn from_file(f: IndustryFile) -> IndustryData {
    let types: HashMap<i64, IndustryType> =
        f.types.into_iter().map(|(id, name, group, published)| (id, IndustryType { id, name, group, published })).collect();
    let mut blueprints = HashMap::new();
    let mut made_by: HashMap<i64, (i64, Activity)> = HashMap::new();
    let mut invented_from: HashMap<i64, Vec<i64>> = HashMap::new();
    for (id, max_runs, acts) in f.blueprints {
        let activities: HashMap<Activity, ActivityData> = acts
            .into_iter()
            .filter_map(|(k, a)| {
                Activity::from_key(&k).map(|act| {
                    let data = ActivityData {
                        time: a.time,
                        materials: a.materials,
                        products: a.products.into_iter().map(|(type_id, quantity, probability)| Product { type_id, quantity, probability }).collect(),
                        skills: a.skills,
                    };
                    (act, data)
                })
            })
            .collect();
        for act in [Activity::Manufacturing, Activity::Reaction] {
            if let Some(p) = activities.get(&act).and_then(|d| d.products.first()) {
                // A few items have more than one blueprint (old event versions); prefer one that's published.
                let better = match made_by.get(&p.type_id) {
                    None => true,
                    Some((old, _)) => !types.get(old).is_some_and(|t| t.published) && types.get(&id).is_some_and(|t| t.published),
                };
                if better {
                    made_by.insert(p.type_id, (id, act));
                }
            }
        }
        if let Some(inv) = activities.get(&Activity::Invention) {
            for p in &inv.products {
                invented_from.entry(p.type_id).or_default().push(id);
            }
        }
        blueprints.insert(id, Blueprint { id, max_runs, activities });
    }
    let groups = f.groups.into_iter().map(|(id, cat, name)| (id, (cat, name))).collect();
    let filters = f.filters.into_iter().map(|(id, name, categories, groups)| (id, Filter { id, name, categories, groups })).collect();
    let structures = f
        .structures
        .into_iter()
        .map(|s| Structure {
            id: s.id,
            name: s.name,
            group: s.group,
            rig_size: s.rig_size,
            bonuses: s
                .bonuses
                .into_iter()
                .filter_map(|(a, k, v)| Some((Activity::from_key(&a)?, BonusKind::from_key(&k)?, v)))
                .collect(),
        })
        .collect();
    let rigs = f
        .rigs
        .into_iter()
        .map(|r| Rig {
            id: r.id,
            name: r.name,
            size: r.size,
            tech: r.tech,
            security: [r.security.0, r.security.1, r.security.2],
            bonuses: r
                .bonuses
                .into_iter()
                .filter_map(|(a, k, v, filter)| Some((Activity::from_key(&a)?, BonusKind::from_key(&k)?, v, filter)))
                .collect(),
        })
        .collect();
    let decryptors =
        f.decryptors.into_iter().map(|(id, name, probability, me, te, runs)| Decryptor { id, name, probability, me, te, runs }).collect();
    let mut skill_bonuses: HashMap<i64, Vec<(String, f64)>> = HashMap::new();
    for (skill, attr, value) in f.skill_bonuses {
        skill_bonuses.entry(skill).or_default().push((attr, value));
    }
    IndustryData { blueprints, made_by, invented_from, types, groups, filters, structures, rigs, decryptors, skill_bonuses }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_data() {
        let d = &*INDUSTRY;
        // Rifter (587) is made from the Rifter Blueprint (691) with minerals
        let (bp, act) = d.maker(587).expect("rifter blueprint");
        assert_eq!(act, Activity::Manufacturing);
        assert_eq!(bp.id, 691);
        assert!(bp.activity(Activity::Manufacturing).unwrap().materials.iter().any(|m| m.0 == 34));
        // Jaguar's blueprint is invented from the Rifter's
        let (jaguar_bp, _) = d.maker(11400).expect("jaguar blueprint");
        assert!(d.invention_sources(jaguar_bp.id).iter().any(|b| b.id == 691));
        // reactions
        assert!(d.maker(16672).is_some_and(|(b, a)| a == Activity::Reaction && b.is_reaction()));
        // the Raitaru's 1% material bonus and Industry's 4% per level
        let raitaru = d.structure(35825).unwrap();
        assert!((raitaru.multiplier(Activity::Manufacturing, BonusKind::Material) - 0.99).abs() < 1e-9);
        assert_eq!(d.skill_bonus(3380, "manufacturingTimeBonus"), Some(-4.0));
        assert_eq!(d.decryptors.len(), 8);
        assert!(d.rigs.iter().any(|r| r.bonuses.iter().any(|b| b.0 == Activity::Reaction)));
        assert!(d.search_products("rifter", 5).iter().any(|h| h.0 == 587));
    }
}
