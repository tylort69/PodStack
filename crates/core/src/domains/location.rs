//! Current location, active ship and jump fatigue.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use crate::universe::Place;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DOMAINS: &[DomainSpec] = &[
    domain!("location", "Current Location", 3600, refresh_location),
    domain!("ship", "Active Ship", 3600, refresh_ship),
    domain!("fatigue", "Jump Fatigue", 3600, refresh_fatigue),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Location {
    pub system: Place,
    /// Station or structure the character is docked in.
    #[serde(default)]
    pub docked: Option<Place>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ship {
    pub ship_item_id: i64,
    pub ship_name: String,
    pub ship_type_id: i64,
    #[serde(default)]
    pub type_name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Fatigue {
    #[serde(default)]
    pub jump_fatigue_expire_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_jump_date: Option<DateTime<Utc>>,
    /// Jump activation timer ("red timer") expiry.
    #[serde(default)]
    pub last_update_date: Option<DateTime<Utc>>,
}

#[derive(Deserialize)]
struct RawLocation {
    solar_system_id: i64,
    #[serde(default)]
    station_id: Option<i64>,
    #[serde(default)]
    structure_id: Option<i64>,
}

async fn refresh_location(cx: &Cx) -> DomainResult {
    let raw: RawLocation = cx.get(cx.char_path("location")).scope("esi-location.read_location.v1").json().await?;
    let system = cx.universe().place(cx.esi(), raw.solar_system_id, None).await;
    let docked = match raw.station_id.or(raw.structure_id) {
        Some(id) => Some(cx.universe().place(cx.esi(), id, Some(&cx.auth)).await),
        None => None,
    };
    let loc = Location { system, docked };
    Ok(Box::new(move |c| c.location = Some(loc)))
}

async fn refresh_ship(cx: &Cx) -> DomainResult {
    let mut ship: Ship = cx.get(cx.char_path("ship")).scope("esi-location.read_ship_type.v1").json().await?;
    ship.type_name = cx.universe().name(cx.esi(), ship.ship_type_id).await.unwrap_or_default();
    Ok(Box::new(move |c| c.ship = Some(ship)))
}

async fn refresh_fatigue(cx: &Cx) -> DomainResult {
    let f: Fatigue = cx.get(cx.char_path("fatigue")).scope("esi-characters.read_fatigue.v1").json().await?;
    Ok(Box::new(move |c| c.fatigue = Some(f)))
}
