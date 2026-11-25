//! Finding stations and structures by name, for a doctrine's stock places. Three sources, so a place can be picked
//! before anyone keeps anything there:
//! - places the characters already use: asset locations, where they're docked, jump clones and home stations;
//! - the NPC stations of systems whose name matches (public);
//! - each character's own ESI search, which finds NPC stations and the player structures that character can dock
//!   at, private alliance structures included. That needs [`SEARCH_SCOPE`].

use crate::esi::Auth;
use crate::character::Character;
use crate::sde::MAP;
use crate::Core;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

/// ESI calls this "search structures", but a character search with it also covers NPC stations.
pub const SEARCH_SCOPE: &str = "esi-search.search_structures.v1";
const STRUCTURE_SCOPE: &str = "esi-universe.read_structures.v1";
/// ESI's character search wants at least this many characters.
pub const MIN_SEARCH_LEN: usize = 3;
/// Results resolved per search (each structure is a lookup).
const MAX_RESULTS: usize = 25;
/// Systems whose NPC stations are listed for a query.
const STATION_SYSTEMS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Why {
    /// A character is docked there now.
    Docked,
    /// A character has assets there.
    Assets,
    /// A character's jump clone is there.
    Clone,
    /// A character's home station.
    Home,
    /// An NPC station in a system that matched.
    InSystem,
    /// Found by a character's ESI search.
    Search,
}

impl Why {
    pub fn label(&self) -> &'static str {
        match self {
            Why::Docked => "docked there",
            Why::Assets => "assets there",
            Why::Clone => "jump clone there",
            Why::Home => "home station",
            Why::InSystem => "in this system",
            Why::Search => "found by search",
        }
    }
}

/// A station or structure that matched.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub id: i64,
    pub name: String,
    pub structure: bool,
    pub system_name: String,
    pub why: Why,
}

/// What the remote search found, and how far it could look.
#[derive(Clone, Debug, Default)]
pub struct Remote {
    pub found: Vec<Found>,
    /// Characters whose search ran.
    pub searched: usize,
    /// Characters that couldn't search: their login lacks [`SEARCH_SCOPE`].
    pub without_scope: usize,
}

/// Places the characters already use whose name contains `query` (no network).
pub fn known(chars: &[Arc<Character>], query: &str) -> Vec<Found> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut out: BTreeMap<i64, Found> = BTreeMap::new();
    let mut add = |p: &crate::universe::Place, why: Why| {
        if !matches!(p.kind.as_str(), "station" | "structure") || !p.name.to_lowercase().contains(&q) {
            return;
        }
        let f = Found { id: p.id, name: p.name.clone(), structure: p.kind == "structure", system_name: p.system_name.clone().unwrap_or_default(), why };
        // the more direct reason wins (docked beats assets beats clone…)
        match out.get(&p.id) {
            Some(had) if had.why <= f.why => {}
            _ => {
                out.insert(p.id, f);
            }
        }
    };
    for c in chars {
        if let Some(a) = &c.assets {
            for p in a.locations.values() {
                add(p, Why::Assets);
            }
        }
        if let Some(p) = c.location.as_ref().and_then(|l| l.docked.as_ref()) {
            add(p, Why::Docked);
        }
        if let Some(cl) = &c.clones {
            for j in &cl.jump_clones {
                add(&j.place, Why::Clone);
            }
            if let Some(p) = &cl.home {
                add(p, Why::Home);
            }
        }
    }
    sort(out.into_values().collect(), &q)
}

/// Names starting with the query first, then shorter names.
fn sort(mut v: Vec<Found>, q: &str) -> Vec<Found> {
    v.sort_by_key(|f| (!f.name.to_lowercase().starts_with(q), f.name.len(), f.name.clone()));
    v
}

#[derive(Deserialize, Default)]
struct SearchHits {
    #[serde(default)]
    station: Vec<i64>,
    #[serde(default)]
    structure: Vec<i64>,
}

#[derive(Deserialize)]
struct SystemStations {
    #[serde(default)]
    stations: Vec<i64>,
}

/// NPC stations of systems matching `query`, and stations and structures the characters' searches find. One
/// character per corporation searches: structure access is almost always granted by corporation or alliance, and
/// a hundred characters shouldn't mean a hundred searches.
pub async fn remote(core: &Core, chars: &[Arc<Character>], query: &str) -> Remote {
    let q = query.trim().to_lowercase();
    let mut out = Remote::default();
    if q.chars().count() < MIN_SEARCH_LEN {
        return out;
    }
    let mut found: HashMap<i64, Found> = HashMap::new();

    for sys in MAP.search_systems(&q, STATION_SYSTEMS) {
        let Ok(s) = core.esi.get(format!("universe/systems/{}", sys.id)).json::<SystemStations>().await else { continue };
        for id in s.stations {
            if let Ok(st) = core.universe.station(&core.esi, id).await {
                found.insert(id, Found { id, name: st.name, structure: false, system_name: sys.name.clone(), why: Why::InSystem });
            }
        }
    }

    let corp_of: HashMap<i64, i64> = chars.iter().filter_map(|c| Some((c.id, c.info.as_ref()?.corporation_id))).collect();
    let mut corps_done: HashSet<i64> = HashSet::new();
    let mut searchers: Vec<Auth> = Vec::new();
    for a in core.auth.all() {
        if a.needs_reauth() {
            continue;
        }
        if !a.has_scope(SEARCH_SCOPE) {
            out.without_scope += 1;
            continue;
        }
        // characters without info yet search on their own
        if let Some(corp) = corp_of.get(&a.id) {
            if !corps_done.insert(*corp) {
                continue;
            }
        }
        if let Ok(auth) = core.auth.auth(&core.esi.http, a.id, 120).await {
            searchers.push(auth);
        }
    }
    // a structure is resolved with a character that found it, so it has docking access
    let mut structures: Vec<(i64, usize)> = Vec::new();
    for (i, auth) in searchers.iter().enumerate() {
        let hits: SearchHits = match core
            .esi
            .get(format!("characters/{}/search/", auth.character_id))
            .auth(auth)
            .scope(SEARCH_SCOPE)
            .query("categories", "station,structure")
            .query("search", &q)
            .query("strict", "false")
            .json()
            .await
        {
            Ok(h) => h,
            Err(e) => {
                tracing::debug!("place search for {}: {e}", auth.character_id);
                continue;
            }
        };
        out.searched += 1;
        for id in hits.station.into_iter().take(MAX_RESULTS) {
            if found.contains_key(&id) {
                continue;
            }
            if let Ok(st) = core.universe.station(&core.esi, id).await {
                let system_name = MAP.system(st.system_id).map(|s| s.name.clone()).unwrap_or_default();
                found.insert(id, Found { id, name: st.name, structure: false, system_name, why: Why::Search });
            }
        }
        for id in hits.structure {
            if !structures.iter().any(|(s, _)| *s == id) {
                structures.push((id, i));
            }
        }
    }
    for (id, i) in structures.into_iter().take(MAX_RESULTS) {
        let auth = &searchers[i];
        if !auth.has_scope(STRUCTURE_SCOPE) {
            continue;
        }
        if let Some(s) = core.universe.structure(&core.esi, id, auth).await {
            let system_name = MAP.system(s.system_id).map(|x| x.name.clone()).unwrap_or_default();
            found.insert(id, Found { id, name: s.name, structure: true, system_name, why: Why::Search });
        }
    }
    core.universe.flush();
    out.found = sort(found.into_values().collect(), &q);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::assets::AssetsData;
    use crate::domains::clones::{Clones, JumpClone};
    use crate::domains::location::Location;
    use crate::universe::Place;

    fn place(id: i64, name: &str, kind: &str) -> Place {
        Place { id, name: name.into(), kind: kind.into(), system_name: Some("CTAC-N".into()), ..Default::default() }
    }

    #[test]
    fn known_places_from_every_source() {
        let mut a = Character::new(1);
        let mut assets = AssetsData::default();
        assets.locations.insert(10, place(10, "CTAC-N - Fountain of Life", "structure"));
        assets.locations.insert(11, place(11, "CTAC-N", "system"));
        a.assets = Some(assets);
        let mut b = Character::new(2);
        b.location = Some(Location { system: place(11, "CTAC-N", "system"), docked: Some(place(10, "CTAC-N - Fountain of Life", "structure")) });
        b.clones = Some(Clones {
            home: Some(place(20, "Horkison VI - Moon 2 - Station", "station")),
            jump_clones: vec![JumpClone {
                jump_clone_id: 1,
                name: None,
                location_id: 30,
                location_type: "structure".into(),
                place: place(30, "CTAC-N - Staging Fortizar", "structure"),
                implants: vec![],
            }],
            ..Default::default()
        });
        let chars = vec![Arc::new(a), Arc::new(b)];
        let got: Vec<(i64, Why)> = known(&chars, "ctac").into_iter().map(|f| (f.id, f.why)).collect();
        // systems aren't stations; docked beats assets for the Keepstar
        assert_eq!(got, [(10, Why::Docked), (30, Why::Clone)]);
        assert_eq!(known(&chars, "horkison")[0].why, Why::Home);
        assert!(known(&chars, " ").is_empty());
    }
}
