//! Assets: every item with its type name, the ship or container it's in, and the station, structure or system its
//! outermost container sits in. Ships and containers get their in-game names (`assets/names`), and each item gets
//! EVE's universe-wide average price so pages can show estimated values without waiting on the market.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use crate::universe::Place;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

pub const DOMAINS: &[DomainSpec] = &[domain!("assets", "Assets", 3600, refresh_assets)];

pub const SCOPE: &str = "esi-assets.read_assets.v1";

/// How many container levels [`root_location`] climbs before giving up (guards against a loop in bad data).
const MAX_DEPTH: usize = 10;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Asset {
    pub item_id: i64,
    pub type_id: i64,
    /// Type name ("Type #id" when it couldn't be resolved).
    pub name: String,
    /// The name the player gave a ship or container.
    #[serde(default)]
    pub custom_name: Option<String>,
    pub quantity: i64,
    /// ESI's location_flag, e.g. "Hangar", "Cargo", "HiSlot0".
    #[serde(default)]
    pub flag: String,
    /// The item (ship, container) this one is in, if it's in another of the character's items.
    #[serde(default)]
    pub parent_id: Option<i64>,
    /// The station, structure or system of the outermost container.
    pub root_location_id: i64,
    #[serde(default)]
    pub is_blueprint_copy: bool,
    #[serde(default)]
    pub is_singleton: bool,
    /// EVE's average price for one unit when the assets were loaded.
    #[serde(default)]
    pub unit_price: Option<f64>,
}

impl Asset {
    /// "Custom name (Type)" for named ships and containers, else the type name.
    pub fn label(&self) -> String {
        match &self.custom_name {
            Some(n) => format!("{n} ({})", self.name),
            None => self.name.clone(),
        }
    }

    /// Estimated value of the stack; blueprint copies can't be sold on the market, so they count as 0.
    pub fn value(&self) -> f64 {
        if self.is_blueprint_copy {
            0.0
        } else {
            self.unit_price.unwrap_or(0.0) * self.count() as f64
        }
    }

    /// How many items the stack holds. ESI reports blueprints as -1 (original) or -2 (copy), which is one item.
    pub fn count(&self) -> i64 {
        self.quantity.max(1)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AssetsData {
    pub items: Vec<Asset>,
    /// Root locations by id.
    #[serde(default)]
    pub locations: BTreeMap<i64, Place>,
    /// When the prices in [`Asset::unit_price`] were fetched.
    #[serde(default)]
    pub priced: Option<DateTime<Utc>>,
}

impl AssetsData {
    pub fn location_name(&self, id: i64) -> String {
        self.locations.get(&id).map(|p| p.name.clone()).unwrap_or_else(|| format!("Location #{id}"))
    }

    pub fn total_value(&self) -> f64 {
        self.items.iter().map(Asset::value).sum()
    }

    pub fn by_id(&self) -> HashMap<i64, &Asset> {
        self.items.iter().map(|a| (a.item_id, a)).collect()
    }
}

/// One entry of `characters/{id}/assets`.
#[derive(Clone, Debug, Deserialize)]
pub struct RawAsset {
    pub item_id: i64,
    pub type_id: i64,
    #[serde(default = "one")]
    pub quantity: i64,
    pub location_id: i64,
    #[serde(default)]
    pub location_flag: String,
    #[serde(default)]
    pub is_singleton: bool,
    #[serde(default)]
    pub is_blueprint_copy: Option<bool>,
}

fn one() -> i64 {
    1
}

#[derive(Deserialize)]
struct RawName {
    item_id: i64,
    #[serde(default)]
    name: String,
}

/// Follows `location_id` through the character's own items up to the outermost one's location.
pub fn root_location(by_id: &HashMap<i64, &RawAsset>, asset: &RawAsset) -> i64 {
    let mut current = asset;
    for _ in 0..MAX_DEPTH {
        match by_id.get(&current.location_id) {
            Some(parent) => current = parent,
            None => break,
        }
    }
    current.location_id
}

/// Items worth asking a name for: assembled items other items sit in (ships and containers).
pub fn nameable(raw: &[RawAsset]) -> Vec<i64> {
    let holders: HashSet<i64> = raw.iter().map(|a| a.location_id).collect();
    raw.iter().filter(|a| a.is_singleton && holders.contains(&a.item_id)).map(|a| a.item_id).collect()
}

/// Turns ESI's list into [`Asset`]s, with names and prices from the lookups.
pub fn build_assets(
    raw: &[RawAsset],
    type_names: &HashMap<i64, String>,
    custom_names: &HashMap<i64, String>,
    prices: &HashMap<i64, f64>,
) -> Vec<Asset> {
    let by_id: HashMap<i64, &RawAsset> = raw.iter().map(|a| (a.item_id, a)).collect();
    raw.iter()
        .map(|a| Asset {
            item_id: a.item_id,
            type_id: a.type_id,
            name: type_names.get(&a.type_id).cloned().unwrap_or_else(|| format!("Type #{}", a.type_id)),
            custom_name: custom_names.get(&a.item_id).cloned(),
            quantity: a.quantity,
            flag: a.location_flag.clone(),
            parent_id: by_id.contains_key(&a.location_id).then_some(a.location_id),
            root_location_id: root_location(&by_id, a),
            is_blueprint_copy: a.is_blueprint_copy == Some(true),
            is_singleton: a.is_singleton,
            unit_price: prices.get(&a.type_id).copied(),
        })
        .collect()
}

async fn refresh_assets(cx: &Cx) -> DomainResult {
    let raw: Vec<RawAsset> = cx.get(cx.char_path("assets")).scope(SCOPE).all_pages().await?;

    let by_id: HashMap<i64, &RawAsset> = raw.iter().map(|a| (a.item_id, a)).collect();
    let roots: HashSet<i64> = raw.iter().map(|a| root_location(&by_id, a)).collect();
    let type_names = cx.universe().names(cx.esi(), raw.iter().map(|a| a.type_id)).await;
    let places = cx.universe().places(cx.esi(), roots, Some(&cx.auth)).await;

    // A failed name lookup only costs the custom names, so it doesn't fail the refresh.
    let mut custom_names: HashMap<i64, String> = HashMap::new();
    for chunk in nameable(&raw).chunks(1000) {
        match cx.post(cx.char_path("assets/names")).scope(SCOPE).body(chunk).json::<Vec<RawName>>().await {
            Ok(list) => custom_names.extend(list.into_iter().filter(|n| !n.name.is_empty() && n.name != "None").map(|n| (n.item_id, n.name))),
            Err(err) => tracing::debug!("asset names for {}: {err}", cx.id),
        }
    }

    let prices = cx.core.market.average_prices(cx.esi()).await;
    let data = AssetsData {
        items: build_assets(&raw, &type_names, &custom_names, &prices),
        locations: places.into_iter().collect(),
        priced: (!prices.is_empty()).then(Utc::now),
    };
    Ok(Box::new(move |c| c.assets = Some(data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"[
        {"is_singleton": true, "item_id": 1001, "location_flag": "Hangar", "location_id": 60003760, "location_type": "station", "quantity": 1, "type_id": 587},
        {"is_singleton": false, "item_id": 1002, "location_flag": "Cargo", "location_id": 1001, "location_type": "item", "quantity": 500, "type_id": 34},
        {"is_singleton": true, "item_id": 1003, "location_flag": "Cargo", "location_id": 1001, "location_type": "item", "quantity": 1, "type_id": 17366},
        {"is_singleton": false, "item_id": 1004, "location_flag": "Unlocked", "location_id": 1003, "location_type": "item", "quantity": 3, "type_id": 2048},
        {"is_blueprint_copy": true, "is_singleton": true, "item_id": 1005, "location_flag": "Hangar", "location_id": 1035466617946, "location_type": "other", "quantity": -2, "type_id": 691},
        {"is_singleton": false, "item_id": 1006, "location_flag": "Hangar", "location_id": 30000142, "location_type": "solar_system", "quantity": 2, "type_id": 34}
    ]"#;

    #[test]
    fn parses_and_resolves_roots() {
        let raw: Vec<RawAsset> = serde_json::from_str(SAMPLE).unwrap();
        let names: HashMap<i64, String> = [(587, "Rifter"), (34, "Tritanium"), (17366, "Station Container")]
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect();
        let custom: HashMap<i64, String> = [(1001, "Little Red".to_string())].into_iter().collect();
        let prices: HashMap<i64, f64> = [(34, 4.0), (587, 500_000.0), (691, 9e6)].into_iter().collect();
        let assets = build_assets(&raw, &names, &custom, &prices);

        assert_eq!(assets.len(), 6);
        let ship = &assets[0];
        assert_eq!(ship.label(), "Little Red (Rifter)");
        assert_eq!(ship.parent_id, None);
        assert_eq!(ship.root_location_id, 60003760);
        // nested two levels deep: ship → container → item
        let nested = &assets[3];
        assert_eq!(nested.parent_id, Some(1003));
        assert_eq!(nested.root_location_id, 60003760);
        assert_eq!(nested.name, "Type #2048");
        assert_eq!(assets[1].value(), 2000.0);
        // blueprint copies are worth nothing on the market
        assert!(assets[4].is_blueprint_copy);
        assert_eq!(assets[4].value(), 0.0);
        assert_eq!(assets[4].root_location_id, 1035466617946);
        assert_eq!(assets[4].count(), 1);
        assert_eq!(assets[5].root_location_id, 30000142);
    }

    #[test]
    fn names_only_holders() {
        let raw: Vec<RawAsset> = serde_json::from_str(SAMPLE).unwrap();
        let mut n = nameable(&raw);
        n.sort();
        assert_eq!(n, vec![1001, 1003]);
    }

    #[test]
    fn loops_terminate() {
        let raw: Vec<RawAsset> = serde_json::from_str(
            r#"[{"item_id": 1, "type_id": 1, "location_id": 2, "quantity": 1},
                {"item_id": 2, "type_id": 1, "location_id": 1, "quantity": 1}]"#,
        )
        .unwrap();
        let assets = build_assets(&raw, &HashMap::new(), &HashMap::new(), &HashMap::new());
        assert_eq!(assets.len(), 2);
    }

    #[test]
    fn data_roundtrips() {
        let mut d = AssetsData::default();
        d.locations.insert(60003760, Place { id: 60003760, name: "Jita IV - Moon 4".into(), ..Default::default() });
        let json = serde_json::to_string(&d).unwrap();
        let back: AssetsData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.location_name(60003760), "Jita IV - Moon 4");
        assert_eq!(back.location_name(5), "Location #5");
    }
}
