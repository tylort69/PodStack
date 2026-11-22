//! Static game data that follows CCP's releases without a PodStack update.
//!
//! The market types and groups, the map and the industry data (blueprints, reactions, structures, rigs, decryptors
//! and skill bonuses) come from CCP's static data export. PodStack ships a snapshot in `data/` so it works offline
//! from the first start, and in the background checks CCP's latest build (every few hours): when CCP publishes a new
//! one, PodStack downloads it, converts the parts it uses into `$XDG_CACHE_HOME/podstack/sde/` and swaps the new data
//! in while running. Balance changes to blueprints, rigs or skills therefore reach PodStack within hours.
//!
//! The conversion here is also what produced the bundled snapshot: `cargo run -p podstack-core --example
//! convert_sde -- <unzipped SDE dir>` writes `data/`.

use crate::{config, store};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::OnceLock;

/// Bumped whenever the converted files change shape, so caches written by an older PodStack are converted again.
pub const FORMAT: u32 = 2;

const LATEST_URL: &str = "https://developers.eveonline.com/static-data/tranquility/latest.jsonl";
/// How often to ask CCP whether there's a newer build.
const CHECK_HOURS: i64 = 6;

/// The converted files, by name.
pub const FILES: [&str; 5] = ["market_types.json", "market_groups.json", "universe.json", "industry.json", "pi.json"];

/// Data that can be replaced while the app runs. Reading it is as cheap as a static: the current value is leaked on
/// purpose, because references handed out earlier may still be in use when a newer build replaces it (that happens
/// at most once per CCP release).
pub struct Live<T: 'static> {
    cell: OnceLock<AtomicPtr<T>>,
    init: fn() -> T,
}

impl<T: 'static> Live<T> {
    pub const fn new(init: fn() -> T) -> Self {
        Live { cell: OnceLock::new(), init }
    }

    fn ptr(&self) -> &AtomicPtr<T> {
        self.cell.get_or_init(|| AtomicPtr::new(Box::into_raw(Box::new((self.init)()))))
    }

    /// Swaps in a new value (the old one stays allocated, see above).
    pub fn replace(&self, value: T) {
        self.ptr().store(Box::into_raw(Box::new(value)), Ordering::Release);
    }

    /// Loads the value again from the cache or the bundled snapshot.
    pub fn reload(&self) {
        self.replace((self.init)());
    }
}

impl<T: 'static> std::ops::Deref for Live<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the pointer always comes from Box::into_raw and is never freed.
        unsafe { &*self.ptr().load(Ordering::Acquire) }
    }
}

/// Which build the data in use comes from.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct BuildInfo {
    pub build: i64,
    #[serde(default)]
    pub release: Option<DateTime<Utc>>,
    #[serde(default)]
    pub format: u32,
}

/// What `status()` reports.
#[derive(Clone, Debug)]
pub struct Status {
    pub build: i64,
    pub release: Option<DateTime<Utc>>,
    /// True while PodStack runs on the snapshot it was built with.
    pub bundled: bool,
    pub checked: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct CheckState {
    checked: Option<DateTime<Utc>>,
}

fn dir() -> PathBuf {
    store::cache_dir().join("sde")
}

const BUNDLED_BUILD: &str = include_str!("../../../data/sde_build.json");

fn bundled_build() -> BuildInfo {
    serde_json::from_str(BUNDLED_BUILD).unwrap_or_default()
}

/// The cached conversion, if it's usable and newer than the bundled snapshot.
fn cached_build() -> Option<BuildInfo> {
    let b: BuildInfo = store::read_json(&dir().join("build.json"))?;
    let complete = FILES.iter().all(|f| dir().join(f).exists());
    (b.format == FORMAT && complete && b.build > bundled_build().build).then_some(b)
}

/// The text of a converted file: from the newest cached build, else the bundled snapshot.
pub fn read(name: &str, bundled: &'static str) -> String {
    if cached_build().is_some() {
        match std::fs::read_to_string(dir().join(name)) {
            Ok(s) => return s,
            Err(err) => tracing::warn!("static data {name}: {err}"),
        }
    }
    bundled.to_string()
}

pub fn status() -> Status {
    let checked = store::read_json::<CheckState>(&dir().join("checked.json")).and_then(|c| c.checked);
    match cached_build() {
        Some(b) => Status { build: b.build, release: b.release, bundled: false, checked },
        None => {
            let b = bundled_build();
            Status { build: b.build, release: b.release, bundled: true, checked }
        }
    }
}

/// Reloads every live table from disk (after an update).
fn reload_all() {
    crate::sde::MARKET.reload();
    crate::sde::MAP.reload();
    crate::blueprints::INDUSTRY.reload();
    crate::pi::PI.reload();
}

/// Asks CCP for its latest build and, when it's newer than the data in use, downloads and converts it and swaps it
/// in. Returns the new build number when the data changed. Without `force`, does nothing if it checked recently.
pub async fn update(force: bool) -> Result<Option<i64>, String> {
    // one download at a time
    static RUNNING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _running = RUNNING.lock().await;
    let state: CheckState = store::read_json(&dir().join("checked.json")).unwrap_or_default();
    if !force && state.checked.is_some_and(|c| Utc::now() - c < chrono::Duration::hours(CHECK_HOURS)) {
        return Ok(None);
    }
    let client = reqwest::Client::builder().user_agent(config::user_agent()).build().map_err(|e| e.to_string())?;
    let latest: BuildInfo = client
        .get(LATEST_URL)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| e.to_string())?
        .json::<LatestLine>()
        .await
        .map(|l| BuildInfo { build: l.build_number, release: l.release_date, format: FORMAT })
        .map_err(|e| e.to_string())?;
    let mark_checked = || {
        let _ = std::fs::create_dir_all(dir());
        let _ = store::write_json(&dir().join("checked.json"), &CheckState { checked: Some(Utc::now()) }, false);
    };
    if latest.build <= status().build {
        mark_checked();
        return Ok(None);
    }
    tracing::info!("static data: downloading build {}", latest.build);
    let url = format!("https://developers.eveonline.com/static-data/tranquility/eve-online-static-data-{}-jsonl.zip", latest.build);
    std::fs::create_dir_all(dir()).map_err(|e| e.to_string())?;
    let zip_path = dir().join("download.zip");
    {
        use tokio::io::AsyncWriteExt;
        let mut resp = client.get(&url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
        let mut file = tokio::fs::File::create(&zip_path).await.map_err(|e| e.to_string())?;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        }
        file.flush().await.map_err(|e| e.to_string())?;
    }
    let out = dir().join("new");
    let info = latest.clone();
    let (zp, o) = (zip_path.clone(), out.clone());
    tokio::task::spawn_blocking(move || convert_zip(&zp, &o, &info)).await.map_err(|e| e.to_string())??;
    for f in FILES.iter().chain(["build.json"].iter()) {
        std::fs::rename(out.join(f), dir().join(f)).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_file(&zip_path);
    mark_checked();
    reload_all();
    tracing::info!("static data: now on build {}", latest.build);
    Ok(Some(latest.build))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LatestLine {
    build_number: i64,
    #[serde(default)]
    release_date: Option<DateTime<Utc>>,
}

fn convert_zip(zip_path: &Path, out: &Path, info: &BuildInfo) -> Result<(), String> {
    let open = |name: &str| -> Result<Box<dyn BufRead>, String> {
        let file = std::fs::File::open(zip_path).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let mut entry = archive.by_name(&format!("{name}.jsonl")).map_err(|e| format!("{name}: {e}"))?;
        // Entries are read whole: ZipFile borrows the archive, and the biggest file used is ~160 MB of text that
        // is parsed and dropped one file at a time.
        let mut buf = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        Ok(Box::new(std::io::Cursor::new(buf)))
    };
    convert(&open, out, info)
}

/// Converts an unzipped SDE directory (for the bundled snapshot).
pub fn convert_dir(src: &Path, out: &Path) -> Result<BuildInfo, String> {
    let open = |name: &str| -> Result<Box<dyn BufRead>, String> {
        let f = std::fs::File::open(src.join(format!("{name}.jsonl"))).map_err(|e| format!("{name}: {e}"))?;
        Ok(Box::new(BufReader::new(f)))
    };
    let mut info = BuildInfo { format: FORMAT, ..Default::default() };
    for line in open("_sde")?.lines() {
        if let Ok(l) = serde_json::from_str::<LatestLine>(&line.map_err(|e| e.to_string())?) {
            info.build = l.build_number;
            info.release = l.release_date;
        }
    }
    convert(&open, out, &info)?;
    Ok(info)
}

// ---------- conversion ----------

type Opener<'a> = dyn Fn(&str) -> Result<Box<dyn BufRead>, String> + 'a;

fn each<T: for<'de> Deserialize<'de>>(open: &Opener, name: &str, mut f: impl FnMut(T)) -> Result<(), String> {
    for line in open(name)?.lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<T>(&line) {
            Ok(v) => f(v),
            Err(err) => return Err(format!("{name}: {err}")),
        }
    }
    Ok(())
}

#[derive(Deserialize, Default)]
struct Names {
    #[serde(default)]
    en: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawType {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(rename = "groupID", default)]
    group: i64,
    #[serde(default)]
    name: Names,
    #[serde(default)]
    published: bool,
    #[serde(rename = "marketGroupID", default)]
    market_group: Option<i64>,
    #[serde(default)]
    packaged_volume: Option<f64>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(rename = "metaGroupID", default)]
    meta_group: Option<i64>,
    #[serde(default)]
    capacity: Option<f64>,
}

#[derive(Deserialize)]
struct RawGroup {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(rename = "categoryID", default)]
    category: i64,
    #[serde(default)]
    name: Names,
}

#[derive(Deserialize)]
struct RawMarketGroup {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(rename = "parentGroupID", default)]
    parent: Option<i64>,
    #[serde(default)]
    name: Names,
}

#[derive(Deserialize)]
struct RawRegion {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(default)]
    name: Names,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSystem {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(default)]
    name: Names,
    #[serde(rename = "regionID")]
    region: i64,
    #[serde(default)]
    security_status: f64,
}

#[derive(Deserialize)]
struct RawDogma {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(rename = "dogmaAttributes", default)]
    attributes: Vec<RawAttrValue>,
}

#[derive(Deserialize)]
struct RawAttrValue {
    #[serde(rename = "attributeID")]
    id: i64,
    value: f64,
}

#[derive(Deserialize)]
struct RawAttr {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct RawBlueprint {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(rename = "maxProductionLimit", default)]
    limit: i64,
    #[serde(default)]
    activities: BTreeMap<String, RawActivity>,
}

#[derive(Deserialize)]
struct RawActivity {
    #[serde(default)]
    time: i64,
    #[serde(default)]
    materials: Vec<RawQty>,
    #[serde(default)]
    products: Vec<RawQty>,
    #[serde(default)]
    skills: Vec<RawSkill>,
}

#[derive(Deserialize)]
struct RawQty {
    #[serde(rename = "typeID")]
    id: i64,
    quantity: i64,
    #[serde(default)]
    probability: Option<f64>,
}

#[derive(Deserialize)]
struct RawSkill {
    #[serde(rename = "typeID")]
    id: i64,
    level: i64,
}

#[derive(Deserialize)]
struct RawFilter {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(default)]
    name: String,
    #[serde(rename = "categoryIDs", default)]
    categories: Vec<i64>,
    #[serde(rename = "groupIDs", default)]
    groups: Vec<i64>,
}

#[derive(Deserialize)]
struct RawModifier {
    #[serde(rename = "dogmaAttributeID")]
    attribute: i64,
    #[serde(rename = "filterID", default)]
    filter: Option<i64>,
}

/// The industry file: see `sde::IndustryData` for what each part means.
#[derive(Serialize, Deserialize, Default)]
pub struct IndustryFile {
    /// (blueprint, max runs per copy, activities)
    pub blueprints: Vec<(i64, i64, BTreeMap<String, ActivityFile>)>,
    /// (type, name, group, published) of every type the industry data mentions
    pub types: Vec<(i64, String, i64, bool)>,
    /// (group, category, name)
    pub groups: Vec<(i64, i64, String)>,
    /// (filter, name, categories, groups): what a rig's bonus applies to
    pub filters: Vec<(i64, String, Vec<i64>, Vec<i64>)>,
    pub structures: Vec<StructureFile>,
    pub rigs: Vec<RigFile>,
    /// (type, name, probability multiplier, ME, TE, runs)
    pub decryptors: Vec<(i64, String, f64, i64, i64, i64)>,
    /// (skill, attribute name, value per level) for skills in the industry skill groups
    pub skill_bonuses: Vec<(i64, String, f64)>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct ActivityFile {
    pub time: i64,
    #[serde(default)]
    pub materials: Vec<(i64, i64)>,
    /// (type, quantity, invention probability)
    #[serde(default)]
    pub products: Vec<(i64, i64, Option<f64>)>,
    #[serde(default)]
    pub skills: Vec<(i64, i64)>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct StructureFile {
    pub id: i64,
    pub name: String,
    pub group: i64,
    /// Which rigs fit: 2 medium, 3 large, 4 extra large.
    #[serde(default)]
    pub rig_size: i64,
    /// (activity, "material" | "time" | "cost", multiplier such as 0.99)
    pub bonuses: Vec<(String, String, f64)>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct RigFile {
    pub id: i64,
    pub name: String,
    /// 2 medium, 3 large, 4 extra large
    pub size: i64,
    pub tech: i64,
    /// How much stronger the bonus is in high, low and null security; `None` where the rig doesn't work.
    pub security: (Option<f64>, Option<f64>, Option<f64>),
    /// (activity, "material" | "time" | "cost", percent such as -2.0, filter)
    pub bonuses: Vec<(String, String, f64, Option<i64>)>,
}

/// Skill groups whose skills change industry jobs: Production, Science, Resource Processing (reactions).
const INDUSTRY_SKILL_GROUPS: [i64; 3] = [268, 270, 1218];
/// Group of the T2 invention decryptors.
const DECRYPTOR_GROUP: i64 = 1304;
/// Structures with industry bonuses: Engineering Complex, Refinery, Citadel.
const STRUCTURE_GROUPS: [i64; 3] = [1404, 1406, 1657];

/// The modifier tables name activities differently from the blueprint tables.
fn activity_key(name: &str) -> String {
    match name {
        "researchMaterial" => "research_material".into(),
        "researchTime" => "research_time".into(),
        other => other.into(),
    }
}

fn convert(open: &Opener, out: &Path, info: &BuildInfo) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let write = |name: &str, value: &dyn erased::Json| -> Result<(), String> {
        std::fs::write(out.join(name), value.to_json()).map_err(|e| format!("{name}: {e}"))
    };

    // market groups
    let mut market_groups: Vec<(i64, Option<i64>, String)> = Vec::new();
    each::<RawMarketGroup>(open, "marketGroups", |g| market_groups.push((g.id, g.parent, g.name.en)))?;
    market_groups.sort_by_key(|g| g.0);
    write("market_groups.json", &market_groups)?;

    // types: everything is read once; the market list keeps published types with a market group
    let mut types: HashMap<i64, RawType> = HashMap::new();
    each::<RawType>(open, "types", |t| {
        types.insert(t.id, t);
    })?;
    let mut market: Vec<(i64, &str, i64, f64, Option<i64>)> = types
        .values()
        .filter(|t| t.published && t.market_group.is_some())
        // Packaged volume is what a hauler moves; ships and containers are only traded packaged.
        .map(|t| {
            let v = t.packaged_volume.or(t.volume).unwrap_or(0.0);
            (t.id, t.name.en.as_str(), t.market_group.unwrap_or(0), (v * 10000.0).round() / 10000.0, t.meta_group)
        })
        .collect();
    market.sort_by_key(|t| t.0);
    write("market_types.json", &market)?;

    // map: wormhole space (11xxxxxx) and the abyss have no markets
    let mut regions: Vec<(i64, String)> = Vec::new();
    each::<RawRegion>(open, "mapRegions", |r| {
        if r.id < 11000000 {
            regions.push((r.id, r.name.en))
        }
    })?;
    regions.sort();
    let known: HashSet<i64> = regions.iter().map(|r| r.0).collect();
    let mut systems: Vec<(i64, String, i64, f64)> = Vec::new();
    each::<RawSystem>(open, "mapSolarSystems", |s| {
        if known.contains(&s.region) {
            systems.push((s.id, s.name.en, s.region, (s.security_status * 10000.0).round() / 10000.0))
        }
    })?;
    systems.sort_by_key(|s| s.0);
    let known_systems: HashSet<i64> = systems.iter().map(|s| s.0).collect();
    write("universe.json", &serde_json::json!({ "regions": regions, "systems": systems }))?;

    // industry
    let mut groups: HashMap<i64, RawGroup> = HashMap::new();
    each::<RawGroup>(open, "groups", |g| {
        groups.insert(g.id, g);
    })?;
    let mut attr_names: HashMap<i64, String> = HashMap::new();
    each::<RawAttr>(open, "dogmaAttributes", |a| {
        attr_names.insert(a.id, a.name);
    })?;
    let mut blueprints = Vec::new();
    let mut mentioned: BTreeSet<i64> = BTreeSet::new();
    each::<RawBlueprint>(open, "blueprints", |b| {
        // Blueprints that can't be bought or used (unpublished) only clutter the lists.
        if !types.get(&b.id).is_some_and(|t| t.published) {
            return;
        }
        mentioned.insert(b.id);
        let acts = b
            .activities
            .into_iter()
            .map(|(name, a)| {
                for q in a.materials.iter().chain(&a.products) {
                    mentioned.insert(q.id);
                }
                for s in &a.skills {
                    mentioned.insert(s.id);
                }
                let file = ActivityFile {
                    time: a.time,
                    materials: a.materials.iter().map(|q| (q.id, q.quantity)).collect(),
                    products: a.products.iter().map(|q| (q.id, q.quantity, q.probability)).collect(),
                    skills: a.skills.iter().map(|s| (s.id, s.level)).collect(),
                };
                (name, file)
            })
            .collect();
        blueprints.push((b.id, b.limit, acts));
    })?;
    blueprints.sort_by_key(|b| b.0);

    let mut dogma: HashMap<i64, HashMap<String, f64>> = HashMap::new();
    let wanted: HashSet<i64> = types
        .values()
        .filter(|t| {
            let g = groups.get(&t.group);
            t.published
                && (t.group == DECRYPTOR_GROUP
                    || STRUCTURE_GROUPS.contains(&t.group)
                    || INDUSTRY_SKILL_GROUPS.contains(&t.group)
                    || PI_COMMODITY_GROUPS.contains(&t.group)
                    || PI_PIN_GROUPS.contains(&t.group)
                    || g.is_some_and(|g| g.name.en.starts_with("Structure ") && g.name.en.contains("Rig")))
        })
        .map(|t| t.id)
        .collect();
    each::<RawDogma>(open, "typeDogma", |d| {
        if wanted.contains(&d.id) {
            let attrs = d.attributes.iter().filter_map(|a| attr_names.get(&a.id).map(|n| (n.clone(), a.value))).collect();
            dogma.insert(d.id, attrs);
        }
    })?;

    let mut filters = Vec::new();
    each::<RawFilter>(open, "industryTargetFilters", |f| filters.push((f.id, f.name, f.categories, f.groups)))?;
    filters.sort_by_key(|f| f.0);

    let mut structures = Vec::new();
    let mut rigs = Vec::new();
    each::<serde_json::Map<String, serde_json::Value>>(open, "industryModifierSources", |m| {
        let Some(id) = m.get("_key").and_then(|v| v.as_i64()) else { return };
        let Some(t) = types.get(&id).filter(|t| t.published) else { return };
        let attrs = dogma.get(&id).cloned().unwrap_or_default();
        // (activity, kind, attribute, filter)
        let mut mods: Vec<(String, String, i64, Option<i64>)> = Vec::new();
        for (activity, kinds) in &m {
            let Some(kinds) = kinds.as_object() else { continue };
            for (kind, list) in kinds {
                let list: Vec<RawModifier> = serde_json::from_value(list.clone()).unwrap_or_default();
                for r in list {
                    mods.push((activity_key(activity), kind.clone(), r.attribute, r.filter));
                }
            }
        }
        if STRUCTURE_GROUPS.contains(&t.group) {
            // The structure carries the multiplier itself, in the attribute the modifier names.
            let bonuses = mods
                .iter()
                .filter_map(|(a, k, attr, _)| attr_names.get(attr).and_then(|n| attrs.get(n)).map(|v| (a.clone(), k.clone(), *v)))
                .collect();
            let rig_size = attrs.get("rigSize").copied().unwrap_or(0.0) as i64;
            structures.push(StructureFile { id, name: t.name.en.clone(), group: t.group, rig_size, bonuses });
        } else if groups.get(&t.group).is_some_and(|g| g.name.en.starts_with("Structure ") && g.name.en.contains("Rig")) {
            // A rig names the structure attribute it changes; its own size of bonus is in a "…Rig…Bonus" attribute.
            let percent = |kind: &str| -> Option<f64> {
                let suffix = match kind {
                    "material" => "MatBonus",
                    "time" => "TimeBonus",
                    _ => "CostBonus",
                };
                attrs.iter().find(|(n, _)| n.contains("Rig") && n.ends_with(suffix)).map(|(_, v)| *v)
            };
            let bonuses = mods
                .iter()
                .filter_map(|(a, k, _, f)| percent(k).filter(|p| *p != 0.0).map(|p| (a.clone(), k.clone(), p, *f)))
                .collect();
            let sec = |n: &str| attrs.get(n).copied();
            rigs.push(RigFile {
                id,
                name: t.name.en.clone(),
                size: attrs.get("rigSize").copied().unwrap_or(0.0) as i64,
                tech: attrs.get("techLevel").copied().unwrap_or(1.0) as i64,
                security: (sec("hiSecModifier"), sec("lowSecModifier"), sec("nullSecModifier")),
                bonuses,
            });
        }
        mentioned.insert(id);
    })?;
    // Structures without bonuses (Astrahus, Athanor, …) still run jobs.
    let listed: HashSet<i64> = structures.iter().map(|s| s.id).collect();
    for t in types.values().filter(|t| t.published && STRUCTURE_GROUPS.contains(&t.group) && !listed.contains(&t.id)) {
        let rig_size = dogma.get(&t.id).and_then(|d| d.get("rigSize")).copied().unwrap_or(0.0) as i64;
        structures.push(StructureFile { id: t.id, name: t.name.en.clone(), group: t.group, rig_size, bonuses: Vec::new() });
        mentioned.insert(t.id);
    }
    structures.sort_by_key(|s| s.id);
    rigs.sort_by_key(|r| r.id);

    let mut decryptors: Vec<(i64, String, f64, i64, i64, i64)> = types
        .values()
        .filter(|t| t.published && t.group == DECRYPTOR_GROUP)
        .filter_map(|t| {
            let d = dogma.get(&t.id)?;
            let v = |n: &str| d.get(n).copied().unwrap_or(0.0);
            Some((t.id, t.name.en.clone(), v("inventionPropabilityMultiplier"), v("inventionMEModifier") as i64, v("inventionTEModifier") as i64, v("inventionMaxRunModifier") as i64))
        })
        .collect();
    decryptors.sort_by_key(|d| d.0);
    for d in &decryptors {
        mentioned.insert(d.0);
    }

    // Skill bonuses: every per-level number on an industry skill except the training data every skill has.
    const SKILL_COMMON: [&str; 6] = ["primaryAttribute", "secondaryAttribute", "skillTimeConstant", "canNotBeTrainedOnTrial", "skillLevel", "requiredSkill"];
    let mut skill_bonuses: Vec<(i64, String, f64)> = dogma
        .iter()
        .filter(|(id, _)| types.get(id).is_some_and(|t| INDUSTRY_SKILL_GROUPS.contains(&t.group)))
        .flat_map(|(id, attrs)| {
            attrs
                .iter()
                .filter(|(n, v)| **v != 0.0 && !SKILL_COMMON.iter().any(|c| n.starts_with(c)))
                .map(move |(n, v)| (*id, n.clone(), *v))
        })
        .collect();
    skill_bonuses.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let mut type_rows: Vec<(i64, String, i64, bool)> =
        mentioned.iter().filter_map(|id| types.get(id).map(|t| (t.id, t.name.en.clone(), t.group, t.published))).collect();
    type_rows.sort_by_key(|t| t.0);
    let used_groups: BTreeSet<i64> = type_rows.iter().map(|t| t.2).collect();
    let group_rows: Vec<(i64, i64, String)> =
        used_groups.iter().filter_map(|g| groups.get(g).map(|g| (g.id, g.category, g.name.en.clone()))).collect();

    let industry = IndustryFile { blueprints, types: type_rows, groups: group_rows, filters, structures, rigs, decryptors, skill_bonuses };
    write("industry.json", &industry)?;

    // planetary industry
    let mut schematics = Vec::new();
    each::<RawSchematic>(open, "planetSchematics", |sc| {
        let inputs = sc.types.iter().filter(|t| t.is_input).map(|t| (t.id, t.quantity)).collect();
        if let Some(out) = sc.types.iter().find(|t| !t.is_input) {
            schematics.push((sc.id, sc.name.en, sc.cycle_time, inputs, (out.id, out.quantity), sc.pins));
        }
    })?;
    schematics.sort_by_key(|s| s.0);
    let mut commodities: Vec<(i64, String, i64, f64, f64)> = types
        .values()
        .filter(|t| t.published && PI_COMMODITY_GROUPS.contains(&t.group))
        .map(|t| {
            // Customs offices tax a fixed value per unit (its "tax multiplier"), not the market price.
            let tax_base = dogma.get(&t.id).and_then(|d| d.get("exportTaxMultiplier")).copied().unwrap_or(0.0);
            (t.id, t.name.en.clone(), t.group, t.volume.unwrap_or(0.0), tax_base)
        })
        .collect();
    commodities.sort_by_key(|c| c.0);
    let mut pins: Vec<PiPinFile> = types
        .values()
        .filter(|t| t.published && PI_PIN_GROUPS.contains(&t.group))
        .map(|t| {
            let d = dogma.get(&t.id);
            let v = |n: &str| d.and_then(|d| d.get(n)).copied();
            PiPinFile {
                id: t.id,
                name: t.name.en.clone(),
                group: t.group,
                planet_type: v("planetRestriction").map(|p| p as i64),
                capacity: t.capacity.unwrap_or(0.0),
                cpu: v("cpuLoad").or(v("cpuOutput")).unwrap_or(0.0),
                power: v("powerLoad").or(v("powerOutput")).unwrap_or(0.0),
                export_tax: v("exportTax"),
                import_tax: v("importTax"),
            }
        })
        .collect();
    pins.sort_by_key(|p| p.id);
    let mut planet_types: Vec<(i64, String)> =
        types.values().filter(|t| t.group == PLANET_GROUP).map(|t| (t.id, t.name.en.clone())).collect();
    planet_types.sort();
    // Every planet in known space by system: (system, [(planet type, celestial index)]).
    let mut by_system: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
    each::<RawPlanetRow>(open, "mapPlanets", |pl| {
        if known_systems.contains(&pl.system) {
            by_system.entry(pl.system).or_default().push((pl.type_id, pl.index));
        }
    })?;
    for v in by_system.values_mut() {
        v.sort_by_key(|p| p.1);
    }
    let pi = PiFile { schematics, commodities, pins, planet_types, planets: by_system.into_iter().collect() };
    write("pi.json", &pi)?;

    write("build.json", info)?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSchematic {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(default)]
    name: Names,
    #[serde(default)]
    cycle_time: i64,
    #[serde(default)]
    pins: Vec<i64>,
    #[serde(default)]
    types: Vec<RawSchematicType>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawSchematicType {
    #[serde(rename = "_key")]
    id: i64,
    #[serde(default)]
    is_input: bool,
    quantity: i64,
}

#[derive(Deserialize)]
struct RawPlanetRow {
    #[serde(rename = "solarSystemID")]
    system: i64,
    #[serde(rename = "typeID")]
    type_id: i64,
    #[serde(rename = "celestialIndex", default)]
    index: i64,
}

/// Raw resources, P1-P4 commodities: Planet Solid / Liquid-Gas / Organic raw resources, Basic, Refined,
/// Specialized and Advanced commodities.
pub const PI_COMMODITY_GROUPS: [i64; 7] = [1032, 1033, 1035, 1042, 1034, 1040, 1041];
/// Extractors, command centers, processors, storage, launchpads, extractor control units.
pub const PI_PIN_GROUPS: [i64; 6] = [1026, 1027, 1028, 1029, 1030, 1063];
const PLANET_GROUP: i64 = 7;

/// (schematic, name, cycle seconds, inputs (type, quantity), output (type, quantity), factory pin types)
pub type SchematicRow = (i64, String, i64, Vec<(i64, i64)>, (i64, i64), Vec<i64>);

/// The planetary industry file: see `pi::PiData`.
#[derive(Serialize, Deserialize, Default)]
pub struct PiFile {
    /// (schematic, name, cycle seconds, inputs (type, quantity), output (type, quantity), factory pin types)
    pub schematics: Vec<SchematicRow>,
    /// (type, name, group, volume m³, customs tax base value per unit)
    pub commodities: Vec<(i64, String, i64, f64, f64)>,
    pub pins: Vec<PiPinFile>,
    /// (type, name), e.g. (11, "Planet (Temperate)")
    pub planet_types: Vec<(i64, String)>,
    /// (system, [(planet type, celestial index)])
    pub planets: Vec<(i64, Vec<(i64, i64)>)>,
}

#[derive(Serialize, Deserialize, Default, Clone, Debug)]
pub struct PiPinFile {
    pub id: i64,
    pub name: String,
    pub group: i64,
    /// The planet type it can be built on.
    pub planet_type: Option<i64>,
    /// m³ it holds (storage, launchpads, command centers).
    pub capacity: f64,
    /// CPU and power it uses (a command center: provides).
    pub cpu: f64,
    pub power: f64,
    /// Percent of the tax base charged for exports and imports (launchpads, command centers).
    pub export_tax: Option<f64>,
    pub import_tax: Option<f64>,
}

/// Serializes the different tables through one `write` closure.
mod erased {
    pub trait Json {
        fn to_json(&self) -> String;
    }
    impl<T: serde::Serialize> Json for T {
        fn to_json(&self) -> String {
            serde_json::to_string(self).unwrap_or_default()
        }
    }
}
