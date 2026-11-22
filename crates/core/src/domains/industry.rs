//! Industry jobs and planetary colonies.
//!
//! Jobs come with blueprint and product names and the facility resolved to a [`Place`]. Each colony is loaded with
//! its layout: extractors (what they pull and when they stop), factories (by schematic) and the goods stored on the
//! planet.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use crate::universe::Place;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

pub const DOMAINS: &[DomainSpec] = &[
    // ESI caches jobs for 5 minutes and colonies for 10; PodStack's intervals follow the legacy app's.
    domain!("industry_jobs", "Industry Jobs", 900, refresh_jobs),
    domain!("planets", "Planetary Colonies", 1800, refresh_planets),
    // ESI caches blueprints for an hour.
    domain!("blueprints", "Blueprints", 3600, refresh_blueprints),
];

pub const BLUEPRINTS_SCOPE: &str = "esi-characters.read_blueprints.v1";

pub const JOBS_SCOPE: &str = "esi-industry.read_character_jobs.v1";
pub const PLANETS_SCOPE: &str = "esi-planets.manage_planets.v1";

/// One entry of `characters/{id}/industry/jobs`, plus resolved names.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Job {
    pub job_id: i64,
    pub activity_id: i64,
    pub blueprint_id: i64,
    pub blueprint_type_id: i64,
    #[serde(default)]
    pub blueprint_location_id: i64,
    #[serde(default)]
    pub product_type_id: Option<i64>,
    #[serde(default)]
    pub facility_id: i64,
    /// The station or structure the job runs in.
    pub station_id: i64,
    #[serde(default)]
    pub output_location_id: i64,
    #[serde(default)]
    pub installer_id: i64,
    pub runs: i64,
    #[serde(default)]
    pub licensed_runs: Option<i64>,
    #[serde(default)]
    pub successful_runs: Option<i64>,
    #[serde(default)]
    pub probability: Option<f64>,
    #[serde(default)]
    pub cost: Option<f64>,
    pub duration: i64,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    #[serde(default)]
    pub pause_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub completed_date: Option<DateTime<Utc>>,
    /// "active", "cancelled", "delivered", "paused", "ready" or "reverted"
    pub status: String,

    #[serde(default)]
    pub blueprint_name: Option<String>,
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub location: Option<Place>,
}

impl Job {
    pub fn blueprint_label(&self) -> String {
        self.blueprint_name.clone().unwrap_or_else(|| format!("Type #{}", self.blueprint_type_id))
    }

    pub fn location_name(&self) -> String {
        self.location.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| format!("Location #{}", self.station_id))
    }

    /// The product, when it isn't the blueprint itself (research and copying keep the blueprint).
    pub fn distinct_product(&self) -> Option<&str> {
        match self.product_type_id {
            Some(p) if p != self.blueprint_type_id => self.product_name.as_deref(),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IndustryData {
    pub jobs: Vec<Job>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Extractor {
    pub pin_id: i64,
    #[serde(default)]
    pub product_type_id: Option<i64>,
    #[serde(default)]
    pub product_name: Option<String>,
    #[serde(default)]
    pub qty_per_cycle: Option<i64>,
    /// Seconds per cycle.
    #[serde(default)]
    pub cycle_time: Option<i64>,
    #[serde(default)]
    pub heads: usize,
    #[serde(default)]
    pub install_time: Option<DateTime<Utc>>,
    /// When the extraction program ends; `None` when the extractor has no program.
    #[serde(default)]
    pub expiry_time: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Factory {
    pub pin_id: i64,
    #[serde(default)]
    pub schematic_id: Option<i64>,
    /// What the schematic produces, e.g. "Coolant".
    #[serde(default)]
    pub schematic_name: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Stored {
    pub type_id: i64,
    #[serde(default)]
    pub name: Option<String>,
    pub amount: i64,
}

impl Stored {
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| format!("Type #{}", self.type_id))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Colony {
    pub planet_id: i64,
    #[serde(default)]
    pub planet_name: String,
    /// "temperate", "barren", "oceanic", "ice", "gas", "lava", "storm" or "plasma"
    #[serde(default)]
    pub planet_type: String,
    pub solar_system_id: i64,
    #[serde(default)]
    pub system_name: Option<String>,
    /// Command center upgrade level (0-5).
    #[serde(default)]
    pub upgrade_level: i64,
    #[serde(default)]
    pub num_pins: i64,
    pub last_update: DateTime<Utc>,
    #[serde(default)]
    pub extractors: Vec<Extractor>,
    #[serde(default)]
    pub factories: Vec<Factory>,
    /// Everything stored on the planet (storage, launchpads, factory buffers), summed by type, most first.
    #[serde(default)]
    pub storage: Vec<Stored>,
    /// Every structure of the colony with what it holds (for storage fill and factory buffers).
    #[serde(default)]
    pub pins: Vec<ColonyPin>,
    /// How products move between structures.
    #[serde(default)]
    pub routes: Vec<ColonyRoute>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ColonyPin {
    pub pin_id: i64,
    /// The structure type (e.g. a Barren Launchpad).
    pub type_id: i64,
    #[serde(default)]
    pub schematic_id: Option<i64>,
    /// (type, units) held.
    #[serde(default)]
    pub contents: Vec<(i64, i64)>,
    #[serde(default)]
    pub last_cycle_start: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ColonyRoute {
    pub source_pin_id: i64,
    pub destination_pin_id: i64,
    pub content_type_id: i64,
    pub quantity: f64,
}

impl Colony {
    pub fn name(&self) -> String {
        if self.planet_name.is_empty() {
            format!("Planet #{}", self.planet_id)
        } else {
            self.planet_name.clone()
        }
    }

    /// Factories counted per product, most first: [("Coolant", 4), ...].
    pub fn factory_products(&self) -> Vec<(String, usize)> {
        let mut by: BTreeMap<String, usize> = BTreeMap::new();
        for f in &self.factories {
            let name = f.schematic_name.clone().unwrap_or_else(|| "Unknown schematic".into());
            *by.entry(name).or_default() += 1;
        }
        let mut out: Vec<_> = by.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlanetsData {
    pub colonies: Vec<Colony>,
}

/// One of `characters/{id}/blueprints`, plus resolved names.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct OwnedBlueprint {
    pub item_id: i64,
    pub type_id: i64,
    pub location_id: i64,
    #[serde(default)]
    pub location_flag: String,
    pub material_efficiency: i64,
    pub time_efficiency: i64,
    /// -1 an original, -2 a copy, otherwise a stack of that many originals.
    pub quantity: i64,
    /// Runs left on a copy; -1 for originals.
    pub runs: i64,
    #[serde(default)]
    pub type_name: Option<String>,
    /// The station or structure it's in (through any containers), when known.
    #[serde(default)]
    pub location: Option<Place>,
}

impl OwnedBlueprint {
    pub fn is_copy(&self) -> bool {
        self.quantity == -2
    }
    /// How many blueprints this entry stands for (a stack of originals counts each).
    pub fn count(&self) -> i64 {
        self.quantity.max(1)
    }
    pub fn name(&self) -> String {
        self.type_name.clone().unwrap_or_else(|| crate::blueprints::INDUSTRY.type_name(self.type_id))
    }
    pub fn location_name(&self) -> String {
        self.location.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Unknown location".into())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BlueprintsData {
    pub blueprints: Vec<OwnedBlueprint>,
}

// ---------- fetching ----------

async fn refresh_blueprints(cx: &Cx) -> DomainResult {
    let mut list: Vec<OwnedBlueprint> = cx.get(cx.char_path("blueprints")).scope(BLUEPRINTS_SCOPE).all_pages().await?;
    // Blueprints inside containers name the container as their location; the character's assets know which station
    // or structure the outermost container is in.
    let roots_of: HashMap<i64, i64> =
        cx.prev.assets.as_ref().map(|a| a.items.iter().map(|i| (i.item_id, i.root_location_id)).collect()).unwrap_or_default();
    let root = |id: i64| roots_of.get(&id).copied().unwrap_or(id);
    let roots: Vec<i64> = list.iter().map(|b| root(b.location_id)).collect();
    let mut place_ids: Vec<i64> = roots.clone();
    place_ids.sort();
    place_ids.dedup();
    let places = cx.universe().places(cx.esi(), place_ids, Some(&cx.auth)).await;
    let names = cx.universe().names(cx.esi(), list.iter().map(|b| b.type_id).collect::<Vec<_>>()).await;
    for (b, r) in list.iter_mut().zip(roots) {
        b.type_name = names.get(&b.type_id).cloned();
        b.location = places.get(&r).cloned();
    }
    let data = BlueprintsData { blueprints: list };
    Ok(Box::new(move |c| c.blueprints = Some(data)))
}

async fn refresh_jobs(cx: &Cx) -> DomainResult {
    let mut jobs: Vec<Job> = cx.get(cx.char_path("industry/jobs")).scope(JOBS_SCOPE).json().await?;
    let type_ids: Vec<i64> = jobs.iter().flat_map(|j| [Some(j.blueprint_type_id), j.product_type_id]).flatten().collect();
    let names = cx.universe().names(cx.esi(), type_ids).await;
    let places = cx.universe().places(cx.esi(), jobs.iter().map(|j| j.station_id), Some(&cx.auth)).await;
    for j in &mut jobs {
        j.blueprint_name = names.get(&j.blueprint_type_id).cloned();
        j.product_name = j.product_type_id.and_then(|p| names.get(&p).cloned());
        j.location = places.get(&j.station_id).cloned();
    }
    let data = IndustryData { jobs };
    Ok(Box::new(move |c| c.industry = Some(data)))
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawColony {
    pub planet_id: i64,
    #[serde(default)]
    pub planet_type: String,
    pub solar_system_id: i64,
    #[serde(default)]
    pub upgrade_level: i64,
    #[serde(default)]
    pub num_pins: i64,
    pub last_update: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct RawLayout {
    #[serde(default)]
    pub pins: Vec<RawPin>,
    #[serde(default)]
    pub routes: Vec<ColonyRoute>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawPin {
    pub pin_id: i64,
    #[serde(default)]
    pub type_id: i64,
    #[serde(default)]
    pub contents: Vec<RawContent>,
    #[serde(default)]
    pub install_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub expiry_time: Option<DateTime<Utc>>,
    #[serde(default)]
    pub schematic_id: Option<i64>,
    #[serde(default)]
    pub extractor_details: Option<RawExtractor>,
    #[serde(default)]
    pub factory_details: Option<RawFactory>,
    #[serde(default)]
    pub last_cycle_start: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawContent {
    pub type_id: i64,
    pub amount: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawExtractor {
    #[serde(default)]
    pub product_type_id: Option<i64>,
    #[serde(default)]
    pub qty_per_cycle: Option<i64>,
    #[serde(default)]
    pub cycle_time: Option<i64>,
    #[serde(default)]
    pub heads: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RawFactory {
    pub schematic_id: i64,
}

#[derive(Deserialize)]
struct RawPlanet {
    name: String,
}

#[derive(Deserialize)]
struct RawSchematic {
    schematic_name: String,
}

/// A colony from its list entry and layout; names are filled in afterwards.
pub fn build_colony(raw: &RawColony, layout: &RawLayout) -> Colony {
    let extractors = layout
        .pins
        .iter()
        .filter_map(|p| {
            let e = p.extractor_details.as_ref()?;
            Some(Extractor {
                pin_id: p.pin_id,
                product_type_id: e.product_type_id,
                product_name: None,
                qty_per_cycle: e.qty_per_cycle,
                cycle_time: e.cycle_time,
                heads: e.heads.len(),
                install_time: p.install_time,
                expiry_time: p.expiry_time,
            })
        })
        .collect();
    let factories = layout
        .pins
        .iter()
        .filter(|p| p.factory_details.is_some())
        .map(|p| Factory {
            pin_id: p.pin_id,
            schematic_id: p.factory_details.as_ref().map(|f| f.schematic_id).or(p.schematic_id),
            schematic_name: None,
        })
        .collect();
    let mut stored: HashMap<i64, i64> = HashMap::new();
    for c in layout.pins.iter().flat_map(|p| &p.contents) {
        *stored.entry(c.type_id).or_default() += c.amount;
    }
    let mut storage: Vec<Stored> = stored.into_iter().map(|(type_id, amount)| Stored { type_id, name: None, amount }).collect();
    storage.sort_by(|a, b| b.amount.cmp(&a.amount).then(a.type_id.cmp(&b.type_id)));
    Colony {
        planet_id: raw.planet_id,
        planet_name: String::new(),
        planet_type: raw.planet_type.clone(),
        solar_system_id: raw.solar_system_id,
        system_name: None,
        upgrade_level: raw.upgrade_level,
        num_pins: raw.num_pins,
        last_update: raw.last_update,
        extractors,
        factories,
        storage,
        pins: layout
            .pins
            .iter()
            .map(|p| ColonyPin {
                pin_id: p.pin_id,
                type_id: p.type_id,
                schematic_id: p.factory_details.as_ref().map(|f| f.schematic_id).or(p.schematic_id),
                contents: p.contents.iter().map(|c| (c.type_id, c.amount)).collect(),
                last_cycle_start: p.last_cycle_start,
            })
            .collect(),
        routes: layout.routes.clone(),
    }
}

async fn refresh_planets(cx: &Cx) -> DomainResult {
    let raw: Vec<RawColony> = cx.get(cx.char_path("planets")).scope(PLANETS_SCOPE).json().await?;

    // Planet and schematic names never change: reuse what the last refresh found.
    let prev = cx.prev.planets.as_ref();
    let mut planet_names: HashMap<i64, String> = HashMap::new();
    let mut schematic_names: HashMap<i64, String> = HashMap::new();
    for c in prev.map(|p| p.colonies.as_slice()).unwrap_or_default() {
        if !c.planet_name.is_empty() {
            planet_names.insert(c.planet_id, c.planet_name.clone());
        }
        for f in &c.factories {
            if let (Some(id), Some(n)) = (f.schematic_id, &f.schematic_name) {
                schematic_names.insert(id, n.clone());
            }
        }
    }

    let mut colonies = Vec::new();
    for rc in &raw {
        let layout: RawLayout = cx.get(cx.char_path(&format!("planets/{}", rc.planet_id))).scope(PLANETS_SCOPE).json().await?;
        let mut colony = build_colony(rc, &layout);
        colony.planet_name = match planet_names.get(&rc.planet_id) {
            Some(n) => n.clone(),
            None => cx.esi().get(format!("universe/planets/{}", rc.planet_id)).json::<RawPlanet>().await.map(|p| p.name).unwrap_or_default(),
        };
        for f in &mut colony.factories {
            let Some(id) = f.schematic_id else { continue };
            if let std::collections::hash_map::Entry::Vacant(slot) = schematic_names.entry(id) {
                if let Ok(s) = cx.esi().get(format!("universe/schematics/{id}")).json::<RawSchematic>().await {
                    slot.insert(s.schematic_name);
                }
            }
            f.schematic_name = schematic_names.get(&id).cloned();
        }
        colonies.push(colony);
    }

    let ids: Vec<i64> = colonies
        .iter()
        .flat_map(|c| {
            std::iter::once(c.solar_system_id)
                .chain(c.extractors.iter().filter_map(|e| e.product_type_id))
                .chain(c.storage.iter().map(|s| s.type_id))
        })
        .collect();
    let names = cx.universe().names(cx.esi(), ids).await;
    for c in &mut colonies {
        c.system_name = names.get(&c.solar_system_id).cloned();
        for e in &mut c.extractors {
            e.product_name = e.product_type_id.and_then(|t| names.get(&t).cloned());
        }
        for s in &mut c.storage {
            s.name = names.get(&s.type_id).cloned();
        }
    }
    let data = PlanetsData { colonies };
    Ok(Box::new(move |c| c.planets = Some(data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jobs() {
        let json = r#"[{
            "activity_id": 1, "blueprint_id": 1015116533326, "blueprint_location_id": 60006382,
            "blueprint_type_id": 2047, "cost": 118.0, "duration": 548, "end_date": "2018-06-08T13:21:41Z",
            "facility_id": 60006382, "installer_id": 2112625428, "job_id": 229136101,
            "licensed_runs": 200, "output_location_id": 60006382, "runs": 1, "start_date": "2018-06-08T13:12:33Z",
            "station_id": 60006382, "status": "active", "product_type_id": 2048
        }, {
            "activity_id": 5, "blueprint_id": 2, "blueprint_type_id": 2047, "duration": 10,
            "end_date": "2018-06-08T13:21:41Z", "facility_id": 1, "job_id": 2, "runs": 10,
            "start_date": "2018-06-08T13:12:33Z", "station_id": 1035466617946, "status": "paused",
            "product_type_id": 2047
        }]"#;
        let jobs: Vec<Job> = serde_json::from_str(json).unwrap();
        assert_eq!(jobs.len(), 2);
        let mut j = jobs[0].clone();
        assert_eq!(j.blueprint_label(), "Type #2047");
        j.blueprint_name = Some("Damage Control I Blueprint".into());
        j.product_name = Some("Damage Control I".into());
        assert_eq!(j.distinct_product(), Some("Damage Control I"));
        assert_eq!(j.location_name(), "Location #60006382");
        // copying produces the blueprint itself
        assert_eq!(jobs[1].distinct_product(), None);
        assert_eq!(jobs[1].status, "paused");
    }

    #[test]
    fn builds_colony_from_layout() {
        let raw: RawColony = serde_json::from_str(
            r#"{"last_update": "2016-11-28T16:42:51Z", "num_pins": 9, "owner_id": 95465499, "planet_id": 40023691,
                "planet_type": "plasma", "solar_system_id": 30000379, "upgrade_level": 4}"#,
        )
        .unwrap();
        let layout: RawLayout = serde_json::from_str(
            r#"{"links": [], "routes": [], "pins": [
                {"pin_id": 1, "type_id": 2848, "latitude": 1.0, "longitude": 2.0,
                 "install_time": "2016-11-28T16:00:00Z", "expiry_time": "2016-11-30T16:00:00Z",
                 "extractor_details": {"cycle_time": 1800, "head_radius": 0.01, "product_type_id": 2268,
                                       "qty_per_cycle": 1200, "heads": [{"head_id": 0, "latitude": 1, "longitude": 1},
                                                                         {"head_id": 1, "latitude": 1, "longitude": 1}]}},
                {"pin_id": 2, "type_id": 2471, "latitude": 1.0, "longitude": 2.0, "schematic_id": 121,
                 "factory_details": {"schematic_id": 121}, "contents": [{"amount": 3000, "type_id": 2268}]},
                {"pin_id": 3, "type_id": 2541, "latitude": 1.0, "longitude": 2.0,
                 "contents": [{"amount": 10, "type_id": 2390}, {"amount": 500, "type_id": 2268}]},
                {"pin_id": 4, "type_id": 2848, "latitude": 1.0, "longitude": 2.0,
                 "extractor_details": {"heads": []}}
            ]}"#,
        )
        .unwrap();
        let mut c = build_colony(&raw, &layout);
        assert_eq!(c.name(), "Planet #40023691");
        assert_eq!(c.extractors.len(), 2);
        assert_eq!(c.extractors[0].heads, 2);
        assert_eq!(c.extractors[0].qty_per_cycle, Some(1200));
        assert_eq!(c.extractors[1].expiry_time, None);
        assert_eq!(c.factories.len(), 1);
        assert_eq!(c.factories[0].schematic_id, Some(121));
        assert_eq!(c.storage, vec![Stored { type_id: 2268, name: None, amount: 3500 }, Stored { type_id: 2390, name: None, amount: 10 }]);
        c.factories[0].schematic_name = Some("Reactive Metals".into());
        assert_eq!(c.factory_products(), vec![("Reactive Metals".to_string(), 1)]);
    }
}
