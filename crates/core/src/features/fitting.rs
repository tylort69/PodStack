//! Ship fittings and the skills to fly them (fit planner, ship browser).
//!
//! - [`parse_fits`] reads EFT (EVE's fitting window "Copy to clipboard" and Pyfa exports), EVE's XML fitting export
//!   (several fits per file) and DNA strings (`587:2048;1:31718;1::`).
//! - [`look_up`] resolves the names to types through ESI and reads each type's required skills from its dogma
//!   attributes ([`required_skills`]).
//! - [`analyse`] cross-references that with a character and builds the plan to fly it: the hull first, then the
//!   rest of the fit one unlock at a time, quickest first.
//! - [`level_time`] / [`plan_levels`] price individual skill levels for a character (skill and ship browsers).

use crate::character::Character;
use crate::sde::{self, SKILLS};
use crate::training::{self, AttributeSet};
use crate::universe::TypeInfo;
use crate::Core;
use chrono::Duration;
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

/// Dogma attribute ids for a type's required skills, as (skill attribute, level attribute) pairs.
pub const REQUIRED_SKILL_ATTRIBUTES: [(i64, i64); 6] = [(182, 277), (183, 278), (184, 279), (1285, 1286), (1289, 1287), (1290, 1288)];

/// Inventory category of a type, for display.
pub fn category_name(category_id: i64) -> &'static str {
    match category_id {
        6 => "Ship",
        7 => "Module",
        8 => "Charge",
        18 => "Drone",
        20 => "Implant",
        32 => "Subsystem",
        87 => "Fighter",
        _ => "Item",
    }
}

/// How a fit names an item: by name (EFT, XML) or type id (DNA).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ItemRef {
    Name(String),
    TypeId(i64),
}

impl ItemRef {
    pub fn label(&self) -> String {
        match self {
            ItemRef::Name(n) => n.clone(),
            ItemRef::TypeId(id) => format!("#{id}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FitItem {
    pub item: ItemRef,
    pub quantity: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedFit {
    /// "EFT", "XML" or "DNA"
    pub format: &'static str,
    /// None for DNA, which carries no name (the ship's name is used once looked up).
    pub fit_name: Option<String>,
    pub ship: ItemRef,
    pub items: Vec<FitItem>,
}

/// Adds `quantity` of `item` to a list, keeping the order items first appear in.
fn add_item(items: &mut Vec<FitItem>, item: ItemRef, quantity: u32) {
    match items.iter_mut().find(|i| i.item == item) {
        Some(i) => i.quantity += quantity,
        None => items.push(FitItem { item, quantity }),
    }
}

/// Parses one or more fits, detecting the format. Errors are messages for the user.
pub fn parse_fits(text: &str) -> Result<Vec<ParsedFit>, String> {
    let text = text.trim().trim_start_matches('\u{feff}');
    if text.is_empty() {
        return Err("Paste a fit first.".into());
    }
    // in-game fitting links (<url=fitting:...>) also start with '<' but hold DNA
    if text.starts_with('<') && text.contains("<fitting") {
        return parse_xml(text);
    }
    if text.starts_with('[') && text.lines().next().is_some_and(|l| l.contains(']')) {
        return parse_eft(text).map(|f| vec![f]);
    }
    static DNA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+(?::\d+_?(?:;\d+)?)*)::").unwrap());
    if let Some(m) = DNA.captures(text) {
        return Ok(vec![parse_dna(&m[1])]);
    }
    Err("Couldn't recognise that fit. Paste EFT text (first line [Ship, Fit name]), EVE fitting XML, or a DNA string.".into())
}

pub fn parse_eft(text: &str) -> Result<ParsedFit, String> {
    static HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[([^,\]]+)(?:,\s*(.*))?\]$").unwrap());
    static MUTATED_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[\d+\]\s").unwrap());
    static MUTATED_MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\[\d+\]$").unwrap());
    static OFFLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*/offline$").unwrap());
    static STACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(.*?)\s+x(\d+)$").unwrap());

    let mut lines = text.lines().map(str::trim);
    let first = lines.next().unwrap_or_default();
    let header = HEADER
        .captures(first)
        .ok_or_else(|| "That doesn't look like a fit. The first line should be [Ship name, Fit name].".to_string())?;
    let ship = header[1].trim().to_string();
    let fit_name = header.get(2).map(|m| m.as_str().trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| ship.clone());

    let mut items = Vec::new();
    for line in lines {
        // Pyfa lists mutated modules' stats in blocks starting "[1] Module name" at the end; nothing after is fit
        if MUTATED_BLOCK.is_match(line) {
            break;
        }
        // blank lines separate sections; [Empty Low slot] etc. are placeholders
        if line.is_empty() || (line.starts_with('[') && line.ends_with(']')) {
            continue;
        }
        // "Module, Charge"; offline modules end in "/OFFLINE"; Pyfa marks mutated modules "Module [1]"
        let line = OFFLINE.replace(line, "");
        for part in line.split(',') {
            let part = MUTATED_MARK.replace(part.trim(), "").to_string();
            let (name, quantity) = match STACK.captures(&part) {
                Some(c) => (c[1].trim().to_string(), c[2].parse().unwrap_or(1)),
                None => (part.clone(), 1),
            };
            if !name.is_empty() {
                add_item(&mut items, ItemRef::Name(name), quantity);
            }
        }
    }
    Ok(ParsedFit { format: "EFT", fit_name: Some(fit_name), ship: ItemRef::Name(ship), items })
}

fn xml_unescape(s: &str) -> String {
    s.replace("&quot;", "\"").replace("&apos;", "'").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// Attributes of one XML tag (`name="value"` pairs).
fn xml_attrs(tag: &str) -> HashMap<String, String> {
    static ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"([A-Za-z_:][\w:.-]*)\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());
    ATTR.captures_iter(tag)
        .map(|c| (c[1].to_string(), xml_unescape(c.get(2).or(c.get(3)).map(|m| m.as_str()).unwrap_or_default())))
        .collect()
}

/// EVE's XML fitting export: `<fittings><fitting name=".."><shipType value=".."/><hardware type=".." qty=".."/>`.
pub fn parse_xml(text: &str) -> Result<Vec<ParsedFit>, String> {
    static FITTING: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<fitting\b([^>]*)>(.*?)</fitting>").unwrap());
    static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(shipType|hardware)\b([^>]*)/?>").unwrap());

    let mut fits = Vec::new();
    for f in FITTING.captures_iter(text) {
        let name = xml_attrs(&f[1]).remove("name").filter(|n| !n.trim().is_empty());
        let mut ship = None;
        let mut items = Vec::new();
        for t in TAG.captures_iter(&f[2]) {
            let mut attrs = xml_attrs(&t[2]);
            if &t[1] == "shipType" {
                ship = attrs.remove("value").filter(|v| !v.trim().is_empty());
            } else if let Some(ty) = attrs.remove("type").filter(|v| !v.trim().is_empty()) {
                let qty = attrs.get("qty").and_then(|q| q.trim().parse().ok()).filter(|q| *q > 0).unwrap_or(1);
                add_item(&mut items, ItemRef::Name(ty.trim().to_string()), qty);
            }
        }
        let ship = ship.ok_or_else(|| "A fitting in that XML has no ship type.".to_string())?;
        fits.push(ParsedFit {
            format: "XML",
            fit_name: Some(name.map(|n| n.trim().to_string()).unwrap_or_else(|| ship.clone())),
            ship: ItemRef::Name(ship.trim().to_string()),
            items,
        });
    }
    if fits.is_empty() {
        return Err("Couldn't read that XML. Expected an EVE fitting export (<fittings><fitting>…).".into());
    }
    Ok(fits)
}

/// A DNA string without its trailing "::", e.g. "587:2048;1:31718;1".
pub fn parse_dna(dna: &str) -> ParsedFit {
    let mut parts = dna.split(':');
    let ship = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut items = Vec::new();
    for part in parts.filter(|p| !p.is_empty()) {
        let part = part.replace('_', "");
        let mut it = part.split(';');
        let Some(id) = it.next().and_then(|i| i.parse().ok()) else { continue };
        let qty = it.next().and_then(|q| q.parse().ok()).filter(|q| *q > 0).unwrap_or(1);
        add_item(&mut items, ItemRef::TypeId(id), qty);
    }
    ParsedFit { format: "DNA", fit_name: None, ship: ItemRef::TypeId(ship), items }
}

/// The skills a type needs, from its dogma attributes (skills missing from the static data are left out).
pub fn required_skills(t: &TypeInfo) -> Vec<(i64, u8)> {
    REQUIRED_SKILL_ATTRIBUTES
        .iter()
        .filter_map(|&(skill_attr, level_attr)| {
            let id = t.attr(skill_attr)?.round() as i64;
            let level = t.attr(level_attr)?.round() as i64;
            if id == 0 || level <= 0 {
                return None;
            }
            if SKILLS.get(id).is_none() {
                tracing::warn!("{} requires unknown skill #{id}", t.name);
                return None;
            }
            Some((id, level.min(5) as u8))
        })
        .collect()
}

/// A fit item looked up.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedItem {
    pub type_id: i64,
    pub name: String,
    /// "Ship", "Module", "Charge", ...
    pub category: String,
    pub quantity: u32,
    pub skills: Vec<(i64, u8)>,
}

/// A fit with every item looked up. `items[0]` is the ship.
#[derive(Clone, Debug, PartialEq)]
pub struct Requirements {
    pub ship_id: i64,
    pub ship_name: String,
    pub fit_name: String,
    pub items: Vec<ResolvedItem>,
    /// Names / ids that couldn't be found, skipped.
    pub unknown: Vec<String>,
}

#[derive(Deserialize, Default)]
struct IdsResponse {
    #[serde(default)]
    inventory_types: Vec<IdEntry>,
}

#[derive(Deserialize)]
struct IdEntry {
    id: i64,
    name: String,
}

/// Looks up the ship and every item in a parsed fit, with the skills each needs. Errors are messages for the user.
pub async fn look_up(core: &Core, fit: &ParsedFit) -> Result<Requirements, String> {
    let entries: Vec<(&ItemRef, u32, bool)> =
        std::iter::once((&fit.ship, 1, true)).chain(fit.items.iter().map(|i| (&i.item, i.quantity, false))).collect();

    // names -> type ids, 500 per request
    let names: Vec<String> = entries
        .iter()
        .filter_map(|(r, _, _)| match r {
            ItemRef::Name(n) => Some(n.clone()),
            _ => None,
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let mut ids: HashMap<String, i64> = HashMap::new();
    for chunk in names.chunks(500) {
        match core.esi.post("universe/ids").body(chunk).json::<IdsResponse>().await {
            Ok(res) => ids.extend(res.inventory_types.into_iter().map(|t| (t.name.to_lowercase(), t.id))),
            Err(e) if e.is_not_found() => {}
            Err(e) => {
                tracing::warn!("fit lookup: universe/ids failed: {e}");
                return Err("Couldn't reach EVE to look up the fit. Try again in a moment.".into());
            }
        }
    }

    let type_ids: Vec<Option<i64>> = entries
        .iter()
        .map(|(r, _, _)| match r {
            ItemRef::Name(n) => ids.get(&n.to_lowercase()).copied(),
            ItemRef::TypeId(id) => Some(*id).filter(|i| *i > 0),
        })
        .collect();
    let lookups = type_ids.iter().map(|id| async move {
        match id {
            Some(id) => match core.universe.type_info(&core.esi, *id).await {
                Ok(t) => Ok(Some(t)),
                // an unknown id in a DNA string 404s; anything else is a connection problem
                Err(e) if e.is_not_found() => Ok(None),
                Err(e) => Err(e),
            },
            None => Ok(None),
        }
    });
    let mut resolved = Vec::new();
    for r in futures::future::join_all(lookups).await {
        match r {
            Ok(t) => resolved.push(t),
            Err(e) => {
                tracing::warn!("fit lookup: type info failed: {e}");
                return Err("Couldn't reach EVE to look up the fit. Try again in a moment.".into());
            }
        }
    }
    core.universe.flush();
    build_requirements(fit, &entries, resolved)
}

fn build_requirements(fit: &ParsedFit, entries: &[(&ItemRef, u32, bool)], resolved: Vec<Option<TypeInfo>>) -> Result<Requirements, String> {
    if resolved.first().is_none_or(|t| t.is_none()) {
        return Err(format!("Couldn't find the ship \"{}\".", fit.ship.label()));
    }
    let mut items = Vec::new();
    let mut unknown = Vec::new();
    for ((r, qty, is_ship), t) in entries.iter().zip(resolved) {
        let Some(t) = t else {
            unknown.push(r.label());
            continue;
        };
        items.push(ResolvedItem {
            type_id: t.type_id,
            name: t.name.clone(),
            category: if *is_ship { "Ship".into() } else { category_name(t.category_id).into() },
            quantity: *qty,
            skills: required_skills(&t),
        });
    }
    let ship = &items[0];
    Ok(Requirements {
        ship_id: ship.type_id,
        ship_name: ship.name.clone(),
        fit_name: fit.fit_name.clone().unwrap_or_else(|| ship.name.clone()),
        unknown,
        items,
    })
}

/// Time for `c` to train one level of a skill, given what's already in it (zero when trained).
pub fn level_time(c: &Character, skill_id: i64, level: u8) -> Duration {
    let Some(info) = SKILLS.get(skill_id) else { return Duration::zero() };
    let have = c.skill(skill_id).map(|s| s.skillpoints_in_skill).unwrap_or(0);
    let sp = info.sp_for_level(level) - have.max(info.sp_for_level(level.saturating_sub(1)));
    training::time_for_sp(info, &AttributeSet::of(c), sp, c.is_omega().unwrap_or(true))
}

/// One skill level to train, with its training time.
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedLevel {
    pub skill_id: i64,
    pub level: u8,
    pub time: Duration,
}

impl PlannedLevel {
    pub fn name(&self) -> String {
        skill_name(self.skill_id)
    }
}

pub fn skill_name(id: i64) -> String {
    SKILLS.get(id).map(|s| s.name.clone()).unwrap_or_else(|| format!("Skill #{id}"))
}

/// What `c` still has to train for (skill, level) wants, prerequisites first, each level priced.
pub fn plan_levels(c: &Character, wanted: &[(i64, u8)]) -> Vec<PlannedLevel> {
    training::missing_levels(c, wanted).into_iter().map(|(id, l)| PlannedLevel { skill_id: id, level: l, time: level_time(c, id, l) }).collect()
}

pub fn total_time(levels: &[PlannedLevel]) -> Duration {
    levels.iter().fold(Duration::zero(), |t, l| t + l.time)
}

/// Whether `c` has every skill a ship needs at the level it needs.
pub fn can_fly(c: &Character, ship: &sde::ShipInfo) -> bool {
    ship.skills.iter().all(|r| c.trained_skill_level(r.id) >= r.level)
}

/// What `c` has to train to fly a ship.
pub fn ship_training(c: &Character, ship: &sde::ShipInfo) -> Vec<PlannedLevel> {
    let wanted: Vec<(i64, u8)> = ship.skills.iter().map(|r| (r.id, r.level)).collect();
    plan_levels(c, &wanted)
}

/// A direct requirement of an item the character lacks.
#[derive(Clone, Debug, PartialEq)]
pub struct MissingSkill {
    pub skill_id: i64,
    pub level: u8,
    pub trained: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ItemStatus {
    pub item: ResolvedItem,
    pub missing: Vec<MissingSkill>,
}

impl ItemStatus {
    pub fn usable(&self) -> bool {
        self.missing.is_empty()
    }
}

/// One step of a fit plan: the skills that unlock the next item(s).
#[derive(Clone, Debug, PartialEq)]
pub struct Milestone {
    /// "Fly the Rifter", "Damage Control I, 200mm AutoCannon I +2 more"
    pub label: String,
    /// The items this step unlocks.
    pub items: Vec<String>,
    pub levels: Vec<PlannedLevel>,
    /// Training time from the start of the plan until this step is done.
    pub reached: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Analysis {
    pub items: Vec<ItemStatus>,
    pub milestones: Vec<Milestone>,
    pub total: Duration,
}

impl Analysis {
    /// Every level to train, in plan order.
    pub fn queue(&self) -> Vec<PlannedLevel> {
        self.milestones.iter().flat_map(|m| m.levels.iter().cloned()).collect()
    }

    pub fn hull_usable(&self) -> bool {
        self.items.first().is_some_and(|i| i.usable())
    }

    /// When the hull can be flown (zero when it already can).
    pub fn hull_time(&self, ship_name: &str) -> Duration {
        self.milestones.iter().find(|m| m.items.iter().any(|i| i == ship_name)).map(|m| m.reached).unwrap_or_else(Duration::zero)
    }
}

/// Cross-references a fit against the character and builds the plan to fly it.
///
/// The plan gets the hull flyable first, then unlocks the rest of the fit one step at a time, always taking whichever
/// item is quickest to unlock next (items unlocked by the same skills are grouped). Within a step skills are ordered
/// fastest first with prerequisites kept ahead of the skills that need them.
pub fn analyse(c: &Character, req: &Requirements) -> Analysis {
    let trained = |id: i64| c.trained_skill_level(id);
    let items: Vec<ItemStatus> = req
        .items
        .iter()
        .map(|item| ItemStatus {
            item: item.clone(),
            missing: item
                .skills
                .iter()
                .filter(|(id, lvl)| trained(*id) < *lvl)
                .map(|&(id, lvl)| MissingSkill { skill_id: id, level: lvl, trained: trained(id) })
                .collect(),
        })
        .collect();

    // Skill levels (prerequisites included) still to train to use an item, given what's already planned.
    let needed = |item: &ResolvedItem, planned: &BTreeMap<i64, u8>| -> BTreeMap<i64, u8> {
        fn visit(id: i64, level: u8, trained: u8, planned: &BTreeMap<i64, u8>, levels: &mut BTreeMap<i64, u8>, c: &Character, depth: u8) {
            let have = trained.max(planned.get(&id).copied().unwrap_or(0)).max(levels.get(&id).copied().unwrap_or(0));
            if have >= level || depth > 20 {
                return;
            }
            levels.insert(id, level);
            for r in SKILLS.get(id).map(|s| s.required_skills.as_slice()).unwrap_or(&[]) {
                visit(r.id, r.level, c.trained_skill_level(r.id), planned, levels, c, depth + 1);
            }
        }
        let mut levels = BTreeMap::new();
        for &(id, lvl) in &item.skills {
            visit(id, lvl, trained(id), planned, &mut levels, c, 0);
        }
        levels
    };
    let mut time_cache: HashMap<(i64, u8), Duration> = HashMap::new();
    let mut time_of = |id: i64, l: u8| *time_cache.entry((id, l)).or_insert_with(|| level_time(c, id, l));
    let start_level = |id: i64, planned: &BTreeMap<i64, u8>| trained(id).max(planned.get(&id).copied().unwrap_or(0));

    // Greedy steps: hull first, then the cheapest remaining item each time.
    let mut planned: BTreeMap<i64, u8> = BTreeMap::new();
    let mut remaining: Vec<usize> = (0..items.len()).filter(|&i| !items[i].usable()).collect();
    let mut milestones = Vec::new();
    let mut elapsed = Duration::zero();
    while !remaining.is_empty() {
        let next = if remaining[0] == 0 {
            0
        } else {
            let mut best = (remaining[0], None::<Duration>);
            for &i in &remaining {
                let levels = needed(&items[i].item, &planned);
                let cost = levels.iter().fold(Duration::zero(), |t, (&id, &lvl)| {
                    (start_level(id, &planned) + 1..=lvl).fold(t, |t, l| t + time_of(id, l))
                });
                if best.1.is_none_or(|b| cost < b) {
                    best = (i, Some(cost));
                }
            }
            best.0
        };

        let levels = needed(&items[next].item, &planned);
        let mut step: Vec<PlannedLevel> = Vec::new();
        for (&id, &lvl) in &levels {
            for l in start_level(id, &planned) + 1..=lvl {
                step.push(PlannedLevel { skill_id: id, level: l, time: time_of(id, l) });
            }
        }
        for (&id, &lvl) in &levels {
            let e = planned.entry(id).or_insert(0);
            *e = (*e).max(lvl);
        }

        // anything else these skills happen to unlock comes with this step
        let unlocked: Vec<usize> = remaining.iter().copied().filter(|&i| i == next || needed(&items[i].item, &planned).is_empty()).collect();
        remaining.retain(|i| !unlocked.contains(i));
        let names: Vec<String> = unlocked.iter().map(|&i| items[i].item.name.clone()).collect();
        let label = if next == 0 {
            format!("Fly the {}", req.ship_name)
        } else if names.len() > 2 {
            format!("{} +{} more", names[..2].join(", "), names.len() - 2)
        } else {
            names.join(", ")
        };
        let levels = sort_section(step);
        elapsed += total_time(&levels);
        milestones.push(Milestone { label, items: names, levels, reached: elapsed });
    }

    Analysis { items, milestones, total: elapsed }
}

/// Orders one step's skill levels: always the quickest level whose prerequisites (lower levels of the same skill,
/// and prerequisite skills up to the level needed) are already placed.
pub fn sort_section(items: Vec<PlannedLevel>) -> Vec<PlannedLevel> {
    let blockers: Vec<Vec<usize>> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let prereqs = SKILLS.get(item.skill_id).map(|s| s.required_skills.as_slice()).unwrap_or(&[]);
            items
                .iter()
                .enumerate()
                .filter(|&(j, other)| {
                    j != i
                        && ((other.skill_id == item.skill_id && other.level < item.level)
                            || prereqs.iter().any(|r| r.id == other.skill_id && other.level <= r.level))
                })
                .map(|(j, _)| j)
                .collect()
        })
        .collect();
    let mut done = vec![false; items.len()];
    let mut out = Vec::with_capacity(items.len());
    while out.len() < items.len() {
        let next = (0..items.len())
            .filter(|&i| !done[i] && blockers[i].iter().all(|&j| done[j]))
            .min_by_key(|&i| items[i].time)
            // a cycle can't happen with real data; fall back to list order rather than loop forever
            .or_else(|| (0..items.len()).find(|&i| !done[i]))
            .unwrap();
        done[next] = true;
        out.push(items[next].clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::skills::{Attributes, Skill, SkillsData};
    use crate::universe::DogmaAttribute;

    const EFT: &str = "[Rifter, My Rifter]
Damage Control I
Gyrostabilizer I /OFFLINE
[Empty Low slot]

1MN Afterburner I

200mm AutoCannon I, EMP S
200mm AutoCannon I, EMP S

Hobgoblin I x5
EMP S x800

[1] 200mm AutoCannon I
  Damage 1.2";

    #[test]
    fn parses_eft() {
        let fits = parse_fits(EFT).unwrap();
        assert_eq!(fits.len(), 1);
        let f = &fits[0];
        assert_eq!(f.format, "EFT");
        assert_eq!(f.fit_name.as_deref(), Some("My Rifter"));
        assert_eq!(f.ship, ItemRef::Name("Rifter".into()));
        let q = |n: &str| f.items.iter().find(|i| i.item == ItemRef::Name(n.into())).map(|i| i.quantity);
        assert_eq!(q("Damage Control I"), Some(1));
        assert_eq!(q("Gyrostabilizer I"), Some(1));
        assert_eq!(q("200mm AutoCannon I"), Some(2));
        assert_eq!(q("EMP S"), Some(802));
        assert_eq!(q("Hobgoblin I"), Some(5));
        assert_eq!(q("[Empty Low slot]"), None);
        assert_eq!(f.items.len(), 6);
    }

    #[test]
    fn eft_without_fit_name_and_mutated_marks() {
        let f = parse_eft("[Vexor]\nDrone Damage Amplifier II [1]\n").unwrap();
        assert_eq!(f.fit_name.as_deref(), Some("Vexor"));
        assert_eq!(f.items, vec![FitItem { item: ItemRef::Name("Drone Damage Amplifier II".into()), quantity: 1 }]);
    }

    #[test]
    fn parses_xml() {
        let xml = r#"<?xml version="1.0" ?>
<fittings>
  <fitting name="Rat &amp; Run">
    <description value=""/>
    <shipType value="Rifter"/>
    <hardware flag="LoSlot0" type="Damage Control I"/>
    <hardware flag="HiSlot0" type="200mm AutoCannon I"/>
    <hardware flag="HiSlot1" type="200mm AutoCannon I"/>
    <hardware qty="5" flag="DroneBay" type="Hobgoblin I"/>
  </fitting>
  <fitting name="Second">
    <shipType value="Merlin"/>
  </fitting>
</fittings>"#;
        let fits = parse_fits(xml).unwrap();
        assert_eq!(fits.len(), 2);
        assert_eq!(fits[0].fit_name.as_deref(), Some("Rat & Run"));
        assert_eq!(fits[0].ship, ItemRef::Name("Rifter".into()));
        assert_eq!(fits[0].items.len(), 3);
        assert_eq!(fits[0].items[1].quantity, 2);
        assert_eq!(fits[0].items[2].quantity, 5);
        assert_eq!(fits[1].ship, ItemRef::Name("Merlin".into()));
        assert!(parse_fits("<fittings></fittings>").is_err());
    }

    #[test]
    fn parses_dna() {
        let fits = parse_fits("<url=fitting:587:2048;1:31718_;2:2488;3::>My fit</url>").unwrap();
        // a chat link starts with '<' but isn't an XML export
        assert_eq!(fits[0].format, "DNA");
        assert_eq!(fits[0].items.len(), 3);
        let f = parse_fits("587:2048;1:31718_;2:2488;3::").unwrap().remove(0);
        assert_eq!(f.format, "DNA");
        assert_eq!(f.ship, ItemRef::TypeId(587));
        assert_eq!(f.fit_name, None);
        assert_eq!(f.items.len(), 3);
        assert_eq!(f.items[1], FitItem { item: ItemRef::TypeId(31718), quantity: 2 });
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_fits("").is_err());
        assert!(parse_fits("hello world").is_err());
    }

    fn ty(id: i64, name: &str, attrs: &[(i64, f64)]) -> TypeInfo {
        TypeInfo {
            type_id: id,
            name: name.into(),
            dogma_attributes: attrs.iter().map(|&(a, v)| DogmaAttribute { attribute_id: a, value: v }).collect(),
            ..Default::default()
        }
    }

    fn skill_id(name: &str) -> i64 {
        SKILLS.by_name(name).unwrap().type_id
    }

    #[test]
    fn reads_required_skills() {
        let frig = skill_id("Minmatar Frigate") as f64;
        let gunnery = skill_id("Gunnery") as f64;
        let t = ty(587, "Rifter", &[(182, frig), (277, 1.0), (183, gunnery), (278, 2.0), (184, 999_999.0), (279, 1.0), (1285, 0.0), (1286, 0.0)]);
        assert_eq!(required_skills(&t), vec![(frig as i64, 1), (gunnery as i64, 2)]);
    }

    fn pilot(skills: &[(&str, u8)]) -> Character {
        let mut c = Character::new(1);
        c.attributes = Some(Attributes { charisma: 19, intelligence: 20, memory: 20, perception: 27, willpower: 21, ..Default::default() });
        c.skills = Some(SkillsData {
            skills: skills
                .iter()
                .map(|&(n, l)| {
                    let info = SKILLS.by_name(n).unwrap();
                    Skill { skill_id: info.type_id, skillpoints_in_skill: info.sp_for_level(l), trained_skill_level: l, active_skill_level: l, ..Default::default() }
                })
                .collect(),
            total_sp: 0,
            unallocated_sp: 0,
        });
        c
    }

    fn rifter_requirements() -> Requirements {
        let item = |id, name: &str, cat: &str, skills: Vec<(i64, u8)>| ResolvedItem { type_id: id, name: name.into(), category: cat.into(), quantity: 1, skills };
        Requirements {
            ship_id: 587,
            ship_name: "Rifter".into(),
            fit_name: "Test".into(),
            items: vec![
                item(587, "Rifter", "Ship", vec![(skill_id("Minmatar Frigate"), 1)]),
                item(2048, "Damage Control I", "Module", vec![(skill_id("Hull Upgrades"), 1)]),
                item(2873, "125mm Gatling AutoCannon I", "Module", vec![(skill_id("Small Projectile Turret"), 1)]),
                item(2488, "Hobgoblin I", "Drone", vec![(skill_id("Drones"), 1)]),
            ],
            unknown: vec![],
        }
    }

    #[test]
    fn analysis_puts_the_hull_first_and_prerequisites_ahead() {
        let c = pilot(&[("Spaceship Command", 1), ("Drones", 1)]);
        let a = analyse(&c, &rifter_requirements());
        assert!(!a.hull_usable());
        assert!(a.items[3].usable());
        assert_eq!(a.items.iter().filter(|i| i.usable()).count(), 1);
        assert_eq!(a.milestones[0].label, "Fly the Rifter");
        assert!(a.milestones[0].levels.iter().any(|l| l.skill_id == skill_id("Minmatar Frigate")));
        assert_eq!(a.hull_time("Rifter"), a.milestones[0].reached);
        // the drone was already usable, so the other two modules make up the remaining steps
        let unlocked: Vec<&String> = a.milestones.iter().skip(1).flat_map(|m| m.items.iter()).collect();
        assert_eq!(unlocked.len(), 2);

        // every level comes after its prerequisites and lower levels
        let queue = a.queue();
        for (i, l) in queue.iter().enumerate() {
            for r in &SKILLS.get(l.skill_id).unwrap().required_skills {
                if let Some(j) = queue.iter().position(|o| o.skill_id == r.id && o.level == r.level) {
                    assert!(j < i, "{} before its prerequisite", l.name());
                }
            }
            if l.level > 1 {
                if let Some(j) = queue.iter().position(|o| o.skill_id == l.skill_id && o.level == l.level - 1) {
                    assert!(j < i);
                }
            }
        }
        assert_eq!(a.total, total_time(&queue));
        assert!(a.total > Duration::zero());
    }

    #[test]
    fn analysis_of_a_flyable_fit_is_empty() {
        let c = pilot(&[
            ("Spaceship Command", 1),
            ("Minmatar Frigate", 1),
            ("Mechanics", 2),
            ("Hull Upgrades", 1),
            ("Gunnery", 1),
            ("Small Projectile Turret", 1),
            ("Drones", 1),
        ]);
        let a = analyse(&c, &rifter_requirements());
        assert!(a.hull_usable());
        assert!(a.milestones.is_empty());
        assert_eq!(a.total, Duration::zero());
    }

    #[test]
    fn level_times_count_partial_progress() {
        let c = pilot(&[("Gunnery", 1)]);
        let g = skill_id("Gunnery");
        assert_eq!(level_time(&c, g, 1), Duration::zero());
        assert!(level_time(&c, g, 2) > Duration::zero());
        let levels = plan_levels(&c, &[(g, 3)]);
        assert_eq!(levels.iter().map(|l| l.level).collect::<Vec<_>>(), vec![2, 3]);
    }

    #[test]
    fn ship_training_and_can_fly() {
        let rifter = sde::SHIPS.by_id.get(&587).unwrap();
        let c = pilot(&[]);
        assert!(!can_fly(&c, rifter));
        assert!(!ship_training(&c, rifter).is_empty());
    }

    #[test]
    fn requirements_need_the_ship() {
        let fit = parse_eft("[Nope]\nDamage Control I").unwrap();
        let entries: Vec<(&ItemRef, u32, bool)> = vec![(&fit.ship, 1, true), (&fit.items[0].item, 1, false)];
        assert!(build_requirements(&fit, &entries, vec![None, None]).is_err());
        let r = build_requirements(&fit, &entries, vec![Some(ty(1, "Nope", &[])), None]).unwrap();
        assert_eq!(r.unknown, vec!["Damage Control I".to_string()]);
        assert_eq!(r.fit_name, "Nope");
    }
}
