//! Active implants and jump clones.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use crate::sde::{self, Attribute};
use crate::universe::Place;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DOMAINS: &[DomainSpec] = &[
    domain!("implants", "Active Implants", 3600, refresh_implants),
    domain!("clones", "Jump Clones", 3600, refresh_clones),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Implant {
    pub type_id: i64,
    pub name: String,
    #[serde(default)]
    pub slot: Option<u8>,
    /// Attribute bonuses (+1 … +5) the implant gives.
    #[serde(default)]
    pub bonuses: Vec<(Attribute, i32)>,
}

impl Implant {
    pub fn bonus(&self, a: Attribute) -> i32 {
        self.bonuses.iter().filter(|(x, _)| *x == a).map(|(_, v)| *v).sum()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JumpClone {
    pub jump_clone_id: i64,
    #[serde(default)]
    pub name: Option<String>,
    pub location_id: i64,
    pub location_type: String,
    pub place: Place,
    pub implants: Vec<Implant>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Clones {
    #[serde(default)]
    pub home: Option<Place>,
    #[serde(default)]
    pub last_clone_jump_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_station_change_date: Option<DateTime<Utc>>,
    pub jump_clones: Vec<JumpClone>,
}

async fn implants_for(cx: &Cx, ids: &[i64]) -> Vec<Implant> {
    let types = cx.universe().types(cx.esi(), ids.iter().copied()).await;
    let mut out: Vec<Implant> = ids
        .iter()
        .map(|id| {
            let t = types.get(id);
            Implant {
                type_id: *id,
                name: t.map(|t| t.name.clone()).unwrap_or_else(|| format!("Implant #{id}")),
                slot: t.and_then(|t| t.attr(sde::DOGMA_IMPLANT_SLOT)).map(|v| v as u8),
                bonuses: sde::DOGMA_ATTRIBUTE_BONUS
                    .iter()
                    .filter_map(|(attr_id, a)| t.and_then(|t| t.attr(*attr_id)).filter(|v| *v > 0.0).map(|v| (*a, v as i32)))
                    .collect(),
            }
        })
        .collect();
    out.sort_by_key(|i| i.slot.unwrap_or(99));
    out
}

async fn refresh_implants(cx: &Cx) -> DomainResult {
    let ids: Vec<i64> = cx.get(cx.char_path("implants")).scope("esi-clones.read_implants.v1").json().await?;
    let implants = implants_for(cx, &ids).await;
    Ok(Box::new(move |c| c.implants = Some(implants)))
}

#[derive(Deserialize)]
struct RawLocation {
    location_id: i64,
    #[allow(dead_code)]
    location_type: String,
}

#[derive(Deserialize)]
struct RawClone {
    jump_clone_id: i64,
    #[serde(default)]
    name: Option<String>,
    location_id: i64,
    location_type: String,
    #[serde(default)]
    implants: Vec<i64>,
}

#[derive(Deserialize)]
struct RawClones {
    #[serde(default)]
    home_location: Option<RawLocation>,
    #[serde(default)]
    last_clone_jump_date: Option<DateTime<Utc>>,
    #[serde(default)]
    last_station_change_date: Option<DateTime<Utc>>,
    #[serde(default)]
    jump_clones: Vec<RawClone>,
}

async fn refresh_clones(cx: &Cx) -> DomainResult {
    let raw: RawClones = cx.get(cx.char_path("clones")).scope("esi-clones.read_clones.v1").json().await?;
    let mut ids: Vec<i64> = raw.jump_clones.iter().map(|c| c.location_id).collect();
    if let Some(h) = &raw.home_location {
        ids.push(h.location_id);
    }
    let places: HashMap<i64, Place> = cx.universe().places(cx.esi(), ids, Some(&cx.auth)).await;
    let mut jump_clones = Vec::new();
    for c in raw.jump_clones {
        jump_clones.push(JumpClone {
            jump_clone_id: c.jump_clone_id,
            name: c.name,
            location_id: c.location_id,
            location_type: c.location_type,
            place: places.get(&c.location_id).cloned().unwrap_or_default(),
            implants: implants_for(cx, &c.implants).await,
        });
    }
    let clones = Clones {
        home: raw.home_location.and_then(|h| places.get(&h.location_id).cloned()),
        last_clone_jump_date: raw.last_clone_jump_date,
        last_station_change_date: raw.last_station_change_date,
        jump_clones,
    };
    Ok(Box::new(move |c| c.clones = Some(clones)))
}
