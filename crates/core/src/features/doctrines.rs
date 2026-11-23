//! Doctrines: groups (an alliance, a corporation, a fleet community), optionally with subgroups inside (SIGs such as
//! "Capitals" or "Incursions", as deep as wanted), each level with doctrines ("Home Defense", "Armor Cruisers"), each
//! with the fits it accepts. Who of the user's characters can fly what, what each is
//! missing, and the skill plan to get there.
//!
//! Fits are kept as EFT text (what EVE and Pyfa export and import) plus the items with the skills each needs, looked
//! up once when imported ([`resolve`]), so checking characters against them never waits on ESI. A whole group or
//! the whole document exports as PodStack JSON to share with corp mates; fits export as EFT.

use crate::character::Character;
use crate::features::fitting::{self, ItemRef, ParsedFit, PlannedLevel, Requirements, ResolvedItem};
use crate::sde::{ShipInfo, SHIPS, SKILLS};
use crate::{store, Core, Event};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

// ---------- data ----------

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DoctrineDoc {
    pub groups: Vec<Group>,
}

/// People the user flies with, and their doctrines. A group can hold subgroups (SIGs), which hold doctrines and
/// subgroups of their own; any level can have doctrines directly, so nesting is only there when wanted.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Group {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub groups: Vec<Group>,
    pub doctrines: Vec<Doctrine>,
}

impl Group {
    /// This group and every subgroup below it, each with its path of names from the top ("Init › Capitals").
    pub fn walk(&self) -> Vec<(Vec<&str>, &Group)> {
        fn go<'a>(g: &'a Group, path: &mut Vec<&'a str>, out: &mut Vec<(Vec<&'a str>, &'a Group)>) {
            path.push(&g.name);
            out.push((path.clone(), g));
            for sub in &g.groups {
                go(sub, path, out);
            }
            path.pop();
        }
        let mut out = Vec::new();
        go(self, &mut Vec::new(), &mut out);
        out
    }
    /// Every doctrine in this group and its subgroups.
    pub fn all_doctrines(&self) -> Vec<&Doctrine> {
        self.walk().into_iter().flat_map(|(_, g)| g.doctrines.iter()).collect()
    }
    fn find_mut(&mut self, id: &str) -> Option<&mut Group> {
        if self.id == id {
            return Some(self);
        }
        self.groups.iter_mut().find_map(|g| g.find_mut(id))
    }
    fn doctrines_mut(&mut self) -> Vec<&mut Doctrine> {
        let mut out: Vec<&mut Doctrine> = self.doctrines.iter_mut().collect();
        for g in &mut self.groups {
            out.extend(g.doctrines_mut());
        }
        out
    }
    /// Fresh ids for the group and everything in it (an imported copy).
    fn renew_ids(&mut self) {
        self.id = new_id();
        for d in &mut self.doctrines {
            d.id = new_id();
            for f in &mut d.fits {
                f.id = new_id();
            }
        }
        for g in &mut self.groups {
            g.renew_ids();
        }
    }
    pub fn fit_count(&self) -> usize {
        self.all_doctrines().iter().map(|d| d.fits.len()).sum()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Doctrine {
    pub id: String,
    pub name: String,
    pub notes: String,
    pub fits: Vec<DoctrineFit>,
}

/// A fit a doctrine accepts.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DoctrineFit {
    pub id: String,
    /// The fit's name ("Ishtar - Sentry").
    pub name: String,
    /// What it does in the fleet ("DPS", "Logistics", "Tackle", …), free text.
    pub role: String,
    pub notes: String,
    pub ship_id: i64,
    pub ship_name: String,
    /// The fit as EFT text.
    pub eft: String,
    /// The ship (first) and every item, with the skills each needs.
    pub items: Vec<FitItem>,
    /// Names in the fit that couldn't be found when it was imported.
    pub unknown: Vec<String>,
    pub added: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct FitItem {
    pub type_id: i64,
    pub name: String,
    /// "Ship", "Module", "Charge", "Drone", …
    pub category: String,
    pub quantity: u32,
    pub skills: Vec<(i64, u8)>,
}

impl DoctrineFit {
    /// The fit as the fitting planner's [`Requirements`].
    pub fn requirements(&self) -> Requirements {
        Requirements {
            ship_id: self.ship_id,
            ship_name: self.ship_name.clone(),
            fit_name: self.name.clone(),
            items: self
                .items
                .iter()
                .map(|i| ResolvedItem { type_id: i.type_id, name: i.name.clone(), category: i.category.clone(), quantity: i.quantity, skills: i.skills.clone() })
                .collect(),
            unknown: self.unknown.clone(),
        }
    }
    /// Skills the hull alone needs.
    pub fn hull_skills(&self) -> &[(i64, u8)] {
        self.items.first().map(|i| i.skills.as_slice()).unwrap_or(&[])
    }
    /// Every skill level the whole fit needs (highest level per skill).
    pub fn all_skills(&self) -> Vec<(i64, u8)> {
        let mut m: BTreeMap<i64, u8> = BTreeMap::new();
        for (id, l) in self.items.iter().flat_map(|i| i.skills.iter()) {
            let e = m.entry(*id).or_insert(0);
            *e = (*e).max(*l);
        }
        m.into_iter().collect()
    }
    pub fn ship(&self) -> Option<&'static ShipInfo> {
        SHIPS.by_id.get(&self.ship_id)
    }
}

pub const DOC: &str = "doctrines";

pub fn load() -> DoctrineDoc {
    store::load_doc(DOC)
}

pub fn save(core: &Core, d: &DoctrineDoc) {
    store::save_doc(DOC, d);
    core.emit(Event::Doc(DOC.into()));
}

/// A new unique id.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!("{:x}{:x}", Utc::now().timestamp_micros(), N.fetch_add(1, Ordering::Relaxed))
}

impl DoctrineDoc {
    /// Every group and subgroup, with its path of names from the top.
    pub fn all_groups(&self) -> Vec<(Vec<&str>, &Group)> {
        self.groups.iter().flat_map(|g| g.walk()).collect()
    }
    pub fn group(&self, id: &str) -> Option<&Group> {
        self.all_groups().into_iter().find(|(_, g)| g.id == id).map(|(_, g)| g)
    }
    pub fn group_mut(&mut self, id: &str) -> Option<&mut Group> {
        self.groups.iter_mut().find_map(|g| g.find_mut(id))
    }
    /// The group holding a group (None for a top-level one).
    pub fn parent_of(&self, id: &str) -> Option<&Group> {
        self.all_groups().into_iter().map(|(_, g)| g).find(|g| g.groups.iter().any(|s| s.id == id))
    }
    /// Removes a group (and everything in it) wherever it is.
    pub fn remove_group(&mut self, id: &str) -> Option<Group> {
        fn go(list: &mut Vec<Group>, id: &str) -> Option<Group> {
            if let Some(i) = list.iter().position(|g| g.id == id) {
                return Some(list.remove(i));
            }
            list.iter_mut().find_map(|g| go(&mut g.groups, id))
        }
        go(&mut self.groups, id)
    }
    /// (path of group names, group, doctrine) by doctrine id.
    pub fn doctrine(&self, id: &str) -> Option<(Vec<&str>, &Group, &Doctrine)> {
        self.all_groups().into_iter().find_map(|(p, g)| g.doctrines.iter().find(|d| d.id == id).map(|d| (p, g, d)))
    }
    pub fn doctrine_mut(&mut self, id: &str) -> Option<&mut Doctrine> {
        self.groups.iter_mut().flat_map(|g| g.doctrines_mut()).find(|d| d.id == id)
    }
    /// Removes a doctrine wherever it is.
    pub fn remove_doctrine(&mut self, id: &str) -> Option<Doctrine> {
        fn go(g: &mut Group, id: &str) -> Option<Doctrine> {
            if let Some(i) = g.doctrines.iter().position(|d| d.id == id) {
                return Some(g.doctrines.remove(i));
            }
            g.groups.iter_mut().find_map(|s| go(s, id))
        }
        self.groups.iter_mut().find_map(|g| go(g, id))
    }
    /// (path of group names, doctrine, fit) by fit id.
    pub fn fit(&self, id: &str) -> Option<(Vec<&str>, &Doctrine, &DoctrineFit)> {
        self.all_fits().into_iter().find(|(_, _, f)| f.id == id)
    }
    /// Every fit, with its group path and doctrine.
    pub fn all_fits(&self) -> Vec<(Vec<&str>, &Doctrine, &DoctrineFit)> {
        let mut out = Vec::new();
        for (p, g) in self.all_groups() {
            for d in &g.doctrines {
                out.extend(d.fits.iter().map(|f| (p.clone(), d, f)));
            }
        }
        out
    }
    /// Every doctrine, with its group path.
    pub fn all_doctrines(&self) -> Vec<(Vec<&str>, &Doctrine)> {
        self.all_groups().into_iter().flat_map(|(p, g)| g.doctrines.iter().map(move |d| (p.clone(), d))).collect()
    }
    pub fn fit_count(&self) -> usize {
        self.all_fits().len()
    }
}

// ---------- import ----------

/// Looks up a parsed fit's items (through ESI, cached) and makes it a doctrine fit. Errors are messages for the user.
pub async fn resolve(core: &Core, parsed: &ParsedFit, source: &str) -> Result<DoctrineFit, String> {
    let req = fitting::look_up(core, parsed).await?;
    Ok(from_requirements(&req, parsed, source))
}

fn from_requirements(req: &Requirements, parsed: &ParsedFit, source: &str) -> DoctrineFit {
    let items: Vec<FitItem> = req
        .items
        .iter()
        .map(|i| FitItem { type_id: i.type_id, name: i.name.clone(), category: i.category.clone(), quantity: i.quantity, skills: i.skills.clone() })
        .collect();
    let eft = if parsed.format == "EFT" { source.trim().to_string() } else { eft_from_items(&req.fit_name, &items) };
    DoctrineFit {
        id: new_id(),
        name: req.fit_name.clone(),
        role: String::new(),
        notes: String::new(),
        ship_id: req.ship_id,
        ship_name: req.ship_name.clone(),
        eft,
        items,
        unknown: req.unknown.clone(),
        added: Some(Utc::now()),
    }
}

/// EFT text for a fit known only by its items (imported as XML or DNA): modules first, then drones and fighters,
/// then charges and the rest as cargo. EVE and Pyfa place modules in their slots on import.
pub fn eft_from_items(name: &str, items: &[FitItem]) -> String {
    let Some(ship) = items.first() else { return String::new() };
    let mut out = vec![format!("[{}, {}]", ship.name, name)];
    let rest = &items[1..];
    let section = |cats: &[&str], stacked: bool| -> Vec<String> {
        rest.iter()
            .filter(|i| cats.contains(&i.category.as_str()))
            .flat_map(|i| {
                if stacked || i.quantity > 1 && !matches!(i.category.as_str(), "Module" | "Subsystem") {
                    vec![format!("{} x{}", i.name, i.quantity)]
                } else {
                    std::iter::repeat_n(i.name.clone(), i.quantity.max(1) as usize).collect()
                }
            })
            .collect()
    };
    for (cats, stacked) in [(&["Subsystem", "Module"][..], false), (&["Drone", "Fighter"][..], true)] {
        let lines = section(cats, stacked);
        if !lines.is_empty() {
            out.push(String::new());
            out.extend(lines);
        }
    }
    let other: Vec<String> = rest
        .iter()
        .filter(|i| !matches!(i.category.as_str(), "Subsystem" | "Module" | "Drone" | "Fighter"))
        .map(|i| format!("{} x{}", i.name, i.quantity.max(1)))
        .collect();
    if !other.is_empty() {
        out.push(String::new());
        out.extend(other);
    }
    out.join("\n")
}

/// Splits pasted text holding several EFT fits (each starting with a "[Ship, Name]" line) into one text per fit;
/// anything else (XML, DNA) comes back whole for [`fitting::parse_fits`].
pub fn split_fits(text: &str) -> Vec<String> {
    let text = text.trim().trim_start_matches('\u{feff}');
    if !text.starts_with('[') {
        return vec![text.to_string()];
    }
    let header = regex::Regex::new(r"^\[[^\]\d][^,\]]*(,[^\]]*)?\]$").unwrap();
    let mut out: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        // a new header, unless it's an empty-slot marker inside a fit
        let l = line.trim();
        if header.is_match(l) && !(l.starts_with("[Empty ") && l.ends_with(" slot]")) {
            out.push(vec![line]);
        } else if let Some(cur) = out.last_mut() {
            cur.push(line);
        }
    }
    out.into_iter().map(|l| l.join("\n").trim().to_string()).filter(|s| !s.is_empty()).collect()
}

/// Every fit in a doctrine as EFT, blank-line separated (Pyfa imports them all at once).
pub fn export_eft(fits: &[DoctrineFit]) -> String {
    fits.iter().map(|f| f.eft.trim().to_string()).collect::<Vec<_>>().join("\n\n")
}

/// What PodStack shares: groups with their doctrines and fits.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SharedDoc {
    format: String,
    version: u32,
    groups: Vec<Group>,
}

const SHARE_FORMAT: &str = "podstack-doctrines";

/// Groups as PodStack JSON, for a corp mate's PodStack.
pub fn export_json(groups: &[Group]) -> String {
    serde_json::to_string_pretty(&SharedDoc { format: SHARE_FORMAT.into(), version: 1, groups: groups.to_vec() }).unwrap_or_default()
}

/// Groups from PodStack JSON, with fresh ids so importing twice doesn't clash. Errors are messages for the user.
pub fn import_json(text: &str) -> Result<Vec<Group>, String> {
    let d: SharedDoc = serde_json::from_str(text.trim()).map_err(|_| "That isn't a PodStack doctrines file.".to_string())?;
    if d.format != SHARE_FORMAT {
        return Err("That isn't a PodStack doctrines file.".into());
    }
    Ok(d.groups
        .into_iter()
        .map(|mut g| {
            g.renew_ids();
            g
        })
        .collect())
}

/// Adds imported groups: a group with the same name as one already at that level gets merged into it (its
/// subgroups the same way, and a doctrine with the same name gets the fits it doesn't have yet, by name and hull).
/// Returns how many fits were added.
pub fn merge_groups(doc: &mut DoctrineDoc, groups: Vec<Group>) -> usize {
    merge_into(&mut doc.groups, groups)
}

fn merge_into(list: &mut Vec<Group>, incoming: Vec<Group>) -> usize {
    let mut added = 0;
    for g in incoming {
        let Some(mine) = list.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&g.name)) else {
            added += g.fit_count();
            list.push(g);
            continue;
        };
        for d in g.doctrines {
            match mine.doctrines.iter_mut().find(|x| x.name.eq_ignore_ascii_case(&d.name)) {
                Some(md) => {
                    for f in d.fits {
                        if !md.fits.iter().any(|x| x.name.eq_ignore_ascii_case(&f.name) && x.ship_id == f.ship_id) {
                            md.fits.push(f);
                            added += 1;
                        }
                    }
                }
                None => {
                    added += d.fits.len();
                    mine.doctrines.push(d);
                }
            }
        }
        added += merge_into(&mut mine.groups, g.groups);
    }
    added
}

// ---------- who can fly what ----------

/// How ready a character is for a fit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Readiness {
    /// Can't sit in the hull yet.
    Missing,
    /// Can fly the hull, but not every module (or drone, charge…) of the fit.
    HullOnly,
    /// Can fly the fit as it is.
    Ready,
}

impl Readiness {
    pub fn label(self) -> &'static str {
        match self {
            Readiness::Ready => "Can fly",
            Readiness::HullOnly => "Hull only",
            Readiness::Missing => "Can't fly",
        }
    }
}

/// A character against a fit.
#[derive(Clone, Debug, PartialEq)]
pub struct FitCheck {
    pub readiness: Readiness,
    /// Training left for the whole fit, and for the hull alone (zero when there).
    pub time_to_fit: Duration,
    pub time_to_hull: Duration,
    /// Items the character can't use yet.
    pub missing_items: Vec<String>,
}

pub fn check(c: &Character, fit: &DoctrineFit) -> FitCheck {
    let a = fitting::analyse(c, &fit.requirements());
    let readiness = if a.items.iter().all(|i| i.usable()) {
        Readiness::Ready
    } else if a.hull_usable() {
        Readiness::HullOnly
    } else {
        Readiness::Missing
    };
    FitCheck {
        readiness,
        time_to_fit: a.total,
        time_to_hull: a.hull_time(&fit.ship_name),
        missing_items: a.items.iter().filter(|i| !i.usable()).map(|i| i.item.name.clone()).collect(),
    }
}

/// The skill levels a character still needs for a fit (prerequisites first), in the fitting planner's order: the
/// hull first, then the rest quickest first.
pub fn plan_for(c: &Character, fit: &DoctrineFit) -> Vec<PlannedLevel> {
    fitting::analyse(c, &fit.requirements()).queue()
}

/// The skill levels a character needs for several fits together (a doctrine, or the fits it can't fly yet), each
/// level once.
pub fn plan_for_all(c: &Character, fits: &[&DoctrineFit]) -> Vec<PlannedLevel> {
    let mut wanted: BTreeMap<i64, u8> = BTreeMap::new();
    for f in fits {
        for (id, l) in f.all_skills() {
            let e = wanted.entry(id).or_insert(0);
            *e = (*e).max(l);
        }
    }
    fitting::plan_levels(c, &wanted.into_iter().collect::<Vec<_>>())
}

/// A plan as EVE's skill plan text (the Skills window's clipboard format, which its import reads back).
pub fn eve_plan_text(levels: &[PlannedLevel]) -> String {
    levels.iter().map(|l| format!("{} {}", fitting::skill_name(l.skill_id), l.level)).collect::<Vec<_>>().join("\r\n")
}

/// Every character against every fit of a doctrine: (character id, fit id) → check.
pub type Matrix = BTreeMap<(i64, String), FitCheck>;

pub fn matrix(chars: &[Arc<Character>], fits: &[&DoctrineFit]) -> Matrix {
    let mut m = Matrix::new();
    for c in chars.iter().filter(|c| c.skills.is_some()) {
        for f in fits {
            m.insert((c.id, f.id.clone()), check(c, f));
        }
    }
    m
}

/// Characters that can fly a doctrine: at least one of its fits as it is.
pub fn pilots_of(chars: &[Arc<Character>], doctrine: &Doctrine) -> Vec<i64> {
    chars
        .iter()
        .filter(|c| c.skills.is_some() && doctrine.fits.iter().any(|f| check(c, f).readiness == Readiness::Ready))
        .map(|c| c.id)
        .collect()
}

// ---------- hulls: who can fly which class ----------

/// Whether a character can sit in a hull (skills only).
pub fn can_fly_hull(c: &Character, ship: &ShipInfo) -> bool {
    fitting::can_fly(c, ship)
}

/// Pilots per ship class: class (CCP's group, e.g. "Dreadnought") → race → hull → characters who can fly it.
#[derive(Clone, Debug, Default)]
pub struct HullGrid {
    pub classes: BTreeMap<String, ClassPilots>,
}

#[derive(Clone, Debug, Default)]
pub struct ClassPilots {
    pub group_id: i64,
    /// Characters who can fly at least one hull of the class.
    pub pilots: BTreeSet<i64>,
    /// race name → characters who can fly at least one hull of the class of that race.
    pub by_race: BTreeMap<String, BTreeSet<i64>>,
    /// hull type id → characters who can fly it.
    pub by_hull: BTreeMap<i64, BTreeSet<i64>>,
}

/// Which hulls each character can fly, by class and race. `include` picks the hulls (e.g. Tech I and II only).
pub fn hull_grid(chars: &[Arc<Character>], include: impl Fn(&ShipInfo) -> bool) -> HullGrid {
    let mut g = HullGrid::default();
    for ship in SHIPS.by_id.values().filter(|s| include(s)) {
        let class = g.classes.entry(ship.group.clone()).or_default();
        class.group_id = ship.group_id;
        class.by_hull.entry(ship.type_id).or_default();
        let race = race_name(ship);
        for c in chars.iter().filter(|c| c.skills.is_some()) {
            if can_fly_hull(c, ship) {
                class.pilots.insert(c.id);
                class.by_race.entry(race.clone()).or_default().insert(c.id);
                class.by_hull.entry(ship.type_id).or_default().insert(c.id);
            }
        }
    }
    g
}

pub fn race_name(ship: &ShipInfo) -> String {
    ship.race_id.and_then(|r| SHIPS.races.get(&r).cloned()).unwrap_or_else(|| "Other".into())
}

/// A skill's name, or its id when unknown.
pub fn skill_label(id: i64, level: u8) -> String {
    format!("{} {}", SKILLS.get(id).map(|s| s.name.as_str()).unwrap_or("Unknown skill"), level)
}

/// The parsed fits in a paste, each with the text it came from. Errors are messages for the user.
pub fn parse_paste(text: &str) -> Result<Vec<(ParsedFit, String)>, String> {
    let mut out = Vec::new();
    for chunk in split_fits(text) {
        let fits = fitting::parse_fits(&chunk)?;
        let single = fits.len() == 1;
        for f in fits {
            // XML with several fits: each gets its own EFT once resolved
            out.push((f, if single { chunk.clone() } else { String::new() }));
        }
    }
    if out.is_empty() {
        return Err("Paste a fit first.".into());
    }
    Ok(out)
}

/// The ship a parsed fit names, for showing before it's resolved.
pub fn parsed_ship_label(f: &ParsedFit) -> String {
    match &f.ship {
        ItemRef::Name(n) => n.clone(),
        ItemRef::TypeId(id) => SHIPS.by_id.get(id).map(|s| s.name.clone()).unwrap_or_else(|| format!("#{id}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISHTAR: &str = "[Ishtar, Sentry]\nDrone Damage Amplifier II\n\nLarge Shield Extender II\n\nGarde II x5\n";
    const MULTI: &str = "[Ishtar, Sentry]\nDrone Damage Amplifier II\n[Empty Low slot]\n\n[Rifter, Tackle]\nWarp Disruptor II\n";

    fn item(id: i64, name: &str, cat: &str, q: u32, skills: &[(i64, u8)]) -> FitItem {
        FitItem { type_id: id, name: name.into(), category: cat.into(), quantity: q, skills: skills.to_vec() }
    }

    #[test]
    fn splits_several_eft_fits() {
        let parts = split_fits(MULTI);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].contains("[Empty Low slot]"));
        assert!(parts[1].starts_with("[Rifter, Tackle]"));
        let parsed = parse_paste(MULTI).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed_ship_label(&parsed[1].0), "Rifter");
        assert_eq!(split_fits(ISHTAR).len(), 1);
    }

    #[test]
    fn eft_from_items_puts_sections_in_order() {
        let items = vec![
            item(12005, "Ishtar", "Ship", 1, &[]),
            item(1, "Drone Damage Amplifier II", "Module", 2, &[]),
            item(2, "Garde II", "Drone", 5, &[]),
            item(3, "Nanite Repair Paste", "Charge", 50, &[]),
        ];
        let eft = eft_from_items("Sentry", &items);
        assert_eq!(eft, "[Ishtar, Sentry]\n\nDrone Damage Amplifier II\nDrone Damage Amplifier II\n\nGarde II x5\n\nNanite Repair Paste x50");
        assert_eq!(fitting::parse_eft(&eft).unwrap().items.len(), 3);
    }

    #[test]
    fn json_round_trip_and_merge() {
        let fit = DoctrineFit { id: "f".into(), name: "Sentry".into(), ship_id: 12005, ship_name: "Ishtar".into(), ..Default::default() };
        let g = Group {
            id: "g".into(),
            name: "Alliance".into(),
            doctrines: vec![Doctrine { id: "d".into(), name: "Home Defense".into(), fits: vec![fit.clone()], ..Default::default() }],
            ..Default::default()
        };
        let text = export_json(std::slice::from_ref(&g));
        let back = import_json(&text).unwrap();
        assert_eq!(back[0].doctrines[0].fits[0].name, "Sentry");
        assert_ne!(back[0].id, "g", "fresh ids");
        assert!(import_json("{}").is_err());

        let mut doc = DoctrineDoc { groups: vec![g] };
        // same group and doctrine: only the new fit is added
        let mut incoming = back.clone();
        incoming[0].doctrines[0].fits.push(DoctrineFit { name: "Logi".into(), ship_id: 11985, ..Default::default() });
        assert_eq!(merge_groups(&mut doc, incoming), 1);
        assert_eq!(doc.groups.len(), 1);
        assert_eq!(doc.groups[0].doctrines[0].fits.len(), 2);
        // a new group comes in whole
        let mut other = back;
        other[0].name = "Corp".into();
        assert_eq!(merge_groups(&mut doc, other), 1);
        assert_eq!(doc.groups.len(), 2);
    }

    #[test]
    fn nested_groups_are_optional() {
        let fit = |n: &str| DoctrineFit { id: new_id(), name: n.into(), ..Default::default() };
        let doc = |n: &str, fits: Vec<DoctrineFit>| Doctrine { id: new_id(), name: n.into(), fits, ..Default::default() };
        let caps = Group { id: "caps".into(), name: "Capitals".into(), doctrines: vec![doc("Dreads", vec![fit("Revelation")])], ..Default::default() };
        let init = Group {
            id: "init".into(),
            name: "Init".into(),
            groups: vec![caps],
            doctrines: vec![doc("Mainline", vec![fit("Ishtar"), fit("Guardian")])],
            ..Default::default()
        };
        let mut d = DoctrineDoc { groups: vec![init, Group { id: "solo".into(), name: "Solo".into(), ..Default::default() }] };
        assert_eq!(d.fit_count(), 3);
        let paths: Vec<String> = d.all_doctrines().iter().map(|(p, x)| format!("{} › {}", p.join(" › "), x.name)).collect();
        assert_eq!(paths, ["Init › Mainline", "Init › Capitals › Dreads"]);
        assert_eq!(d.parent_of("caps").map(|g| g.name.as_str()), Some("Init"));
        assert!(d.group_mut("caps").is_some());
        let dreads = d.all_doctrines()[1].1.id.clone();
        assert!(d.doctrine_mut(&dreads).is_some());
        assert_eq!(d.remove_doctrine(&dreads).map(|x| x.name), Some("Dreads".into()));
        assert_eq!(d.remove_group("caps").map(|g| g.name), Some("Capitals".into()));
        assert_eq!(d.fit_count(), 2);

        // merging a shared copy adds the SIG under the same top group
        let mut shared = d.groups.clone();
        shared[0].groups.push(Group { id: "x".into(), name: "Incursions".into(), doctrines: vec![doc("Shield", vec![fit("Nightmare")])], ..Default::default() });
        assert_eq!(merge_groups(&mut d, shared), 1);
        assert_eq!(d.groups[0].groups[0].name, "Incursions");
    }

    #[test]
    fn checks_characters_against_fits() {
        use crate::domains::skills::{Skill, SkillsData};
        // Rifter: Minmatar Frigate 1; a module needing Minmatar Frigate 3 (stand-in)
        let fit = DoctrineFit {
            id: "f".into(),
            name: "Tackle".into(),
            ship_id: 587,
            ship_name: "Rifter".into(),
            items: vec![item(587, "Rifter", "Ship", 1, &[(3329, 1)]), item(1, "Thing", "Module", 1, &[(3329, 3)])],
            ..Default::default()
        };
        let mut c = Character::new(1);
        let skill = |lvl: u8| SkillsData {
            skills: vec![Skill { skill_id: 3329, trained_skill_level: lvl, active_skill_level: lvl, ..Default::default() }],
            ..Default::default()
        };
        c.skills = Some(skill(0));
        assert_eq!(check(&c, &fit).readiness, Readiness::Missing);
        assert!(!plan_for(&c, &fit).is_empty());
        c.skills = Some(skill(1));
        assert_eq!(check(&c, &fit).readiness, Readiness::HullOnly);
        assert_eq!(check(&c, &fit).missing_items, vec!["Thing".to_string()]);
        c.skills = Some(skill(3));
        let r = check(&c, &fit);
        assert_eq!((r.readiness, r.time_to_fit), (Readiness::Ready, Duration::zero()));
        let text = eve_plan_text(&plan_for_all(&Character { skills: Some(skill(0)), ..Character::new(2) }, &[&fit]));
        // prerequisites come first
        assert!(text.starts_with("Spaceship Command 1\r\nMinmatar Frigate 1") && text.ends_with("Minmatar Frigate 3"), "{text}");
    }
}
