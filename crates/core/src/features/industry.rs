//! Industry jobs and slots, owned blueprints, planetary extractors and asset lists, as the Assets, Asset Search,
//! Industry and PI pages show them.
//!
//! Everything takes `now` so the countdown logic can be tested.

use crate::blueprints::IndustryData;
use crate::character::Character;
use crate::domains::assets::{Asset, AssetsData};
use crate::domains::industry::{Colony, Extractor, Job, OwnedBlueprint};
use chrono::{DateTime, Utc};
use std::collections::HashMap;

// ---------- industry jobs ----------

pub fn activity_name(activity_id: i64) -> String {
    match activity_id {
        1 => "Manufacturing".into(),
        3 => "TE Research".into(),
        4 => "ME Research".into(),
        5 => "Copying".into(),
        7 => "Reverse Engineering".into(),
        8 => "Invention".into(),
        9 | 11 => "Reactions".into(),
        other => format!("Activity {other}"),
    }
}

/// Material Icons name for an activity.
pub fn activity_icon(activity_id: i64) -> &'static str {
    match activity_id {
        1 => "precision_manufacturing",
        3 => "timer",
        4 => "science",
        5 => "content_copy",
        7 => "settings_backup_restore",
        8 => "lightbulb",
        9 | 11 => "bubble_chart",
        _ => "build",
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JobState {
    /// Finished and waiting to be delivered.
    Ready,
    Active,
    Paused,
    /// Delivered, cancelled or reverted.
    Closed,
}

impl JobState {
    pub fn label(self) -> &'static str {
        match self {
            JobState::Ready => "Ready",
            JobState::Active => "In progress",
            JobState::Paused => "Paused",
            JobState::Closed => "Closed",
        }
    }
}

/// ESI keeps a job "active" until it's delivered, even after its end date.
pub fn job_state(job: &Job, now: DateTime<Utc>) -> JobState {
    match job.status.as_str() {
        "ready" => JobState::Ready,
        "active" if job.end_date <= now => JobState::Ready,
        "active" => JobState::Active,
        "paused" => JobState::Paused,
        _ => JobState::Closed,
    }
}

pub fn is_job_ready(job: &Job, now: DateTime<Utc>) -> bool {
    job_state(job, now) == JobState::Ready
}

/// 0..1. A paused job's progress stands still at its pause date.
pub fn job_progress(job: &Job, now: DateTime<Utc>) -> f64 {
    let at = if job.status == "paused" { job.pause_date.unwrap_or(now) } else { now };
    progress(job.start_date, job.end_date, at)
}

fn progress(start: DateTime<Utc>, end: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    let total = (end - start).num_milliseconds();
    if total <= 0 {
        return 1.0;
    }
    ((now - start).num_milliseconds() as f64 / total as f64).clamp(0.0, 1.0)
}

/// Unfinished jobs (active, paused or ready to deliver) of the given characters, soonest end first.
pub fn open_jobs(characters: &[std::sync::Arc<Character>], now: DateTime<Utc>) -> Vec<(&Character, &Job)> {
    let mut out: Vec<(&Character, &Job)> = characters
        .iter()
        .flat_map(|c| c.industry.iter().flat_map(|d| d.jobs.iter()).map(move |j| (c.as_ref(), j)))
        .filter(|(_, j)| job_state(j, now) != JobState::Closed)
        .collect();
    out.sort_by_key(|(_, j)| j.end_date);
    out
}

pub fn count_ready_jobs(characters: &[std::sync::Arc<Character>], now: DateTime<Utc>) -> usize {
    open_jobs(characters, now).iter().filter(|(_, j)| is_job_ready(j, now)).count()
}

/// The next job to finish that is still running.
pub fn next_completion<'a>(jobs: &[(&'a Character, &'a Job)], now: DateTime<Utc>) -> Option<(&'a Character, &'a Job)> {
    jobs.iter().copied().filter(|(_, j)| job_state(j, now) == JobState::Active).min_by_key(|(_, j)| j.end_date)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobCounts {
    pub open: usize,
    pub active: usize,
    pub ready: usize,
    pub paused: usize,
}

pub fn job_counts(jobs: &[(&Character, &Job)], now: DateTime<Utc>) -> JobCounts {
    let mut c = JobCounts { open: jobs.len(), ..Default::default() };
    for (_, j) in jobs {
        match job_state(j, now) {
            JobState::Ready => c.ready += 1,
            JobState::Active => c.active += 1,
            JobState::Paused => c.paused += 1,
            JobState::Closed => {}
        }
    }
    c
}

// ---------- job slots ----------

/// The three kinds of industry job slot a character has.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SlotKind {
    Manufacturing,
    /// Research, copying and invention share the science slots.
    Science,
    Reactions,
}

impl SlotKind {
    pub const ALL: [SlotKind; 3] = [SlotKind::Manufacturing, SlotKind::Science, SlotKind::Reactions];

    /// The slot an ESI job activity takes.
    pub fn of_activity(activity_id: i64) -> SlotKind {
        match activity_id {
            1 => SlotKind::Manufacturing,
            9 | 11 => SlotKind::Reactions,
            _ => SlotKind::Science,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SlotKind::Manufacturing => "Manufacturing",
            SlotKind::Science => "Science",
            SlotKind::Reactions => "Reactions",
        }
    }

    /// Position in [`SlotKind::ALL`] and in the `[i64; 3]` arrays below.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// How a character's slots are used right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SlotUsage {
    /// Jobs holding a slot (active, paused or ready to deliver), per [`SlotKind::index`].
    pub used: [i64; 3],
    pub ready: usize,
    /// When the next running job ends.
    pub next_end: Option<DateTime<Utc>>,
}

/// Slot use of one character's jobs. A finished job keeps its slot until it's delivered, so ready jobs count.
pub fn slot_usage(jobs: &[Job], now: DateTime<Utc>) -> SlotUsage {
    let mut u = SlotUsage::default();
    for j in jobs {
        let state = job_state(j, now);
        if state == JobState::Closed {
            continue;
        }
        u.used[SlotKind::of_activity(j.activity_id).index()] += 1;
        match state {
            JobState::Ready => u.ready += 1,
            JobState::Active => u.next_end = Some(u.next_end.map_or(j.end_date, |e| e.min(j.end_date))),
            _ => {}
        }
    }
    u
}

/// The Reactions skill: without it a character can't run reactions at all, whatever its base slot.
pub const REACTIONS_SKILL: i64 = 45746;

/// A character's slots per [`SlotKind::index`], from its skills; `None` until its skills have loaded.
pub fn character_slots(c: &Character) -> Option<[i64; 3]> {
    c.skills.as_ref()?;
    let levels = crate::features::manufacturing::levels_of(c);
    let (m, s, r) = crate::features::manufacturing::slots(&crate::blueprints::INDUSTRY, &levels);
    let r = if levels.get(&REACTIONS_SKILL).copied().unwrap_or(0) > 0 { r } else { 0 };
    Some([m, s, r])
}

// ---------- owned blueprints ----------

/// What an owned blueprint is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BlueprintKind {
    Original,
    Copy,
    /// A reaction formula (always an original: formulas can't be copied or researched).
    Formula,
}

impl BlueprintKind {
    pub const ALL: [BlueprintKind; 3] = [BlueprintKind::Original, BlueprintKind::Copy, BlueprintKind::Formula];

    pub fn label(self) -> &'static str {
        match self {
            BlueprintKind::Original => "Original",
            BlueprintKind::Copy => "Copy",
            BlueprintKind::Formula => "Formula",
        }
    }
}

pub fn blueprint_kind(data: &IndustryData, b: &OwnedBlueprint) -> BlueprintKind {
    if data.blueprint(b.type_id).is_some_and(|bp| bp.is_reaction()) {
        BlueprintKind::Formula
    } else if b.is_copy() {
        BlueprintKind::Copy
    } else {
        BlueprintKind::Original
    }
}

/// ME 10 and TE 20: research can't improve it further.
pub fn fully_researched(me: i64, te: i64) -> bool {
    me >= 10 && te >= 20
}

/// What a blueprint's product is, for filtering lists of blueprints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProductCategory {
    Ships,
    Modules,
    Charges,
    Drones,
    Subsystems,
    Components,
    Reactions,
    Structures,
    Implants,
    Other,
}

impl ProductCategory {
    pub const ALL: [ProductCategory; 10] = [
        ProductCategory::Ships,
        ProductCategory::Modules,
        ProductCategory::Charges,
        ProductCategory::Drones,
        ProductCategory::Subsystems,
        ProductCategory::Components,
        ProductCategory::Reactions,
        ProductCategory::Structures,
        ProductCategory::Implants,
        ProductCategory::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ProductCategory::Ships => "Ships",
            ProductCategory::Modules => "Modules",
            ProductCategory::Charges => "Charges",
            ProductCategory::Drones => "Drones & fighters",
            ProductCategory::Subsystems => "Subsystems",
            ProductCategory::Components => "Components",
            ProductCategory::Reactions => "Reactions",
            ProductCategory::Structures => "Structures & deployables",
            ProductCategory::Implants => "Implants & boosters",
            ProductCategory::Other => "Other",
        }
    }
}

/// The category of what `blueprint` makes (by the product's EVE category and group).
pub fn product_category(data: &IndustryData, blueprint: i64) -> ProductCategory {
    let Some(bp) = data.blueprint(blueprint) else { return ProductCategory::Other };
    if bp.is_reaction() {
        return ProductCategory::Reactions;
    }
    let Some((_, _, product)) = bp.output() else { return ProductCategory::Other };
    let group = data.get_type(product.type_id).and_then(|t| data.group_name(t.group)).unwrap_or("");
    match data.category_of(product.type_id) {
        Some(6) => ProductCategory::Ships,
        Some(7) => ProductCategory::Modules,
        Some(8) => ProductCategory::Charges,
        Some(18 | 87) => ProductCategory::Drones,
        Some(32) => ProductCategory::Subsystems,
        Some(20) => ProductCategory::Implants,
        Some(22 | 23 | 39 | 40 | 46 | 65 | 66) => ProductCategory::Structures,
        // Commodities hold the construction components and R.A.M. tools, but also mutaplasmids and filaments.
        Some(17) if group.contains("Component") || group == "Tool" => ProductCategory::Components,
        Some(4) if group == "Fuel Block" => ProductCategory::Components,
        _ => ProductCategory::Other,
    }
}

/// Tech level of a product, from its market meta group.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TechLevel {
    Tech1,
    Tech2,
    Tech3,
    /// Faction, storyline, officer and the like.
    Faction,
}

impl TechLevel {
    pub const ALL: [TechLevel; 4] = [TechLevel::Tech1, TechLevel::Tech2, TechLevel::Tech3, TechLevel::Faction];

    pub fn label(self) -> &'static str {
        match self {
            TechLevel::Tech1 => "Tech I",
            TechLevel::Tech2 => "Tech II",
            TechLevel::Tech3 => "Tech III",
            TechLevel::Faction => "Faction",
        }
    }

    pub fn short(self) -> &'static str {
        match self {
            TechLevel::Tech1 => "T1",
            TechLevel::Tech2 => "T2",
            TechLevel::Tech3 => "T3",
            TechLevel::Faction => "Faction",
        }
    }
}

pub fn tech_level(product: i64) -> TechLevel {
    match crate::sde::MARKET.get(product).and_then(|t| t.meta_group) {
        None | Some(1) => TechLevel::Tech1,
        Some(2) => TechLevel::Tech2,
        Some(14) => TechLevel::Tech3,
        Some(_) => TechLevel::Faction,
    }
}

/// Identical blueprints of one character in one place, as one row: ESI lists every copy as its own item, so 50
/// copies of the same print would otherwise be 50 rows. Originals already come stacked.
#[derive(Clone, Debug, PartialEq)]
pub struct BlueprintStack {
    pub character_id: i64,
    pub type_id: i64,
    pub kind: BlueprintKind,
    pub me: i64,
    pub te: i64,
    /// Runs per copy; -1 for originals and formulas.
    pub runs: i64,
    pub location_id: i64,
    pub location: Option<crate::universe::Place>,
    pub name: String,
    /// Blueprints in the stack.
    pub quantity: i64,
}

impl BlueprintStack {
    pub fn location_name(&self) -> String {
        self.location.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Unknown location".into())
    }
}

/// Every character's blueprints, identical ones in the same place merged, sorted by name.
pub fn blueprint_stacks(data: &IndustryData, characters: &[std::sync::Arc<Character>]) -> Vec<BlueprintStack> {
    // (character, type, kind, ME, TE, runs, location) → index in `out`
    type Key = (i64, i64, BlueprintKind, i64, i64, i64, i64);
    let mut by: HashMap<Key, usize> = HashMap::new();
    let mut out: Vec<BlueprintStack> = Vec::new();
    for c in characters {
        for b in c.blueprints.iter().flat_map(|d| &d.blueprints) {
            let kind = blueprint_kind(data, b);
            let runs = if kind == BlueprintKind::Copy { b.runs } else { -1 };
            // the location's place id, so blueprints in different containers of one station merge
            let location_id = b.location.as_ref().map_or(b.location_id, |p| p.id);
            let key = (c.id, b.type_id, kind, b.material_efficiency, b.time_efficiency, runs, location_id);
            match by.get(&key) {
                Some(&i) => out[i].quantity += b.count(),
                None => {
                    by.insert(key, out.len());
                    out.push(BlueprintStack {
                        character_id: c.id,
                        type_id: b.type_id,
                        kind,
                        me: b.material_efficiency,
                        te: b.time_efficiency,
                        runs,
                        location_id,
                        location: b.location.clone(),
                        name: b.name(),
                        quantity: b.count(),
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.kind.cmp(&b.kind)).then(b.me.cmp(&a.me)).then(a.character_id.cmp(&b.character_id)).then(b.runs.cmp(&a.runs)));
    out
}

/// Who owns a blueprint type and the best research among them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OwnedSummary {
    /// Character ids, in the order given.
    pub characters: Vec<i64>,
    pub originals: i64,
    pub copies: i64,
    /// Best (ME, TE) of any original or copy.
    pub best: Option<(i64, i64)>,
}

pub fn owned_summary(characters: &[std::sync::Arc<Character>]) -> HashMap<i64, OwnedSummary> {
    let mut out: HashMap<i64, OwnedSummary> = HashMap::new();
    for c in characters {
        for b in c.blueprints.iter().flat_map(|d| &d.blueprints) {
            let s = out.entry(b.type_id).or_default();
            if !s.characters.contains(&c.id) {
                s.characters.push(c.id);
            }
            if b.is_copy() {
                s.copies += 1;
            } else {
                s.originals += b.count();
            }
            let me_te = (b.material_efficiency, b.time_efficiency);
            s.best = Some(s.best.map_or(me_te, |best| best.max(me_te)));
        }
    }
    out
}

// ---------- planetary industry ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtractorState {
    Running,
    /// Its program ended: it needs restarting in-game.
    Expired,
    /// Placed but never given a program.
    Idle,
}

pub fn extractor_state(e: &Extractor, now: DateTime<Utc>) -> ExtractorState {
    match e.expiry_time {
        None => ExtractorState::Idle,
        Some(t) if t <= now => ExtractorState::Expired,
        Some(_) => ExtractorState::Running,
    }
}

pub fn is_extractor_expired(e: &Extractor, now: DateTime<Utc>) -> bool {
    extractor_state(e, now) == ExtractorState::Expired
}

pub fn extractor_progress(e: &Extractor, now: DateTime<Utc>) -> f64 {
    match (e.install_time, e.expiry_time) {
        (Some(s), Some(t)) => progress(s, t, now),
        _ => 0.0,
    }
}

/// Units per hour of a running extractor (from its current cycle's yield; real yields fall over the program).
pub fn extractor_per_hour(e: &Extractor) -> Option<f64> {
    let (q, t) = (e.qty_per_cycle?, e.cycle_time?);
    (t > 0).then(|| q as f64 * 3600.0 / t as f64)
}

/// The soonest expiry among a colony's running extractors.
pub fn colony_next_expiry(c: &Colony, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    c.extractors.iter().filter(|e| extractor_state(e, now) == ExtractorState::Running).filter_map(|e| e.expiry_time).min()
}

pub fn colony_needs_attention(c: &Colony, now: DateTime<Utc>) -> bool {
    c.extractors.iter().any(|e| is_extractor_expired(e, now))
}

/// Colonies of the given characters: those with expired extractors first, then by soonest expiry.
pub fn colonies(characters: &[std::sync::Arc<Character>], now: DateTime<Utc>) -> Vec<(&Character, &Colony)> {
    let mut out: Vec<(&Character, &Colony)> = characters
        .iter()
        .flat_map(|c| c.planets.iter().flat_map(|p| p.colonies.iter()).map(move |col| (c.as_ref(), col)))
        .collect();
    sort_colonies(&mut out, now);
    out
}

pub fn sort_colonies<T>(list: &mut [(T, &Colony)], now: DateTime<Utc>) {
    list.sort_by_key(|(_, c)| (!colony_needs_attention(c, now), colony_next_expiry(c, now).unwrap_or(DateTime::<Utc>::MAX_UTC)));
}

pub fn count_expired_extractors(characters: &[std::sync::Arc<Character>], now: DateTime<Utc>) -> usize {
    colonies(characters, now).iter().map(|(_, c)| c.extractors.iter().filter(|e| is_extractor_expired(e, now)).count()).sum()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExtractorCounts {
    pub total: usize,
    pub running: usize,
    pub expired: usize,
    pub idle: usize,
}

pub fn extractor_counts<'a>(colonies: impl IntoIterator<Item = &'a Colony>, now: DateTime<Utc>) -> ExtractorCounts {
    let mut n = ExtractorCounts::default();
    for e in colonies.into_iter().flat_map(|c| &c.extractors) {
        n.total += 1;
        match extractor_state(e, now) {
            ExtractorState::Running => n.running += 1,
            ExtractorState::Expired => n.expired += 1,
            ExtractorState::Idle => n.idle += 1,
        }
    }
    n
}

// ---------- assets ----------

/// A readable location flag: "CargoHold" → "Cargo Hold", "HiSlot3" → "High Slot".
pub fn flag_label(flag: &str) -> String {
    let base = flag.trim_end_matches(|c: char| c.is_ascii_digit());
    let known = match base {
        "Hangar" => Some("Hangar"),
        "Cargo" => Some("Cargo"),
        "HiSlot" => Some("High Slot"),
        "MedSlot" => Some("Mid Slot"),
        "LoSlot" => Some("Low Slot"),
        "RigSlot" => Some("Rig Slot"),
        "SubSystemSlot" => Some("Subsystem"),
        "DroneBay" => Some("Drone Bay"),
        "FighterBay" => Some("Fighter Bay"),
        "FighterTube" => Some("Fighter Tube"),
        "ShipHangar" => Some("Ship Hangar"),
        "FleetHangar" => Some("Fleet Hangar"),
        "Unlocked" | "Locked" => Some("Container"),
        "AssetSafety" => Some("Asset Safety"),
        "Deliveries" => Some("Deliveries"),
        "Implant" => Some("Implant"),
        "Skill" => Some("Skill"),
        "HangarAll" => Some("Hangar"),
        "AutoFit" => Some("Item Hangar"),
        _ => None,
    };
    if let Some(k) = known {
        return k.to_string();
    }
    let name = base.strip_prefix("Specialized").unwrap_or(base);
    // split CamelCase into words
    let mut out = String::new();
    let mut prev_lower = false;
    for ch in name.chars() {
        if prev_lower && ch.is_uppercase() {
            out.push(' ');
        }
        prev_lower = ch.is_lowercase();
        out.push(ch);
    }
    if out.is_empty() {
        flag.to_string()
    } else {
        out
    }
}

/// Lowercased search text, matched against an asset, the item it's in and its location (as the legacy app did:
/// a location or ship name matching shows everything in it).
pub fn asset_matches(query: &str, asset: &Asset, parent: Option<&Asset>, location: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    asset.label().to_lowercase().contains(query)
        || location.to_lowercase().contains(query)
        || parent.map(|p| p.label().to_lowercase().contains(query)).unwrap_or(false)
}

pub fn normalize_query(q: &str) -> String {
    q.trim().to_lowercase()
}

/// One location's matching items (indices into [`AssetsData::items`]), most valuable first.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetGroup {
    pub location_id: i64,
    pub name: String,
    pub items: Vec<usize>,
    pub value: f64,
}

/// Matching assets grouped by root location, most valuable location first.
pub fn group_assets(data: &AssetsData, query: &str) -> Vec<AssetGroup> {
    let by_id = data.by_id();
    let names: HashMap<i64, String> = data.items.iter().map(|a| a.root_location_id).map(|id| (id, data.location_name(id))).collect();
    let mut groups: HashMap<i64, AssetGroup> = HashMap::new();
    for (i, a) in data.items.iter().enumerate() {
        let location = &names[&a.root_location_id];
        let parent = a.parent_id.and_then(|p| by_id.get(&p).copied());
        if !asset_matches(query, a, parent, location) {
            continue;
        }
        let g = groups.entry(a.root_location_id).or_insert_with(|| AssetGroup {
            location_id: a.root_location_id,
            name: location.clone(),
            items: vec![],
            value: 0.0,
        });
        g.items.push(i);
        g.value += a.value();
    }
    let mut out: Vec<AssetGroup> = groups.into_values().collect();
    for g in &mut out {
        g.items.sort_by(|&x, &y| {
            let (a, b) = (&data.items[x], &data.items[y]);
            b.value().total_cmp(&a.value()).then_with(|| a.name.cmp(&b.name))
        });
    }
    out.sort_by(|a, b| b.value.total_cmp(&a.value).then_with(|| a.name.cmp(&b.name)));
    out
}

/// An asset found by [`search_assets`].
#[derive(Clone, Debug)]
pub struct AssetHit {
    pub character_id: i64,
    pub character_name: String,
    pub asset: Asset,
    pub location: String,
    pub parent: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct AssetSearch {
    pub hits: Vec<AssetHit>,
    /// Matches before `limit` cut the list.
    pub matches: usize,
    pub match_value: f64,
    /// Every loaded asset of every character.
    pub items: usize,
    pub total_value: f64,
    pub characters_loaded: usize,
}

/// Every character's assets matching `query` (already normalized), most valuable first, at most `limit` kept.
pub fn search_assets(characters: &[std::sync::Arc<Character>], query: &str, limit: usize) -> AssetSearch {
    let mut s = AssetSearch::default();
    let mut hits: Vec<AssetHit> = Vec::new();
    for c in characters {
        let Some(data) = &c.assets else { continue };
        s.characters_loaded += 1;
        let by_id = data.by_id();
        let name = c.name();
        for a in &data.items {
            s.items += 1;
            s.total_value += a.value();
            let location = data.location_name(a.root_location_id);
            let parent = a.parent_id.and_then(|p| by_id.get(&p).copied());
            if asset_matches(query, a, parent, &location) {
                s.matches += 1;
                s.match_value += a.value();
                hits.push(AssetHit { character_id: c.id, character_name: name.clone(), asset: a.clone(), location, parent: parent.map(Asset::label) });
            }
        }
    }
    hits.sort_by(|a, b| b.asset.value().total_cmp(&a.asset.value()).then_with(|| a.asset.name.cmp(&b.asset.name)));
    hits.truncate(limit);
    s.hits = hits;
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::assets::AssetsData;
    use crate::domains::industry::{IndustryData, PlanetsData};
    use crate::universe::Place;
    use chrono::Duration;
    use std::sync::Arc;

    fn t(s: &str) -> DateTime<Utc> {
        s.parse().unwrap()
    }

    fn job(id: i64, status: &str, start: &str, end: &str) -> Job {
        Job { job_id: id, activity_id: 1, status: status.into(), start_date: t(start), end_date: t(end), runs: 1, ..Default::default() }
    }

    #[test]
    fn job_states() {
        let now = t("2026-01-01T12:00:00Z");
        assert_eq!(job_state(&job(1, "active", "2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"), now), JobState::Ready);
        assert_eq!(job_state(&job(1, "active", "2026-01-01T10:00:00Z", "2026-01-01T14:00:00Z"), now), JobState::Active);
        assert_eq!(job_state(&job(1, "ready", "2026-01-01T10:00:00Z", "2026-01-01T14:00:00Z"), now), JobState::Ready);
        assert_eq!(job_state(&job(1, "delivered", "2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"), now), JobState::Closed);
        assert!((job_progress(&job(1, "active", "2026-01-01T10:00:00Z", "2026-01-01T14:00:00Z"), now) - 0.5).abs() < 1e-9);
        let mut paused = job(1, "paused", "2026-01-01T10:00:00Z", "2026-01-01T14:00:00Z");
        paused.pause_date = Some(t("2026-01-01T11:00:00Z"));
        assert!((job_progress(&paused, now) - 0.25).abs() < 1e-9);
        assert_eq!(activity_name(9), "Reactions");
        assert_eq!(activity_name(42), "Activity 42");
    }

    fn character(id: i64, jobs: Vec<Job>, colonies: Vec<Colony>) -> Arc<Character> {
        let mut c = Character::new(id);
        c.industry = Some(IndustryData { jobs });
        c.planets = Some(PlanetsData { colonies });
        Arc::new(c)
    }

    #[test]
    fn open_jobs_and_counts() {
        let now = t("2026-01-01T12:00:00Z");
        let chars = vec![
            character(1, vec![job(1, "active", "2026-01-01T10:00:00Z", "2026-01-01T15:00:00Z"), job(2, "delivered", "2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z")], vec![]),
            character(2, vec![job(3, "active", "2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"), job(4, "paused", "2026-01-01T10:00:00Z", "2026-01-01T13:00:00Z")], vec![]),
        ];
        let open = open_jobs(&chars, now);
        assert_eq!(open.iter().map(|(_, j)| j.job_id).collect::<Vec<_>>(), vec![3, 4, 1]);
        assert_eq!(count_ready_jobs(&chars, now), 1);
        assert_eq!(next_completion(&open, now).map(|(c, j)| (c.id, j.job_id)), Some((1, 1)));
        assert_eq!(job_counts(&open, now), JobCounts { open: 3, active: 1, ready: 1, paused: 1 });
    }

    fn extractor(install: Option<&str>, expiry: Option<&str>) -> Extractor {
        Extractor { install_time: install.map(t), expiry_time: expiry.map(t), qty_per_cycle: Some(1000), cycle_time: Some(1800), ..Default::default() }
    }

    fn colony(id: i64, extractors: Vec<Extractor>) -> Colony {
        Colony { planet_id: id, extractors, last_update: t("2026-01-01T00:00:00Z"), ..Default::default() }
    }

    #[test]
    fn extractors_and_colonies() {
        let now = t("2026-01-01T12:00:00Z");
        let running = extractor(Some("2026-01-01T00:00:00Z"), Some("2026-01-02T00:00:00Z"));
        let soon = extractor(Some("2026-01-01T00:00:00Z"), Some("2026-01-01T13:00:00Z"));
        let expired = extractor(Some("2026-01-01T00:00:00Z"), Some("2026-01-01T06:00:00Z"));
        let idle = extractor(None, None);
        assert_eq!(extractor_state(&running, now), ExtractorState::Running);
        assert_eq!(extractor_state(&expired, now), ExtractorState::Expired);
        assert_eq!(extractor_state(&idle, now), ExtractorState::Idle);
        assert!((extractor_progress(&running, now) - 0.5).abs() < 1e-9);
        assert_eq!(extractor_progress(&idle, now), 0.0);
        assert_eq!(extractor_per_hour(&running), Some(2000.0));

        let chars = vec![
            character(1, vec![], vec![colony(10, vec![running.clone()]), colony(11, vec![idle.clone()])]),
            character(2, vec![], vec![colony(20, vec![soon.clone(), running.clone()]), colony(21, vec![expired.clone(), running.clone()])]),
        ];
        let cols = colonies(&chars, now);
        assert_eq!(cols.iter().map(|(_, c)| c.planet_id).collect::<Vec<_>>(), vec![21, 20, 10, 11]);
        assert_eq!(colony_next_expiry(cols[1].1, now), soon.expiry_time);
        assert_eq!(count_expired_extractors(&chars, now), 1);
        let n = extractor_counts(cols.iter().map(|(_, c)| *c), now);
        assert_eq!(n, ExtractorCounts { total: 6, running: 4, expired: 1, idle: 1 });
        assert_eq!(count_expired_extractors(&chars, now + Duration::days(2)), 5);
    }

    fn asset(item_id: i64, name: &str, parent: Option<i64>, root: i64, qty: i64, price: f64) -> Asset {
        Asset {
            item_id,
            type_id: item_id,
            name: name.into(),
            quantity: qty,
            parent_id: parent,
            root_location_id: root,
            unit_price: Some(price),
            ..Default::default()
        }
    }

    fn assets() -> AssetsData {
        let mut d = AssetsData { items: vec![
            asset(1, "Orca", None, 60003760, 1, 1e9),
            asset(2, "Tritanium", Some(1), 60003760, 1000, 4.0),
            asset(3, "Rifter", None, 30000142, 1, 5e5),
            asset(4, "Veldspar", None, 60008494, 10, 1.0),
        ], ..Default::default() };
        d.items[0].custom_name = Some("Mothership".into());
        d.locations.insert(60003760, Place { id: 60003760, name: "Jita IV - Moon 4".into(), ..Default::default() });
        d.locations.insert(30000142, Place { id: 30000142, name: "Jita".into(), ..Default::default() });
        d
    }

    #[test]
    fn groups_and_search() {
        let d = assets();
        let all = group_assets(&d, "");
        assert_eq!(all.iter().map(|g| g.location_id).collect::<Vec<_>>(), vec![60003760, 30000142, 60008494]);
        assert_eq!(all[0].items, vec![0, 1]);
        assert_eq!(all[0].value, 1e9 + 4000.0);
        assert_eq!(all[2].name, "Location #60008494");

        // a ship's custom name finds what's in it
        let in_ship = group_assets(&d, "mothership");
        assert_eq!(in_ship.len(), 1);
        assert_eq!(in_ship[0].items.len(), 2);
        // a location name finds everything there ("jita" matches both Jita locations)
        assert_eq!(group_assets(&d, "jita").iter().map(|g| g.items.len()).sum::<usize>(), 3);
        assert!(group_assets(&d, "nothing").is_empty());

        let mut c = Character::new(7);
        c.assets = Some(d);
        let chars = vec![Arc::new(c)];
        let s = search_assets(&chars, &normalize_query("  TRIT "), 10);
        assert_eq!(s.matches, 1);
        assert_eq!(s.hits[0].parent.as_deref(), Some("Mothership (Orca)"));
        assert_eq!(s.items, 4);
        let top = search_assets(&chars, "", 2);
        assert_eq!(top.hits.len(), 2);
        assert_eq!(top.matches, 4);
        assert_eq!(top.hits[0].asset.name, "Orca");
    }

    #[test]
    fn slots_in_use() {
        let now = t("2026-01-01T12:00:00Z");
        let mut jobs = vec![
            job(1, "active", "2026-01-01T10:00:00Z", "2026-01-01T15:00:00Z"),
            job(2, "active", "2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"),
            job(3, "paused", "2026-01-01T10:00:00Z", "2026-01-01T13:00:00Z"),
            job(4, "active", "2026-01-01T10:00:00Z", "2026-01-01T14:00:00Z"),
            job(5, "delivered", "2026-01-01T10:00:00Z", "2026-01-01T11:00:00Z"),
            job(6, "active", "2026-01-01T10:00:00Z", "2026-01-01T13:30:00Z"),
        ];
        jobs[2].activity_id = 8;
        jobs[3].activity_id = 11;
        jobs[5].activity_id = 4;
        let u = slot_usage(&jobs, now);
        assert_eq!(u.used, [2, 2, 1]);
        assert_eq!(u.ready, 1);
        assert_eq!(u.next_end, Some(t("2026-01-01T13:30:00Z")));
        assert_eq!(slot_usage(&[], now), SlotUsage::default());
        assert_eq!(SlotKind::of_activity(9), SlotKind::Reactions);
        assert_eq!(SlotKind::of_activity(5), SlotKind::Science);

        use crate::domains::skills::{Skill, SkillsData};
        let skill = |id: i64, level: u8| Skill { skill_id: id, trained_skill_level: level, active_skill_level: level, ..Default::default() };
        let mut c = Character::new(1);
        assert_eq!(character_slots(&c), None);
        // Mass Production V: 1 + 5 manufacturing slots; no Reactions skill, no reaction slot
        c.skills = Some(SkillsData { skills: vec![skill(3387, 5)], ..Default::default() });
        assert_eq!(character_slots(&c), Some([6, 1, 0]));
        c.skills.as_mut().unwrap().skills.push(skill(REACTIONS_SKILL, 1));
        assert_eq!(character_slots(&c).map(|s| s[2]), Some(1));
    }

    fn owned(item: i64, type_id: i64, quantity: i64, runs: i64, me: i64, location: i64) -> OwnedBlueprint {
        OwnedBlueprint { item_id: item, type_id, location_id: location, material_efficiency: me, time_efficiency: 0, quantity, runs, ..Default::default() }
    }

    #[test]
    fn owned_blueprints() {
        let data = &*crate::blueprints::INDUSTRY;
        let formula = data.maker(16672).unwrap().0.id;
        let rifter = 691;
        assert_eq!(product_category(data, rifter), ProductCategory::Ships);
        assert_eq!(product_category(data, formula), ProductCategory::Reactions);
        assert_eq!(product_category(data, -1), ProductCategory::Other);
        assert_eq!(tech_level(587), TechLevel::Tech1);
        assert!(fully_researched(10, 20) && !fully_researched(10, 18));

        let mut a = Character::new(1);
        a.blueprints = Some(crate::domains::industry::BlueprintsData {
            blueprints: vec![
                owned(1, rifter, 3, -1, 10, 100),
                owned(2, rifter, -2, 10, 8, 100),
                owned(3, rifter, -2, 10, 8, 100),
                owned(4, rifter, -2, 5, 8, 100),
                owned(5, formula, -1, -1, 0, 100),
            ],
        });
        let mut b = Character::new(2);
        b.blueprints = Some(crate::domains::industry::BlueprintsData { blueprints: vec![owned(6, rifter, -2, 10, 8, 100)] });
        let chars = vec![Arc::new(a), Arc::new(b)];
        assert_eq!(blueprint_kind(data, &chars[0].blueprints.as_ref().unwrap().blueprints[4]), BlueprintKind::Formula);

        let stacks = blueprint_stacks(data, &chars);
        let rows: Vec<(i64, BlueprintKind, i64, i64)> =
            stacks.iter().filter(|s| s.type_id == rifter).map(|s| (s.character_id, s.kind, s.runs, s.quantity)).collect();
        // the stack of 3 originals stays one row; the two identical copies merge; other runs and characters don't
        assert_eq!(
            rows,
            vec![(1, BlueprintKind::Original, -1, 3), (1, BlueprintKind::Copy, 10, 2), (1, BlueprintKind::Copy, 5, 1), (2, BlueprintKind::Copy, 10, 1)]
        );
        assert_eq!(stacks.iter().filter(|s| s.kind == BlueprintKind::Formula).count(), 1);

        let owners = owned_summary(&chars);
        let r = &owners[&rifter];
        assert_eq!(r.characters, vec![1, 2]);
        assert_eq!((r.originals, r.copies), (3, 4));
        assert_eq!(r.best, Some((10, 0)));
    }

    #[test]
    fn flags() {
        assert_eq!(flag_label("HiSlot3"), "High Slot");
        assert_eq!(flag_label("SpecializedOreHold"), "Ore Hold");
        assert_eq!(flag_label("CorpSAG2"), "Corp SAG");
        assert_eq!(flag_label("Hangar"), "Hangar");
        assert_eq!(flag_label(""), "");
    }
}
