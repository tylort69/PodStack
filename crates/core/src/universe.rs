//! Cached lookups of EVE's static-ish universe data: names, types, solar systems, stations and structures.
//! Cached in memory and on disk (`$XDG_CACHE_HOME/podstack/universe.json`); [`Universe::flush`] writes changes.

use crate::esi::{Auth, Esi, EsiError};
use crate::store;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

const TYPE_TTL_DAYS: i64 = 30;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct DogmaAttribute {
    pub attribute_id: i64,
    pub value: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct TypeInfo {
    pub type_id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub group_id: i64,
    #[serde(default)]
    pub group_name: String,
    #[serde(default)]
    pub category_id: i64,
    #[serde(default)]
    pub market_group_id: Option<i64>,
    #[serde(default)]
    pub volume: Option<f64>,
    #[serde(default)]
    pub packaged_volume: Option<f64>,
    #[serde(default)]
    pub dogma_attributes: Vec<DogmaAttribute>,
    #[serde(default)]
    pub fetched: Option<DateTime<Utc>>,
}

impl TypeInfo {
    pub fn attr(&self, id: i64) -> Option<f64> {
        self.dogma_attributes.iter().find(|a| a.attribute_id == id).map(|a| a.value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SystemInfo {
    pub system_id: i64,
    pub name: String,
    pub security_status: f64,
    #[serde(default)]
    pub constellation_id: i64,
    #[serde(default)]
    pub region_id: i64,
    #[serde(default)]
    pub region_name: String,
}

impl SystemInfo {
    /// Security rounded the way the game shows it (0.05 → 0.1, but 0.0-0.05 stays 0.0 only when ≤ 0).
    pub fn display_security(&self) -> f64 {
        let s = self.security_status;
        if s > 0.0 && s < 0.05 {
            0.1
        } else {
            (s * 10.0).round() / 10.0
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct StationInfo {
    pub station_id: i64,
    pub name: String,
    pub system_id: i64,
    #[serde(default)]
    pub type_id: i64,
    #[serde(default)]
    pub owner: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct StructureInfo {
    pub structure_id: i64,
    pub name: String,
    pub system_id: i64,
    #[serde(default)]
    pub type_id: Option<i64>,
    #[serde(default)]
    pub owner_id: i64,
}

/// A station, structure or system, resolved for display.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
pub struct Place {
    pub id: i64,
    pub name: String,
    pub system_id: Option<i64>,
    pub system_name: Option<String>,
    pub security: Option<f64>,
    pub region_name: Option<String>,
    /// "station", "structure", "system" or "unknown"
    pub kind: String,
}

#[derive(Default, Serialize, Deserialize)]
struct CacheFile {
    names: HashMap<i64, String>,
    types: HashMap<i64, TypeInfo>,
    systems: HashMap<i64, SystemInfo>,
    stations: HashMap<i64, StationInfo>,
    structures: HashMap<i64, StructureInfo>,
    /// structure id -> characters that were refused access
    structure_denied: HashMap<i64, HashSet<i64>>,
    region_names: HashMap<i64, String>,
}

pub struct Universe {
    cache: Mutex<CacheFile>,
    dirty: AtomicBool,
}

pub fn is_station(id: i64) -> bool {
    (60_000_000..64_000_000).contains(&id)
}
pub fn is_system(id: i64) -> bool {
    (30_000_000..33_000_000).contains(&id)
}
/// Ids universe/names accepts: types, stations, systems, regions, characters, corporations, alliances, factions.
pub fn is_nameable(id: i64) -> bool {
    id > 0 && id < 2_200_000_000
}

#[derive(Deserialize)]
struct NameEntry {
    id: i64,
    name: String,
}

#[derive(Deserialize)]
struct RawType {
    type_id: i64,
    name: String,
    #[serde(default)]
    description: String,
    group_id: i64,
    #[serde(default)]
    market_group_id: Option<i64>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(default)]
    packaged_volume: Option<f64>,
    #[serde(default)]
    dogma_attributes: Vec<DogmaAttribute>,
}

#[derive(Deserialize)]
struct RawGroup {
    name: String,
    category_id: i64,
}

#[derive(Deserialize)]
struct RawSystem {
    system_id: i64,
    name: String,
    security_status: f64,
    constellation_id: i64,
}

#[derive(Deserialize)]
struct RawConstellation {
    region_id: i64,
}

#[derive(Deserialize)]
struct RawStation {
    station_id: i64,
    name: String,
    system_id: i64,
    #[serde(default)]
    type_id: i64,
    #[serde(default)]
    owner: Option<i64>,
}

#[derive(Deserialize)]
struct RawStructure {
    name: String,
    solar_system_id: i64,
    #[serde(default)]
    type_id: Option<i64>,
    #[serde(default)]
    owner_id: i64,
}

fn cache_path() -> std::path::PathBuf {
    store::cache_dir().join("universe.json")
}

impl Universe {
    pub fn load() -> Self {
        Universe { cache: Mutex::new(store::read_json(&cache_path()).unwrap_or_default()), dirty: AtomicBool::new(false) }
    }

    pub fn flush(&self) {
        if self.dirty.swap(false, Ordering::SeqCst) {
            let c = self.cache.lock();
            if let Err(e) = store::write_json(&cache_path(), &*c, false) {
                tracing::warn!("saving universe cache: {e}");
            }
        }
    }

    fn touch(&self) {
        self.dirty.store(true, Ordering::SeqCst);
    }

    /// Cached name, without calling ESI.
    pub fn cached_name(&self, id: i64) -> Option<String> {
        let c = self.cache.lock();
        c.names.get(&id).cloned().or_else(|| c.types.get(&id).map(|t| t.name.clone()))
    }

    /// Names for ids (types, stations, systems, characters, corporations, alliances, factions); unknown ids are
    /// left out. universe/names rejects a whole request if any id is unknown, so a failed chunk is retried one by
    /// one.
    pub async fn names(&self, esi: &Esi, ids: impl IntoIterator<Item = i64>) -> HashMap<i64, String> {
        let ids: Vec<i64> = ids.into_iter().filter(|&i| is_nameable(i)).collect::<HashSet<_>>().into_iter().collect();
        let missing: Vec<i64> = {
            let c = self.cache.lock();
            ids.iter().copied().filter(|i| !c.names.contains_key(i)).collect()
        };
        for chunk in missing.chunks(1000) {
            match esi.post("universe/names").body(chunk).json::<Vec<NameEntry>>().await {
                Ok(list) => self.insert_names(list),
                Err(err) => {
                    tracing::debug!("bulk name lookup failed ({err}), trying ids singly");
                    if chunk.len() > 1 && chunk.len() <= 50 {
                        for &id in chunk {
                            if let Ok(list) = esi.post("universe/names").body([id]).json::<Vec<NameEntry>>().await {
                                self.insert_names(list);
                            }
                        }
                    }
                }
            }
        }
        let c = self.cache.lock();
        ids.into_iter().filter_map(|i| c.names.get(&i).map(|n| (i, n.clone()))).collect()
    }

    fn insert_names(&self, list: Vec<NameEntry>) {
        let mut c = self.cache.lock();
        for n in list {
            c.names.insert(n.id, n.name);
        }
        drop(c);
        self.touch();
    }

    pub async fn name(&self, esi: &Esi, id: i64) -> Option<String> {
        self.names(esi, [id]).await.remove(&id)
    }

    pub fn cached_type(&self, id: i64) -> Option<TypeInfo> {
        self.cache.lock().types.get(&id).cloned()
    }

    pub async fn type_info(&self, esi: &Esi, id: i64) -> Result<TypeInfo, EsiError> {
        if let Some(t) = self.cached_type(id) {
            if t.fetched.map(|f| f > Utc::now() - Duration::days(TYPE_TTL_DAYS)).unwrap_or(false) {
                return Ok(t);
            }
        }
        let raw: RawType = esi.get(format!("universe/types/{id}")).json().await?;
        let group: Option<RawGroup> = esi.get(format!("universe/groups/{}", raw.group_id)).json().await.ok();
        let info = TypeInfo {
            type_id: raw.type_id,
            name: raw.name,
            description: raw.description,
            group_id: raw.group_id,
            group_name: group.as_ref().map(|g| g.name.clone()).unwrap_or_default(),
            category_id: group.map(|g| g.category_id).unwrap_or_default(),
            market_group_id: raw.market_group_id,
            volume: raw.volume,
            packaged_volume: raw.packaged_volume,
            dogma_attributes: raw.dogma_attributes,
            fetched: Some(Utc::now()),
        };
        let mut c = self.cache.lock();
        c.names.insert(id, info.name.clone());
        c.types.insert(id, info.clone());
        drop(c);
        self.touch();
        Ok(info)
    }

    /// Type infos for many ids, fetched concurrently (failures left out).
    pub async fn types(&self, esi: &Esi, ids: impl IntoIterator<Item = i64>) -> HashMap<i64, TypeInfo> {
        let ids: HashSet<i64> = ids.into_iter().collect();
        let futs = ids.into_iter().map(|id| async move { (id, self.type_info(esi, id).await) });
        futures::future::join_all(futs)
            .await
            .into_iter()
            .filter_map(|(id, r)| r.ok().map(|t| (id, t)))
            .collect()
    }

    pub async fn system(&self, esi: &Esi, id: i64) -> Result<SystemInfo, EsiError> {
        if let Some(s) = self.cache.lock().systems.get(&id).cloned() {
            return Ok(s);
        }
        let raw: RawSystem = esi.get(format!("universe/systems/{id}")).json().await?;
        let region_id = esi
            .get(format!("universe/constellations/{}", raw.constellation_id))
            .json::<RawConstellation>()
            .await
            .map(|c| c.region_id)
            .unwrap_or_default();
        let region_name = if region_id > 0 { self.name(esi, region_id).await.unwrap_or_default() } else { String::new() };
        let info = SystemInfo {
            system_id: raw.system_id,
            name: raw.name,
            security_status: raw.security_status,
            constellation_id: raw.constellation_id,
            region_id,
            region_name,
        };
        let mut c = self.cache.lock();
        c.systems.insert(id, info.clone());
        c.names.insert(id, info.name.clone());
        drop(c);
        self.touch();
        Ok(info)
    }

    pub async fn station(&self, esi: &Esi, id: i64) -> Result<StationInfo, EsiError> {
        if let Some(s) = self.cache.lock().stations.get(&id).cloned() {
            return Ok(s);
        }
        let raw: RawStation = esi.get(format!("universe/stations/{id}")).json().await?;
        let info = StationInfo { station_id: raw.station_id, name: raw.name, system_id: raw.system_id, type_id: raw.type_id, owner: raw.owner };
        let mut c = self.cache.lock();
        c.stations.insert(id, info.clone());
        c.names.insert(id, info.name.clone());
        drop(c);
        self.touch();
        Ok(info)
    }

    /// A player structure, if this character has docking access (a refusal is remembered per character).
    pub async fn structure(&self, esi: &Esi, id: i64, auth: &Auth) -> Option<StructureInfo> {
        {
            let c = self.cache.lock();
            if let Some(s) = c.structures.get(&id) {
                return Some(s.clone());
            }
            if c.structure_denied.get(&id).map(|s| s.contains(&auth.character_id)).unwrap_or(false) {
                return None;
            }
        }
        match esi
            .get(format!("universe/structures/{id}"))
            .auth(auth)
            .scope("esi-universe.read_structures.v1")
            .json::<RawStructure>()
            .await
        {
            Ok(raw) => {
                let info = StructureInfo { structure_id: id, name: raw.name, system_id: raw.solar_system_id, type_id: raw.type_id, owner_id: raw.owner_id };
                self.cache.lock().structures.insert(id, info.clone());
                self.touch();
                Some(info)
            }
            Err(err) => {
                if err.is_forbidden() || err.is_not_found() {
                    self.cache.lock().structure_denied.entry(id).or_default().insert(auth.character_id);
                    self.touch();
                }
                None
            }
        }
    }

    /// Resolves a station, structure or system id for display. Structures need `auth` with docking access.
    pub async fn place(&self, esi: &Esi, id: i64, auth: Option<&Auth>) -> Place {
        let mut place = Place { id, kind: "unknown".into(), name: format!("Location #{id}"), ..Default::default() };
        let system_id = if is_station(id) {
            if let Ok(st) = self.station(esi, id).await {
                place.kind = "station".into();
                place.name = st.name;
                Some(st.system_id)
            } else {
                None
            }
        } else if is_system(id) {
            place.kind = "system".into();
            Some(id)
        } else if id > 1_000_000_000_000 {
            place.kind = "structure".into();
            match auth {
                Some(a) => match self.structure(esi, id, a).await {
                    Some(s) => {
                        place.name = s.name;
                        Some(s.system_id)
                    }
                    None => {
                        place.name = format!("Structure #{id} (no access)");
                        None
                    }
                },
                None => None,
            }
        } else {
            None
        };
        if let Some(sid) = system_id {
            if let Ok(sys) = self.system(esi, sid).await {
                if place.kind == "system" {
                    place.name = sys.name.clone();
                }
                place.system_id = Some(sid);
                place.security = Some(sys.display_security());
                place.region_name = Some(sys.region_name.clone()).filter(|r| !r.is_empty());
                place.system_name = Some(sys.name);
            }
        }
        place
    }

    /// Places for many ids at once.
    pub async fn places(&self, esi: &Esi, ids: impl IntoIterator<Item = i64>, auth: Option<&Auth>) -> HashMap<i64, Place> {
        let ids: HashSet<i64> = ids.into_iter().collect();
        let futs = ids.into_iter().map(|id| async move { (id, self.place(esi, id, auth).await) });
        futures::future::join_all(futs).await.into_iter().collect()
    }
}
