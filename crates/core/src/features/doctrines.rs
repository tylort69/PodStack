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
    /// Renames the fit, in its EFT text's "[Ship, Name]" header too (what EVE and Pyfa name it on import).
    pub fn rename(&mut self, name: &str) {
        let name = name.trim();
        self.name = name.to_string();
        let mut lines = self.eft.lines();
        let Some(first) = lines.next().map(str::trim) else { return };
        if !(first.starts_with('[') && first.ends_with(']')) {
            return;
        }
        let ship = first[1..first.len() - 1].split(',').next().unwrap_or_default().trim();
        let ship = if ship.is_empty() { self.ship_name.as_str() } else { ship };
        let rest: Vec<&str> = lines.collect();
        let header = format!("[{ship}, {name}]");
        self.eft = if rest.is_empty() { header } else { format!("{header}\n{}", rest.join("\n")) };
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

// ---------- editing ----------

impl DoctrineDoc {
    pub fn fit_mut(&mut self, id: &str) -> Option<&mut DoctrineFit> {
        for d in self.groups.iter_mut().flat_map(|g| g.doctrines_mut()) {
            if let Some(f) = d.fits.iter_mut().find(|f| f.id == id) {
                return Some(f);
            }
        }
        None
    }

    /// Removes a fit wherever it is.
    pub fn remove_fit(&mut self, id: &str) -> Option<DoctrineFit> {
        for d in self.groups.iter_mut().flat_map(|g| g.doctrines_mut()) {
            if let Some(i) = d.fits.iter().position(|f| f.id == id) {
                return Some(d.fits.remove(i));
            }
        }
        None
    }

    /// Whether group `id` is `ancestor` or somewhere inside it.
    pub fn is_within(&self, id: &str, ancestor: &str) -> bool {
        self.group(ancestor).is_some_and(|a| a.walk().iter().any(|(_, g)| g.id == id))
    }

    /// Moves a doctrine to the end of another group. Returns whether it moved.
    pub fn move_doctrine(&mut self, id: &str, to_group: &str) -> bool {
        if self.group(to_group).is_none() {
            return false;
        }
        let Some(d) = self.remove_doctrine(id) else { return false };
        if let Some(g) = self.group_mut(to_group) {
            g.doctrines.push(d);
        }
        true
    }

    /// Moves a fit to the end of another doctrine. Returns whether it moved.
    pub fn move_fit(&mut self, id: &str, to_doctrine: &str) -> bool {
        if self.doctrine(to_doctrine).is_none() {
            return false;
        }
        let Some(f) = self.remove_fit(id) else { return false };
        if let Some(d) = self.doctrine_mut(to_doctrine) {
            d.fits.push(f);
        }
        true
    }

    /// Moves a group (with everything in it) to the end of another group, or to the top level (`None`). A group
    /// can't go inside itself or one of its own subgroups. Returns whether it moved.
    pub fn move_group(&mut self, id: &str, to: Option<&str>) -> bool {
        if self.group(id).is_none() {
            return false;
        }
        if let Some(t) = to {
            if self.group(t).is_none() || self.is_within(t, id) {
                return false;
            }
        }
        let Some(g) = self.remove_group(id) else { return false };
        match to.and_then(|t| self.group_mut(t)) {
            Some(parent) => parent.groups.push(g),
            None => self.groups.push(g),
        }
        true
    }

    /// Where a group, doctrine or fit sits among its siblings: (index, how many siblings, itself included).
    pub fn position(&self, id: &str) -> Option<(usize, usize)> {
        fn at<T>(list: &[T], same: impl Fn(&T) -> bool) -> Option<(usize, usize)> {
            list.iter().position(same).map(|i| (i, list.len()))
        }
        if let Some(p) = at(&self.groups, |g| g.id == id) {
            return Some(p);
        }
        self.all_groups().into_iter().find_map(|(_, g)| {
            at(&g.groups, |s| s.id == id)
                .or_else(|| at(&g.doctrines, |d| d.id == id))
                .or_else(|| g.doctrines.iter().find_map(|d| at(&d.fits, |f| f.id == id)))
        })
    }

    /// Moves a group, doctrine or fit one place up (or down) among its siblings. Returns whether it moved.
    pub fn shift(&mut self, id: &str, up: bool) -> bool {
        fn swap<T>(list: &mut [T], i: usize, up: bool) -> bool {
            let j = if up { i.checked_sub(1) } else { Some(i + 1).filter(|j| *j < list.len()) };
            match j {
                Some(j) => {
                    list.swap(i, j);
                    true
                }
                None => false,
            }
        }
        fn go(list: &mut [Group], id: &str, up: bool) -> Option<bool> {
            if let Some(i) = list.iter().position(|g| g.id == id) {
                return Some(swap(list, i, up));
            }
            for g in list.iter_mut() {
                if let Some(i) = g.doctrines.iter().position(|d| d.id == id) {
                    return Some(swap(&mut g.doctrines, i, up));
                }
                for d in g.doctrines.iter_mut() {
                    if let Some(i) = d.fits.iter().position(|f| f.id == id) {
                        return Some(swap(&mut d.fits, i, up));
                    }
                }
                if let Some(r) = go(&mut g.groups, id, up) {
                    return Some(r);
                }
            }
            None
        }
        go(&mut self.groups, id, up).unwrap_or(false)
    }
}

impl Doctrine {
    /// Whether the doctrine already has a fit of this name on this hull (importing the same fit twice).
    pub fn has_fit(&self, name: &str, ship_id: i64) -> bool {
        self.fits.iter().any(|f| f.ship_id == ship_id && f.name.trim().eq_ignore_ascii_case(name.trim()))
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

/// A level of a "most fits first" plan, with how many of the fits need it.
#[derive(Clone, Debug, PartialEq)]
pub struct SharedLevel {
    pub level: PlannedLevel,
    pub fits: usize,
}

/// Every level a character needs for `fits`, the levels most of them need first: the first skills help towards as
/// many fits as possible, and each one further down towards fewer. Prerequisites still come first, and that costs
/// nothing: every fit that needs a level also needs its prerequisites and lower levels, so those count at least as
/// many fits, and a stable sort by count keeps them ahead.
pub fn plan_most_fits(c: &Character, fits: &[&DoctrineFit]) -> Vec<SharedLevel> {
    let mut seen = BTreeSet::new();
    let fits: Vec<&DoctrineFit> = fits.iter().copied().filter(|f| seen.insert(f.id.clone())).collect();
    let mut counts: BTreeMap<(i64, u8), usize> = BTreeMap::new();
    for f in &fits {
        for l in crate::training::missing_levels(c, &f.all_skills()) {
            *counts.entry(l).or_default() += 1;
        }
    }
    let mut out: Vec<SharedLevel> =
        plan_for_all(c, &fits).into_iter().map(|l| SharedLevel { fits: counts.get(&(l.skill_id, l.level)).copied().unwrap_or(0), level: l }).collect();
    out.sort_by_key(|l| std::cmp::Reverse(l.fits));
    out
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

/// A character's closest fit among several: one they can fly as it is, else the one with the least training left
/// (between equal times, the one whose hull they can already fly). None when their skills haven't loaded.
pub fn closest_fit<'a>(m: &'a Matrix, character_id: i64, fits: &[&'a DoctrineFit]) -> Option<(&'a DoctrineFit, &'a FitCheck)> {
    fits.iter()
        .filter_map(|f| m.get(&(character_id, f.id.clone())).map(|c| (*f, c)))
        .min_by_key(|(_, c)| (c.time_to_fit, std::cmp::Reverse(c.readiness)))
}

/// The best a character does with any of several fits. None when their skills haven't loaded (or no fits).
pub fn best_readiness(m: &Matrix, character_id: i64, fits: &[&DoctrineFit]) -> Option<Readiness> {
    fits.iter().filter_map(|f| m.get(&(character_id, f.id.clone()))).map(|c| c.readiness).max()
}

/// How many characters can fly at least one of several fits, how many only the hull of one, and how many neither.
/// Characters whose skills haven't loaded aren't counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub ready: usize,
    pub hull_only: usize,
    pub missing: usize,
}

pub fn tally(m: &Matrix, character_ids: &[i64], fits: &[&DoctrineFit]) -> Tally {
    let mut t = Tally::default();
    for id in character_ids {
        match best_readiness(m, *id, fits) {
            Some(Readiness::Ready) => t.ready += 1,
            Some(Readiness::HullOnly) => t.hull_only += 1,
            Some(Readiness::Missing) => t.missing += 1,
            None => {}
        }
    }
    t
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
    fn moves_and_reorders() {
        let fit = |n: &str| DoctrineFit { id: n.into(), name: n.into(), ship_id: 1, ..Default::default() };
        let doctrine = |id: &str, fits: Vec<DoctrineFit>| Doctrine { id: id.into(), name: id.into(), fits, ..Default::default() };
        let caps = Group { id: "caps".into(), name: "Capitals".into(), doctrines: vec![doctrine("dreads", vec![fit("rev")])], ..Default::default() };
        let init = Group {
            id: "init".into(),
            name: "Init".into(),
            groups: vec![caps],
            doctrines: vec![doctrine("main", vec![fit("ishtar"), fit("guardian")]), doctrine("hd", vec![])],
            ..Default::default()
        };
        let solo = Group { id: "solo".into(), name: "Solo".into(), ..Default::default() };
        let mut d = DoctrineDoc { groups: vec![init, solo] };

        assert_eq!(d.position("guardian"), Some((1, 2)));
        assert_eq!(d.position("hd"), Some((1, 2)));
        assert_eq!(d.position("solo"), Some((1, 2)));
        assert_eq!(d.position("caps"), Some((0, 1)));
        assert!(d.shift("guardian", true));
        assert!(!d.shift("guardian", true), "already first");
        assert_eq!(d.doctrine("main").unwrap().2.fits[0].id, "guardian");
        assert!(d.shift("init", false));
        assert_eq!(d.groups[0].id, "solo");

        // a group can't go inside itself
        assert!(d.is_within("caps", "init") && !d.is_within("init", "caps"));
        assert!(!d.move_group("init", Some("caps")));
        assert!(!d.move_group("init", Some("init")));
        assert!(d.move_group("caps", Some("solo")));
        assert_eq!(d.parent_of("caps").map(|g| g.id.as_str()), Some("solo"));
        assert!(d.move_group("caps", None));
        assert_eq!(d.groups.len(), 3);

        assert!(d.move_doctrine("dreads", "init"));
        assert_eq!(d.doctrine("dreads").unwrap().1.id, "init");
        assert!(!d.move_doctrine("dreads", "nowhere"));
        assert!(d.doctrine("dreads").is_some(), "kept when the target is missing");

        assert!(d.move_fit("rev", "hd"));
        assert!(d.doctrine("hd").unwrap().2.has_fit("REV ", 1));
        d.fit_mut("rev").unwrap().role = "DPS".into();
        assert_eq!(d.remove_fit("rev").map(|f| f.role), Some("DPS".into()));
        assert_eq!(d.fit_count(), 2);
    }

    #[test]
    fn renaming_updates_the_eft_header() {
        let mut f = DoctrineFit { name: "Sentry".into(), ship_name: "Ishtar".into(), eft: ISHTAR.trim().into(), ..Default::default() };
        f.rename("  Home Defense Ishtar ");
        assert_eq!(f.name, "Home Defense Ishtar");
        assert!(f.eft.starts_with("[Ishtar, Home Defense Ishtar]\nDrone Damage Amplifier II"), "{}", f.eft);
        assert_eq!(fitting::parse_eft(&f.eft).unwrap().fit_name.as_deref(), Some("Home Defense Ishtar"));
        // no EFT header (nothing to keep in step)
        let mut g = DoctrineFit { eft: String::new(), ..Default::default() };
        g.rename("X");
        assert_eq!((g.name.as_str(), g.eft.as_str()), ("X", ""));
    }

    #[test]
    fn tallies_pilots() {
        let ready = FitCheck { readiness: Readiness::Ready, time_to_fit: Duration::zero(), time_to_hull: Duration::zero(), missing_items: vec![] };
        let hull = FitCheck { readiness: Readiness::HullOnly, time_to_fit: Duration::days(3), ..ready.clone() };
        let far = FitCheck { readiness: Readiness::Missing, time_to_fit: Duration::days(1), time_to_hull: Duration::hours(5), ..ready.clone() };
        let a = DoctrineFit { id: "a".into(), ..Default::default() };
        let b = DoctrineFit { id: "b".into(), ..Default::default() };
        let mut m = Matrix::new();
        m.insert((1, "a".into()), ready.clone());
        m.insert((1, "b".into()), hull.clone());
        m.insert((2, "a".into()), hull);
        m.insert((2, "b".into()), far);
        // 3 has no skills loaded
        let fits = [&a, &b];
        assert_eq!(tally(&m, &[1, 2, 3], &fits), Tally { ready: 1, hull_only: 1, missing: 0 });
        assert_eq!(best_readiness(&m, 3, &fits), None);
        assert_eq!(closest_fit(&m, 1, &fits).map(|(f, _)| f.id.as_str()), Some("a"));
        // least training wins, even when it's a hull they can't fly yet
        assert_eq!(closest_fit(&m, 2, &fits).map(|(f, _)| f.id.as_str()), Some("b"));
        assert_eq!(tally(&m, &[1, 2], &[&b]), Tally { ready: 0, hull_only: 1, missing: 1 });
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

    #[test]
    fn most_fits_first() {
        // Minmatar Frigate III for one fit, I for another, Gallente Frigate I for a third (and the first twice)
        let fit = |id: &str, skills: &[(i64, u8)]| DoctrineFit { id: id.into(), items: vec![item(1, "Hull", "Ship", 1, skills)], ..Default::default() };
        let (a, b, g) = (fit("a", &[(3329, 3)]), fit("b", &[(3329, 1)]), fit("g", &[(3328, 1)]));
        let c = Character { skills: Some(crate::domains::skills::SkillsData::default()), ..Character::new(1) };
        let plan = plan_most_fits(&c, &[&a, &b, &g, &a]);
        let got: Vec<(String, u8, usize)> = plan.iter().map(|l| (l.level.name(), l.level.level, l.fits)).collect();
        let want = [("Spaceship Command", 1, 3), ("Minmatar Frigate", 1, 2), ("Gallente Frigate", 1, 1), ("Minmatar Frigate", 2, 1), ("Minmatar Frigate", 3, 1)];
        assert_eq!(got, want.map(|(n, l, f)| (n.to_string(), l, f)));
    }
}

// =====================================================================================================================
// ---------- Pilots and Ships tabs: who flies what, by character and by hull class ----------
// =====================================================================================================================

use crate::sde::MARKET;

/// Every fit of a group and its subgroups (every fit when `group_id` is None), with its full group path.
pub fn fits_in<'a>(doc: &'a DoctrineDoc, group_id: Option<&str>) -> Vec<(Vec<&'a str>, &'a Doctrine, &'a DoctrineFit)> {
    let Some(id) = group_id else { return doc.all_fits() };
    let Some(group) = doc.group(id) else { return Vec::new() };
    let wanted: BTreeSet<&str> = group.all_doctrines().iter().map(|d| d.id.as_str()).collect();
    doc.all_fits().into_iter().filter(|(_, d, _)| wanted.contains(d.id.as_str())).collect()
}

/// Every doctrine of a group and its subgroups (every doctrine when `group_id` is None), with its full group path.
pub fn doctrines_in<'a>(doc: &'a DoctrineDoc, group_id: Option<&str>) -> Vec<(Vec<&'a str>, &'a Doctrine)> {
    let Some(id) = group_id else { return doc.all_doctrines() };
    let Some(group) = doc.group(id) else { return Vec::new() };
    let wanted: BTreeSet<&str> = group.all_doctrines().iter().map(|d| d.id.as_str()).collect();
    doc.all_doctrines().into_iter().filter(|(_, d)| wanted.contains(d.id.as_str())).collect()
}

/// A character against one fit, with where the fit sits.
#[derive(Clone, Debug)]
pub struct Standing<'a> {
    pub path: Vec<&'a str>,
    pub doctrine: &'a Doctrine,
    pub fit: &'a DoctrineFit,
    pub check: FitCheck,
}

/// A character against a set of fits, sorted for showing.
#[derive(Clone, Debug, Default)]
pub struct PilotStandings<'a> {
    /// Fits it can fly as they are, in the document's order.
    pub ready: Vec<Standing<'a>>,
    /// Fits whose hull it can fly but not everything fitted, quickest to finish first.
    pub hull_only: Vec<Standing<'a>>,
    /// Fits whose hull it can't fly yet, quickest to train first.
    pub missing: Vec<Standing<'a>>,
}

/// Checks a character against fits (see [`fits_in`]). A character whose skills aren't loaded gets nothing.
pub fn pilot_standings<'a>(c: &Character, fits: &[(Vec<&'a str>, &'a Doctrine, &'a DoctrineFit)]) -> PilotStandings<'a> {
    let mut out = PilotStandings::default();
    if c.skills.is_none() {
        return out;
    }
    for (path, doctrine, fit) in fits {
        let s = Standing { path: path.clone(), doctrine, fit, check: check(c, fit) };
        match s.check.readiness {
            Readiness::Ready => out.ready.push(s),
            Readiness::HullOnly => out.hull_only.push(s),
            Readiness::Missing => out.missing.push(s),
        }
    }
    out.hull_only.sort_by_key(|s| s.check.time_to_fit);
    out.missing.sort_by_key(|s| s.check.time_to_fit);
    out
}

impl PilotStandings<'_> {
    /// Distinct doctrines with at least one fit ready.
    pub fn doctrines_ready(&self) -> usize {
        self.ready.iter().map(|s| s.doctrine.id.as_str()).collect::<BTreeSet<_>>().len()
    }
}

/// One character against one doctrine (a cell of the pilots × doctrines matrix).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DoctrineCell {
    pub fits: usize,
    pub ready: usize,
    pub hull_only: usize,
    /// The best readiness over the doctrine's fits (None when it has none).
    pub best: Option<Readiness>,
    /// Training to the quickest fit not flyable yet (None when every fit is ready, or there are none).
    pub closest: Option<Duration>,
}

pub fn doctrine_cell<'a>(checks: impl IntoIterator<Item = &'a FitCheck>) -> DoctrineCell {
    let mut cell = DoctrineCell::default();
    for c in checks {
        cell.fits += 1;
        match c.readiness {
            Readiness::Ready => cell.ready += 1,
            Readiness::HullOnly => cell.hull_only += 1,
            Readiness::Missing => {}
        }
        cell.best = cell.best.max(Some(c.readiness));
        if c.readiness != Readiness::Ready {
            cell.closest = Some(cell.closest.map_or(c.time_to_fit, |t| t.min(c.time_to_fit)));
        }
    }
    cell
}

/// Hull classes (CCP's ship groups) in sections, for the Ships tab.
#[derive(Clone, Debug, PartialEq)]
pub struct HullSection {
    pub name: String,
    /// Class names, entry-level first.
    pub classes: Vec<String>,
}

/// The section for hulls not on the market (capsules, event hulls).
pub const OTHER_SECTION: &str = "Other";

/// Skill points a fresh character needs to sit in a hull, prerequisites included: a measure of how far up the
/// progression a hull is that follows CCP's data instead of a list of class names.
pub fn sp_to_fly(ship: &ShipInfo) -> i64 {
    let mut all: BTreeMap<i64, u8> = BTreeMap::new();
    for r in &ship.skills {
        for (id, l) in crate::training::with_prerequisites(r.id, r.level) {
            let e = all.entry(id).or_insert(0);
            *e = (*e).max(l);
        }
    }
    all.into_iter().map(|(id, l)| SKILLS.get(id).map(|s| s.sp_for_level(l)).unwrap_or(0)).sum()
}

fn median<T: Ord + Copy>(mut v: Vec<T>) -> Option<T> {
    v.sort();
    v.get(v.len() / 2).copied()
}

/// The top-level market section a hull is sold in ("Frigates", "Capital Ships", …).
fn market_section(s: &ShipInfo) -> Option<String> {
    let t = MARKET.get(s.type_id)?;
    let path = MARKET.path(t.group);
    // the path starts at the market's "Ships" root
    path.get(1).or(path.first()).map(|g| g.name.clone())
}

/// Every hull class in sections, ordered from small to big. Everything comes from CCP's static data:
///
/// - a section is a top-level group of the market's ship tree ("Frigates", "Capital Ships", "Mining Barges", …),
///   and a class goes in the section most of its hulls are sold in. Some market sections hold one-off hulls of
///   many classes ("Special Edition Ships"); a section where most classes are mostly sold elsewhere is treated as
///   such a catch-all and only keeps classes found nowhere else;
/// - sections are ordered by the median packaged volume of their hulls (the market's own measure of hull size, so
///   shuttles and frigates come first and capitals last), catch-alls after them, then hulls not on the market;
/// - classes inside a section are ordered by the median skill points a new character needs to fly their hulls
///   ([`sp_to_fly`]): the Tech I class first, then its specialisations.
pub fn hull_sections() -> Vec<HullSection> {
    let ships: Vec<(&ShipInfo, Option<String>)> = SHIPS.by_id.values().map(|s| (s, market_section(s))).collect();
    let mut by_class: BTreeMap<&str, Vec<&(&ShipInfo, Option<String>)>> = BTreeMap::new();
    for s in &ships {
        by_class.entry(s.0.group.as_str()).or_default().push(s);
    }
    let majority = |hulls: &[&(&ShipInfo, Option<String>)], skip: &BTreeSet<String>| -> Option<String> {
        let mut n: BTreeMap<&str, usize> = BTreeMap::new();
        for (_, sec) in hulls {
            if let Some(sec) = sec.as_deref().filter(|s| !skip.contains(*s)) {
                *n.entry(sec).or_default() += 1;
            }
        }
        // most hulls; ties go to the first name, so the result doesn't depend on hash order
        n.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(a.0))).map(|(s, _)| s.to_string())
    };
    // classes per market section
    let mut held: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for (s, sec) in &ships {
        if let Some(sec) = sec {
            held.entry(sec.as_str()).or_default().insert(s.group.as_str());
        }
    }
    // whether a section is where most of its classes are mostly sold, when the `skip` sections don't count
    let leads = |sec: &str, skip: &BTreeSet<String>| -> bool {
        let classes = &held[sec];
        let own = classes.iter().filter(|c| majority(&by_class[*c], skip).as_deref() == Some(sec)).count();
        own * 2 >= classes.len()
    };
    // Catch-alls: sections that don't lead their classes. A small section whose hulls are mostly outnumbered by the
    // catch-all's (a class with more special editions than regular hulls) comes back once the other catch-alls
    // don't count.
    let first: BTreeSet<String> = held.keys().filter(|s| !leads(s, &BTreeSet::new())).map(|s| s.to_string()).collect();
    let catch_all: BTreeSet<String> = first
        .iter()
        .filter(|s| {
            let others: BTreeSet<String> = first.iter().filter(|o| o != s).cloned().collect();
            !leads(s, &others)
        })
        .cloned()
        .collect();
    let raw: BTreeMap<&str, Option<String>> = by_class.iter().map(|(c, h)| (*c, majority(h, &BTreeSet::new()))).collect();

    let mut sections: BTreeMap<String, Vec<(i64, &str)>> = BTreeMap::new();
    for (class, hulls) in &by_class {
        let sec = majority(hulls, &catch_all).or_else(|| raw[class].clone()).unwrap_or_else(|| OTHER_SECTION.to_string());
        let sp = median(hulls.iter().map(|(s, _)| sp_to_fly(s)).collect()).unwrap_or(0);
        sections.entry(sec).or_default().push((sp, class));
    }
    // median packaged volume (m³ × 100, to sort as integers); None for hulls off the market
    let size = |sec: &str| -> Option<i64> {
        median(
            ships
                .iter()
                .filter(|(_, s)| s.as_deref() == Some(sec))
                .filter_map(|(s, _)| MARKET.get(s.type_id))
                .map(|t| (t.volume * 100.0) as i64)
                .collect(),
        )
    };
    let mut out: Vec<((u8, i64), HullSection)> = sections
        .into_iter()
        .map(|(name, mut classes)| {
            classes.sort();
            let rank = match size(&name) {
                None => (2, 0),
                Some(v) if catch_all.contains(&name) => (1, v),
                Some(v) => (0, v),
            };
            (rank, HullSection { classes: classes.into_iter().map(|(_, c)| c.to_string()).collect(), name })
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.name.cmp(&b.1.name)));
    out.into_iter().map(|(_, s)| s).collect()
}

/// The tech levels hulls come in ("Tech I", "Tech II", "Faction", …), in CCP's meta group order.
pub fn tech_levels() -> Vec<String> {
    let mut order: BTreeMap<&str, i64> = BTreeMap::new();
    for s in SHIPS.by_id.values().filter(|s| !s.meta.is_empty()) {
        let id = MARKET.get(s.type_id).and_then(|t| t.meta_group).unwrap_or(i64::MAX);
        let e = order.entry(s.meta.as_str()).or_insert(i64::MAX);
        *e = (*e).min(id);
    }
    let mut v: Vec<(i64, &str)> = order.into_iter().map(|(m, id)| (id, m)).collect();
    v.sort();
    v.into_iter().map(|(_, m)| m.to_string()).collect()
}

#[cfg(test)]
mod roster_tests {
    use super::*;
    use crate::domains::skills::{Skill, SkillsData};

    fn fit(id: &str, ship: i64, items: Vec<FitItem>) -> DoctrineFit {
        DoctrineFit { id: id.into(), name: id.into(), ship_id: ship, items, ..Default::default() }
    }

    fn skilled(levels: &[(i64, u8)]) -> Character {
        let mut c = Character::new(1);
        c.skills = Some(SkillsData {
            skills: levels.iter().map(|&(id, l)| Skill { skill_id: id, trained_skill_level: l, active_skill_level: l, ..Default::default() }).collect(),
            ..Default::default()
        });
        c
    }

    #[test]
    fn standings_by_group_and_readiness() {
        // Rifter: Minmatar Frigate 1; its module needs Minmatar Frigate at `module_level` (a stand-in)
        let rifter = |id: &str, module_level: u8| {
            fit(
                id,
                587,
                vec![
                    FitItem { type_id: 587, name: "Rifter".into(), category: "Ship".into(), quantity: 1, skills: vec![(3329, 1)] },
                    FitItem { type_id: 1, name: "Thing".into(), category: "Module".into(), quantity: 1, skills: vec![(3329, module_level)] },
                ],
            )
        };
        let caps = Group {
            id: "caps".into(),
            name: "Capitals".into(),
            doctrines: vec![Doctrine { id: "d2".into(), name: "Far".into(), fits: vec![rifter("far", 5)], ..Default::default() }],
            ..Default::default()
        };
        let doc = DoctrineDoc {
            groups: vec![Group {
                id: "init".into(),
                name: "Init".into(),
                groups: vec![caps],
                doctrines: vec![Doctrine { id: "d1".into(), name: "Near".into(), fits: vec![rifter("easy", 1), rifter("mid", 3)], ..Default::default() }],
                ..Default::default()
            }],
        };
        assert_eq!(fits_in(&doc, None).len(), 3);
        let sub = fits_in(&doc, Some("caps"));
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].0, ["Init", "Capitals"], "full path, not relative to the group");
        assert_eq!(doctrines_in(&doc, Some("init")).len(), 2);
        assert!(fits_in(&doc, Some("nope")).is_empty());

        let c = skilled(&[(3327, 1), (3329, 1)]);
        let s = pilot_standings(&c, &fits_in(&doc, None));
        assert_eq!(s.ready.iter().map(|s| s.fit.id.as_str()).collect::<Vec<_>>(), ["easy"]);
        assert_eq!(s.hull_only.iter().map(|s| s.fit.id.as_str()).collect::<Vec<_>>(), ["mid", "far"], "quickest first");
        assert_eq!(s.doctrines_ready(), 1);
        assert!(pilot_standings(&Character::new(2), &fits_in(&doc, None)).ready.is_empty(), "no skills, no standings");

        let checks: Vec<FitCheck> = s.ready.iter().chain(&s.hull_only).filter(|x| x.doctrine.id == "d1").map(|x| x.check.clone()).collect();
        let cell = doctrine_cell(&checks);
        assert_eq!((cell.fits, cell.ready, cell.hull_only, cell.best), (2, 1, 1, Some(Readiness::Ready)));
        assert_eq!(cell.closest, Some(s.hull_only[0].check.time_to_fit));
        assert_eq!(doctrine_cell(std::iter::empty()), DoctrineCell::default());
    }

    #[test]
    fn hull_sections_follow_size_and_progression() {
        let sections = hull_sections();
        let pos = |class: &str| sections.iter().position(|s| s.classes.iter().any(|c| c == class)).unwrap_or_else(|| panic!("{class} missing"));
        let section = |class: &str| sections[pos(class)].name.clone();
        // every class lands in exactly one section
        let n: usize = sections.iter().map(|s| s.classes.len()).sum();
        assert_eq!(n, SHIPS.by_id.values().map(|s| s.group.as_str()).collect::<BTreeSet<_>>().len());
        assert!(pos("Frigate") < pos("Cruiser") && pos("Cruiser") < pos("Battleship") && pos("Battleship") < pos("Dreadnought"));
        assert_eq!(section("Heavy Assault Cruiser"), section("Cruiser"));
        assert_eq!(section("Dreadnought"), section("Titan"));
        // a class with one special-edition hull still sits with its own kind
        assert_eq!(section("Assault Frigate"), section("Frigate"));
        assert_eq!(section("Corvette"), "Corvettes");
        // the Tech I class leads its section
        assert_eq!(sections[pos("Frigate")].classes[0], "Frigate");
        assert_eq!(sections[pos("Battleship")].classes[0], "Battleship");
        // hulls off the market come last
        assert_eq!(sections.last().map(|s| s.name.as_str()), Some(OTHER_SECTION));
        assert_eq!(section("Capsule"), OTHER_SECTION);
    }

    #[test]
    fn tech_levels_in_meta_order() {
        let t = tech_levels();
        assert_eq!(t.first().map(String::as_str), Some("Tech I"));
        assert!(t.iter().position(|m| m == "Tech II") < t.iter().position(|m| m == "Tech III"));
        assert!(t.contains(&"Faction".to_string()));
    }

    #[test]
    fn sp_to_fly_counts_prerequisites() {
        // Rifter: Minmatar Frigate I (rank 2) after Spaceship Command I (rank 1)
        assert_eq!(sp_to_fly(&SHIPS.by_id[&587]), 500 + 250);
    }
}
