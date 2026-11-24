//! Doctrine ship stock: for each character and doctrine, the fits it can fly, how many ships of each it should keep
//! (its stock target), how many it has (from its assets, by hull type, and whether each is fitted as the doctrine
//! says), where they are, and what buying the rest costs. Characters are grouped by account, the player's own
//! grouping.
//!
//! Targets are set at several levels and the most specific one set wins: for one character a fit, then a doctrine,
//! then everything that character flies; for everyone a fit, then a doctrine, then everything (one of each unless
//! changed). A target of 0 means the character doesn't stock that fit.
//!
//! Stock is worked out per doctrine: inside one doctrine each ship counts for one fit (a target of two for each of
//! two Ishtar fits needs four Ishtars), but the same ship counts for every doctrine it fits, since a pilot flies one
//! doctrine at a time. The shopping list follows that: a hull several doctrines are short of is bought once per
//! character, as many as the doctrine short of the most of them.
//!
//! Prices are per unit and come from the caller (the page uses Jita's lowest sell orders). A fit that lists modules
//! is bought as the whole fit (hull, modules, charges, drones); a fit that's only a hull is bought as the hull.


use super::accounts::AccountsDoc;
use super::doctrines::{self, Doctrine, DoctrineFit, Readiness};
use crate::character::Character;
use crate::domains::assets::AssetsData;
use crate::sde::SHIPS;
use crate::{store, Core, Event};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Slot flags of fitted modules and subsystems (ESI's location_flag, e.g. "HiSlot0", "RigSlot2").
const FITTED_FLAGS: [&str; 5] = ["HiSlot", "MedSlot", "LoSlot", "RigSlot", "SubSystemSlot"];

/// A ship (or a stack of packaged ones) in a character's assets.
#[derive(Clone, Debug, PartialEq)]
pub struct OwnedShip {
    pub item_id: i64,
    pub type_id: i64,
    /// "Custom name (Type)" or the type name.
    pub label: String,
    pub location_id: i64,
    pub location: String,
    /// The ship or container it's in (a carrier's ship bay, a station container), if any.
    pub inside: Option<String>,
    /// Packaged ships stack and can't have anything fitted.
    pub packaged: bool,
    pub count: u32,
    /// Fitted modules and subsystems: type → how many.
    pub fitted: BTreeMap<i64, u32>,
}

/// Every ship in a character's assets, packaged or assembled, with what's fitted to the assembled ones.
pub fn owned_ships(data: &AssetsData) -> Vec<OwnedShip> {
    let by_id = data.by_id();
    let mut fitted: HashMap<i64, BTreeMap<i64, u32>> = HashMap::new();
    for a in &data.items {
        if let Some(p) = a.parent_id.filter(|_| FITTED_FLAGS.iter().any(|f| a.flag.starts_with(f))) {
            *fitted.entry(p).or_default().entry(a.type_id).or_default() += a.count().max(1) as u32;
        }
    }
    data.items
        .iter()
        .filter(|a| SHIPS.by_id.contains_key(&a.type_id) && !a.is_blueprint_copy)
        .map(|a| OwnedShip {
            item_id: a.item_id,
            type_id: a.type_id,
            label: a.label(),
            location_id: a.root_location_id,
            location: data.location_name(a.root_location_id),
            inside: a.parent_id.and_then(|p| by_id.get(&p)).map(|p| p.label()),
            packaged: !a.is_singleton,
            count: a.count().max(1) as u32,
            fitted: fitted.remove(&a.item_id).unwrap_or_default(),
        })
        .collect()
}

/// The modules and subsystems a fit puts in slots: type → how many.
pub fn fit_modules(fit: &DoctrineFit) -> BTreeMap<i64, u32> {
    let mut m = BTreeMap::new();
    for i in fit.items.iter().skip(1).filter(|i| matches!(i.category.as_str(), "Module" | "Subsystem")) {
        *m.entry(i.type_id).or_default() += i.quantity.max(1);
    }
    m
}

/// Whether an assembled ship has at least every module the fit lists (more is fine: a spare slot filled).
pub fn fitted_as(ship: &OwnedShip, fit: &DoctrineFit) -> bool {
    ship.type_id == fit.ship_id && !ship.packaged && fit_modules(fit).iter().all(|(t, n)| ship.fitted.get(t).copied().unwrap_or(0) >= *n)
}


/// What a missing fit was priced as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Basis {
    /// Hull, modules, charges and drones.
    Fit,
    /// The fit lists nothing but the hull.
    Hull,
}

impl Basis {
    pub fn label(self) -> &'static str {
        match self {
            Basis::Fit => "Full fit",
            Basis::Hull => "Hull only",
        }
    }
}

/// The price of buying a fit.
#[derive(Clone, Debug, PartialEq)]
pub struct Cost {
    /// ISK for everything that has a price.
    pub isk: f64,
    pub basis: Basis,
    /// Names of items without a price (nobody sells them, or prices haven't loaded): `isk` leaves them out.
    pub unpriced: Vec<String>,
}

impl Cost {
    pub fn complete(&self) -> bool {
        self.unpriced.is_empty()
    }
}

/// What a fit costs at `prices` (ISK per unit, by type).
pub fn fit_cost(fit: &DoctrineFit, prices: &HashMap<i64, f64>) -> Cost {
    let mut cost = Cost { isk: 0.0, basis: if fit.items.len() > 1 { Basis::Fit } else { Basis::Hull }, unpriced: Vec::new() };
    let hull_only = [(fit.ship_id, fit.ship_name.as_str(), 1)];
    let items: Vec<(i64, &str, u32)> = match cost.basis {
        Basis::Fit => fit.items.iter().map(|i| (i.type_id, i.name.as_str(), i.quantity.max(1))).collect(),
        Basis::Hull => hull_only.to_vec(),
    };
    for (t, name, n) in items {
        match prices.get(&t) {
            Some(p) => cost.isk += p * n as f64,
            None => cost.unpriced.push(name.to_string()),
        }
    }
    cost
}

/// Every type to price for a set of fits as [`fit_cost`] would.
pub fn types_to_price<'a>(fits: impl IntoIterator<Item = &'a DoctrineFit>) -> Vec<i64> {
    let mut v: Vec<i64> = fits
        .into_iter()
        .flat_map(|f| if f.items.len() > 1 { f.items.iter().map(|i| i.type_id).collect() } else { vec![f.ship_id] })
        .collect();
    v.sort();
    v.dedup();
    v
}


// ---------- stock targets ----------

/// The document the stock targets are kept in.
pub const TARGETS_DOC: &str = "doctrine_stock";
/// The target when none is set anywhere.
pub const DEFAULT_TARGET: u32 = 1;

/// Targets for one scope (everyone, or one character): for everything, per doctrine, and per fit of a doctrine
/// (keyed by [`fit_key`], as the same fit can sit in several doctrines).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ScopeTargets {
    pub overall: Option<u32>,
    pub doctrines: BTreeMap<String, u32>,
    pub fits: BTreeMap<String, u32>,
}

impl ScopeTargets {
    fn is_empty(&self) -> bool {
        self.overall.is_none() && self.doctrines.is_empty() && self.fits.is_empty()
    }
    fn get(&self, level: Level, doctrine: Option<&str>, fit: Option<&str>) -> Option<u32> {
        match level {
            Level::Overall => self.overall,
            Level::Doctrine => self.doctrines.get(doctrine?).copied(),
            Level::Fit => self.fits.get(&fit_key(doctrine?, fit?)).copied(),
        }
    }
    fn set(&mut self, level: Level, doctrine: Option<&str>, fit: Option<&str>, v: Option<u32>) {
        fn put(m: &mut BTreeMap<String, u32>, k: String, v: Option<u32>) {
            match v {
                Some(v) => m.insert(k, v),
                None => m.remove(&k),
            };
        }
        match (level, doctrine, fit) {
            (Level::Overall, _, _) => self.overall = v,
            (Level::Doctrine, Some(d), _) => put(&mut self.doctrines, d.to_string(), v),
            (Level::Fit, Some(d), Some(f)) => put(&mut self.fits, fit_key(d, f), v),
            _ => {}
        }
    }
}

/// How specific a target is, inside its scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Overall,
    Doctrine,
    Fit,
}

/// Where a target came from: the level and whether it was set for the one character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Nothing set: [`DEFAULT_TARGET`].
    Default,
    Everyone(Level),
    Character(Level),
}

/// Every stock target the user set.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Targets {
    pub everyone: ScopeTargets,
    pub characters: BTreeMap<i64, ScopeTargets>,
}

pub fn fit_key(doctrine: &str, fit: &str) -> String {
    format!("{doctrine}/{fit}")
}

impl Targets {
    /// The target for a character's fit in a doctrine.
    pub fn target(&self, character: i64, doctrine: &str, fit: &str) -> u32 {
        self.resolve(Some(character), Some(doctrine), Some(fit)).0
    }

    /// The target that applies at a place in the hierarchy, and where it comes from. `character` None asks for
    /// everyone; `doctrine`/`fit` None ask for a broader level (what a doctrine's or everything's value is).
    pub fn resolve(&self, character: Option<i64>, doctrine: Option<&str>, fit: Option<&str>) -> (u32, Source) {
        let levels: &[Level] = match (doctrine, fit) {
            (Some(_), Some(_)) => &[Level::Fit, Level::Doctrine, Level::Overall],
            (Some(_), None) => &[Level::Doctrine, Level::Overall],
            _ => &[Level::Overall],
        };
        if let Some(c) = character.and_then(|c| self.characters.get(&c)) {
            for &l in levels {
                if let Some(v) = c.get(l, doctrine, fit) {
                    return (v, Source::Character(l));
                }
            }
        }
        for &l in levels {
            if let Some(v) = self.everyone.get(l, doctrine, fit) {
                return (v, Source::Everyone(l));
            }
        }
        (DEFAULT_TARGET, Source::Default)
    }

    /// The value set exactly at this place (not inherited), if any.
    pub fn exact(&self, character: Option<i64>, doctrine: Option<&str>, fit: Option<&str>) -> Option<u32> {
        let level = level_of(doctrine, fit);
        match character {
            Some(c) => self.characters.get(&c)?.get(level, doctrine, fit),
            None => self.everyone.get(level, doctrine, fit),
        }
    }

    /// Sets (or with None clears) the target at this place.
    pub fn set(&mut self, character: Option<i64>, doctrine: Option<&str>, fit: Option<&str>, v: Option<u32>) {
        let level = level_of(doctrine, fit);
        match character {
            Some(c) => {
                let s = self.characters.entry(c).or_default();
                s.set(level, doctrine, fit, v);
                if s.is_empty() {
                    self.characters.remove(&c);
                }
            }
            None => self.everyone.set(level, doctrine, fit, v),
        }
    }

    /// Clears every target set for one character, so it follows everyone's again.
    pub fn clear_character(&mut self, character: i64) {
        self.characters.remove(&character);
    }
}

fn level_of(doctrine: Option<&str>, fit: Option<&str>) -> Level {
    match (doctrine, fit) {
        (Some(_), Some(_)) => Level::Fit,
        (Some(_), None) => Level::Doctrine,
        _ => Level::Overall,
    }
}

pub fn load_targets() -> Targets {
    store::load_doc(TARGETS_DOC)
}

pub fn save_targets(core: &Core, t: &Targets) {
    store::save_doc(TARGETS_DOC, t);
    core.emit(Event::Doc(TARGETS_DOC.into()));
}

// ---------- stock ----------

/// How a character's stock of a fit stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Holding {
    /// Fewer ships than the target.
    Short,
    /// As many as the target, some of them hulls not fitted as the doctrine says (or packaged).
    Hulls,
    /// As many as the target, all fitted as the doctrine.
    Fitted,
    /// The target is 0.
    NotStocked,
}

impl Holding {
    pub fn label(self) -> &'static str {
        match self {
            Holding::Short => "Short",
            Holding::Hulls => "Stocked, not all fitted",
            Holding::Fitted => "Stocked, fitted",
            Holding::NotStocked => "Not stocked",
        }
    }
}

/// A doctrine fit a character can fly, its target and the ships counted for it.
#[derive(Clone, Debug)]
pub struct FitLine<'a> {
    pub fit: &'a DoctrineFit,
    pub readiness: Readiness,
    pub target: u32,
    /// Ships counted fitted as the doctrine, and as hulls.
    pub fitted: u32,
    pub hulls: u32,
    /// The ships counted (a packaged stack's `count` is how many of it were counted here).
    pub ships: Vec<OwnedShip>,
    /// What buying one costs.
    pub cost: Cost,
}

impl FitLine<'_> {
    pub fn stock(&self) -> u32 {
        self.fitted + self.hulls
    }
    pub fn short(&self) -> u32 {
        self.target.saturating_sub(self.stock())
    }
    pub fn holding(&self) -> Holding {
        if self.target == 0 {
            Holding::NotStocked
        } else if self.short() > 0 {
            Holding::Short
        } else if self.hulls > 0 {
            Holding::Hulls
        } else {
            Holding::Fitted
        }
    }
    fn count(&mut self, ship: &OwnedShip, fitted: bool) {
        if fitted {
            self.fitted += 1;
        } else {
            self.hulls += 1;
        }
        match self.ships.iter_mut().find(|s| s.item_id == ship.item_id) {
            Some(s) => s.count += 1,
            None => self.ships.push(OwnedShip { count: 1, ..ship.clone() }),
        }
    }
}

/// A character against one doctrine.
#[derive(Clone, Debug)]
pub struct DoctrineLine<'a> {
    pub path: Vec<&'a str>,
    pub doctrine: &'a Doctrine,
    /// The fits it can fly, in the doctrine's order.
    pub flyable: Vec<FitLine<'a>>,
}

impl<'a> DoctrineLine<'a> {
    pub fn fits(&self) -> usize {
        self.doctrine.fits.len()
    }
    pub fn target(&self) -> u32 {
        self.flyable.iter().map(|l| l.target).sum()
    }
    pub fn stock(&self) -> u32 {
        self.flyable.iter().map(|l| l.stock()).sum()
    }
    pub fn short(&self) -> u32 {
        self.flyable.iter().map(|l| l.short()).sum()
    }
    /// What buying this doctrine's shortfall alone costs.
    pub fn cost(&self) -> f64 {
        self.flyable.iter().map(|l| l.cost.isk * l.short() as f64).sum()
    }
}

/// Ships a character still has to buy for one fit (see the module docs for how doctrines share hulls).
#[derive(Clone, Debug)]
pub struct Purchase<'a> {
    pub fit: &'a DoctrineFit,
    /// The doctrine this purchase is counted for.
    pub doctrine: &'a Doctrine,
    /// How many doctrines are short of this hull.
    pub wanted_by: usize,
    pub quantity: u32,
    /// What one costs.
    pub cost: Cost,
}

impl Purchase<'_> {
    pub fn total(&self) -> f64 {
        self.cost.isk * self.quantity as f64
    }
}

/// One character against every doctrine.
#[derive(Clone, Debug)]
pub struct CharacterInventory<'a> {
    pub character_id: i64,
    pub name: String,
    /// Without assets, nothing can be said about stock: such a character is left out of totals.
    pub assets_loaded: bool,
    pub doctrines: Vec<DoctrineLine<'a>>,
    pub to_buy: Vec<Purchase<'a>>,
}

impl CharacterInventory<'_> {
    pub fn flyable(&self) -> usize {
        self.doctrines.iter().map(|d| d.flyable.len()).sum()
    }
    pub fn target(&self) -> u32 {
        self.doctrines.iter().map(|d| d.target()).sum()
    }
    pub fn stock(&self) -> u32 {
        self.doctrines.iter().map(|d| d.stock()).sum()
    }
    pub fn short(&self) -> u32 {
        self.doctrines.iter().map(|d| d.short()).sum()
    }
    /// Doctrines with at least one fit it can fly.
    pub fn doctrines_flyable(&self) -> usize {
        self.doctrines.iter().filter(|d| !d.flyable.is_empty()).count()
    }
    /// Ships in [`Self::to_buy`].
    pub fn ships_to_buy(&self) -> u32 {
        self.to_buy.iter().map(|p| p.quantity).sum()
    }
    /// What buying everything in [`Self::to_buy`] costs.
    pub fn cost(&self) -> f64 {
        self.to_buy.iter().map(|p| p.total()).sum()
    }
    /// Whether some of that has no price.
    pub fn partly_priced(&self) -> bool {
        self.to_buy.iter().any(|p| !p.cost.complete())
    }
}

/// Which fits count as flyable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Options {
    /// Also count fits where only the hull can be flown (not every module yet).
    pub hull_only: bool,
}

/// A character against doctrines. A character whose skills aren't loaded yet gets None.
pub fn character_inventory<'a>(
    c: &Character,
    doctrines: &[(Vec<&'a str>, &'a Doctrine)],
    targets: &Targets,
    prices: &HashMap<i64, f64>,
    opts: Options,
) -> Option<CharacterInventory<'a>> {
    c.skills.as_ref()?;
    let ships = c.assets.as_ref().map(owned_ships).unwrap_or_default();
    let mut lines = Vec::new();
    for (path, d) in doctrines {
        let mut flyable: Vec<FitLine<'a>> = d
            .fits
            .iter()
            .filter_map(|f| {
                let r = doctrines::check(c, f).readiness;
                let ok = r == Readiness::Ready || opts.hull_only && r == Readiness::HullOnly;
                ok.then(|| FitLine {
                    fit: f,
                    readiness: r,
                    target: targets.target(c.id, &d.id, &f.id),
                    fitted: 0,
                    hulls: 0,
                    ships: Vec::new(),
                    cost: fit_cost(f, prices),
                })
            })
            .collect();
        allocate(&mut flyable, &ships);
        lines.push(DoctrineLine { path: path.clone(), doctrine: d, flyable });
    }
    let to_buy = if c.assets.is_some() { shopping_list(&lines) } else { Vec::new() };
    Some(CharacterInventory { character_id: c.id, name: c.name(), assets_loaded: c.assets.is_some(), doctrines: lines, to_buy })
}

/// Counts ships toward one doctrine's fits, one ship each, up to each fit's target: first ships fitted as the fit,
/// then any remaining hull of the type. Fits take turns, so a scarce hull is spread over the fits that want it.
fn allocate(lines: &mut [FitLine<'_>], ships: &[OwnedShip]) {
    let mut left: Vec<u32> = ships.iter().map(|s| s.count).collect();
    for fitted in [true, false] {
        loop {
            let mut any = false;
            for l in lines.iter_mut().filter(|l| l.stock() < l.target) {
                let found = ships
                    .iter()
                    .enumerate()
                    .filter(|(i, s)| left[*i] > 0 && s.type_id == l.fit.ship_id)
                    .filter(|(_, s)| !fitted || fitted_as(s, l.fit))
                    // the hull pass prefers assembled ships (ready to undock after a refit) over packaged ones
                    .min_by_key(|(_, s)| s.packaged)
                    .map(|(i, _)| i);
                if let Some(i) = found {
                    left[i] -= 1;
                    l.count(&ships[i], fitted);
                    any = true;
                }
            }
            if !any {
                break;
            }
        }
    }
}

/// Per hull, the doctrine short of the most of it (the priciest of those on a tie) decides what's bought.
fn shopping_list<'a>(lines: &[DoctrineLine<'a>]) -> Vec<Purchase<'a>> {
    // hull → (doctrine index, its fits short of that hull)
    let mut by_hull: BTreeMap<i64, Vec<(usize, Vec<&FitLine<'a>>)>> = BTreeMap::new();
    for (i, d) in lines.iter().enumerate() {
        let mut per: BTreeMap<i64, Vec<&FitLine<'a>>> = BTreeMap::new();
        for l in d.flyable.iter().filter(|l| l.short() > 0) {
            per.entry(l.fit.ship_id).or_default().push(l);
        }
        for (hull, fits) in per {
            by_hull.entry(hull).or_default().push((i, fits));
        }
    }
    let mut out = Vec::new();
    for wants in by_hull.into_values() {
        let units = |f: &[&FitLine]| f.iter().map(|l| l.short()).sum::<u32>();
        let isk = |f: &[&FitLine]| f.iter().map(|l| l.cost.isk * l.short() as f64).sum::<f64>();
        let Some((i, fits)) =
            wants.iter().max_by(|a, b| units(&a.1).cmp(&units(&b.1)).then(isk(&a.1).total_cmp(&isk(&b.1))).then(b.0.cmp(&a.0)))
        else {
            continue;
        };
        for l in fits {
            out.push(Purchase { fit: l.fit, doctrine: lines[*i].doctrine, wanted_by: wants.len(), quantity: l.short(), cost: l.cost.clone() });
        }
    }
    out
}

/// Characters of one account (or of none).
#[derive(Clone, Debug)]
pub struct AccountInventory<'a> {
    /// None for characters not assigned to an account.
    pub account: Option<String>,
    pub name: String,
    pub characters: Vec<CharacterInventory<'a>>,
}

impl AccountInventory<'_> {
    pub fn cost(&self) -> f64 {
        self.characters.iter().map(|c| c.cost()).sum()
    }
}

/// Every character (with skills loaded) against doctrines, by account: accounts in name order, characters without
/// one last, characters by name inside.
pub fn inventory<'a>(
    chars: &[Arc<Character>],
    accounts: &AccountsDoc,
    doctrines: &[(Vec<&'a str>, &'a Doctrine)],
    targets: &Targets,
    prices: &HashMap<i64, f64>,
    opts: Options,
) -> Vec<AccountInventory<'a>> {
    let mut groups: Vec<AccountInventory<'a>> =
        accounts.list().into_iter().map(|a| AccountInventory { account: Some(a.id), name: a.name, characters: Vec::new() }).collect();
    let mut none = AccountInventory { account: None, name: "No account".into(), characters: Vec::new() };
    for c in chars {
        let Some(inv) = character_inventory(c, doctrines, targets, prices, opts) else { continue };
        let at = accounts.account_of(c.id).and_then(|a| groups.iter().position(|g| g.account.as_deref() == Some(a)));
        match at {
            Some(i) => groups[i].characters.push(inv),
            None => none.characters.push(inv),
        }
    }
    groups.push(none);
    groups.retain(|g| !g.characters.is_empty());
    for g in &mut groups {
        g.characters.sort_by(|a, b| super::accounts::natural_cmp(&a.name, &b.name));
    }
    groups
}

/// Totals over everyone.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    pub characters: usize,
    pub flyable: usize,
    /// Ships wanted (the targets), in stock, of those fitted as the doctrine, and short.
    pub target: u32,
    pub stock: u32,
    pub fitted: u32,
    pub short: u32,
    /// Ships on the shopping lists, and what buying them costs (only characters with assets loaded).
    pub to_buy: u32,
    pub cost: f64,
    /// Ships bought as full fits, and as hulls.
    pub fit_priced: u32,
    pub hull_priced: u32,
    /// Purchases with an item that has no price.
    pub partly_priced: usize,
}

pub fn totals(groups: &[AccountInventory]) -> Totals {
    let mut t = Totals::default();
    for c in groups.iter().flat_map(|g| &g.characters).filter(|c| c.assets_loaded) {
        t.characters += 1;
        t.flyable += c.flyable();
        t.target += c.target();
        t.stock += c.stock();
        t.fitted += c.doctrines.iter().flat_map(|d| &d.flyable).map(|l| l.fitted).sum::<u32>();
        t.short += c.short();
        t.to_buy += c.ships_to_buy();
        t.cost += c.cost();
        for p in &c.to_buy {
            match p.cost.basis {
                Basis::Fit => t.fit_priced += p.quantity,
                Basis::Hull => t.hull_priced += p.quantity,
            }
            t.partly_priced += !p.cost.complete() as usize;
        }
    }
    t
}

// ---------- shopping list export ----------

/// One line of a shopping list as items to buy.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemLine {
    pub type_id: i64,
    pub name: String,
    pub quantity: u64,
    /// Ships first, as the game's fitting windows list them.
    pub is_ship: bool,
}

/// Every item the purchases add up to: hulls, and for full fits their modules, charges and drones, each type once.
pub fn items<'a>(purchases: impl IntoIterator<Item = &'a Purchase<'a>>) -> Vec<ItemLine> {
    let mut by_type: BTreeMap<i64, ItemLine> = BTreeMap::new();
    let mut add = |type_id: i64, name: &str, n: u64, is_ship: bool| {
        by_type.entry(type_id).or_insert_with(|| ItemLine { type_id, name: name.to_string(), quantity: 0, is_ship }).quantity += n;
    };
    for p in purchases {
        let n = p.quantity as u64;
        match p.cost.basis {
            Basis::Fit => {
                for (i, it) in p.fit.items.iter().enumerate() {
                    add(it.type_id, &it.name, it.quantity.max(1) as u64 * n, i == 0);
                }
            }
            Basis::Hull => add(p.fit.ship_id, &p.fit.ship_name, n, true),
        }
    }
    let mut v: Vec<ItemLine> = by_type.into_values().collect();
    v.sort_by(|a, b| b.is_ship.cmp(&a.is_ship).then_with(|| a.name.cmp(&b.name)));
    v
}

/// Items as the game's Multibuy window takes them: "Name x Quantity", one per line.
pub fn multibuy(items: &[ItemLine]) -> String {
    items.iter().map(|i| format!("{} x{}\n", i.name, i.quantity)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::assets::Asset;
    use crate::domains::skills::{Skill, SkillsData};
    use crate::features::accounts::AccountInfo;
    use crate::features::doctrines::FitItem;
    use crate::universe::Place;

    const RIFTER: i64 = 587;
    const SLASHER: i64 = 585;
    const DISRUPTOR: i64 = 3244;
    const JITA: i64 = 60003760;

    fn item(id: i64, name: &str, cat: &str, q: u32, skills: &[(i64, u8)]) -> FitItem {
        FitItem { type_id: id, name: name.into(), category: cat.into(), quantity: q, skills: skills.to_vec() }
    }

    /// A Minmatar frigate fit; its module needs Minmatar Frigate at `module_level` (a stand-in requirement).
    fn fit(id: &str, ship: i64, module_level: Option<u8>) -> DoctrineFit {
        let name = if ship == RIFTER { "Rifter" } else { "Slasher" };
        let mut items = vec![item(ship, name, "Ship", 1, &[(3329, 1)])];
        if let Some(l) = module_level {
            items.push(item(DISRUPTOR, "Warp Disruptor II", "Module", 1, &[(3329, l)]));
            items.push(item(178, "EMP S", "Charge", 100, &[]));
        }
        DoctrineFit { id: id.into(), name: id.into(), ship_id: ship, ship_name: name.into(), items, ..Default::default() }
    }

    fn doctrine(id: &str, fits: Vec<DoctrineFit>) -> Doctrine {
        Doctrine { id: id.into(), name: id.into(), fits, ..Default::default() }
    }

    fn asset(item_id: i64, type_id: i64, parent: Option<i64>, flag: &str, singleton: bool, quantity: i64) -> Asset {
        Asset {
            item_id,
            type_id,
            name: format!("Type {type_id}"),
            quantity,
            flag: flag.into(),
            parent_id: parent,
            root_location_id: JITA,
            is_singleton: singleton,
            ..Default::default()
        }
    }

    fn pilot(id: i64, frigate: u8, assets: Option<Vec<Asset>>) -> Arc<Character> {
        let mut c = Character::new(id);
        c.skills = Some(SkillsData {
            skills: [(3327, 1), (3329, frigate)]
                .iter()
                .map(|&(s, l)| Skill { skill_id: s, trained_skill_level: l, active_skill_level: l, ..Default::default() })
                .collect(),
            ..Default::default()
        });
        c.assets = assets.map(|items| {
            let mut d = AssetsData { items, ..Default::default() };
            d.locations.insert(JITA, Place { id: JITA, name: "Jita IV - Moon 4".into(), ..Default::default() });
            d
        });
        Arc::new(c)
    }

    fn prices() -> HashMap<i64, f64> {
        [(RIFTER, 500_000.0), (SLASHER, 400_000.0), (DISRUPTOR, 1_000_000.0), (178, 10.0)].into_iter().collect()
    }

    #[test]
    fn finds_ships_and_what_is_fitted() {
        let data = AssetsData {
            items: vec![
                asset(1, RIFTER, None, "Hangar", true, 1),
                asset(2, DISRUPTOR, Some(1), "MedSlot0", true, 1),
                asset(3, 178, Some(1), "Cargo", false, 100),
                asset(4, SLASHER, None, "Hangar", false, 3),
                asset(5, 34, None, "Hangar", false, 1000),
            ],
            ..Default::default()
        };
        let ships = owned_ships(&data);
        assert_eq!(ships.len(), 2);
        assert_eq!(ships[0].fitted, BTreeMap::from([(DISRUPTOR, 1)]), "cargo isn't fitted");
        assert_eq!((ships[1].packaged, ships[1].count), (true, 3));
        assert_eq!(ships[0].location, "Location #60003760");
        let f = fit("tackle", RIFTER, Some(1));
        assert_eq!(fit_modules(&f), BTreeMap::from([(DISRUPTOR, 1)]), "charges aren't slotted");
        assert!(fitted_as(&ships[0], &f));
        assert!(!fitted_as(&ships[1], &fit("s", SLASHER, Some(1))), "packaged ships hold no modules");
        assert!(!fitted_as(&ships[1], &fit("s", SLASHER, None)), "nor are they ready to fly as a hull-only fit");
    }

    #[test]
    fn prices_fits_and_hulls() {
        let full = fit_cost(&fit("t", RIFTER, Some(1)), &prices());
        assert_eq!((full.basis, full.isk), (Basis::Fit, 500_000.0 + 1_000_000.0 + 1_000.0));
        let hull = fit_cost(&fit("h", SLASHER, None), &prices());
        assert_eq!((hull.basis, hull.isk), (Basis::Hull, 400_000.0));
        let partial = fit_cost(&fit("t", RIFTER, Some(1)), &HashMap::from([(RIFTER, 1.0)]));
        assert_eq!(partial.unpriced, ["Warp Disruptor II", "EMP S"]);
        assert!(!partial.complete());
        assert_eq!(types_to_price([&fit("t", RIFTER, Some(1)), &fit("h", SLASHER, None)]), [178, SLASHER, RIFTER, DISRUPTOR]);
    }


    fn one_each() -> Targets {
        Targets::default()
    }

    #[test]
    fn the_most_specific_target_wins() {
        let mut t = Targets::default();
        assert_eq!(t.resolve(Some(1), Some("d"), Some("f")), (DEFAULT_TARGET, Source::Default));
        t.set(None, None, None, Some(2));
        t.set(None, Some("d"), None, Some(3));
        assert_eq!(t.resolve(Some(1), Some("d"), Some("f")), (3, Source::Everyone(Level::Doctrine)));
        assert_eq!(t.resolve(Some(1), Some("other"), Some("f")), (2, Source::Everyone(Level::Overall)));
        t.set(None, Some("d"), Some("f"), Some(5));
        assert_eq!(t.target(1, "d", "f"), 5);
        assert_eq!(t.target(1, "d", "g"), 3);
        // anything set for the character beats everyone's, even everyone's fit
        t.set(Some(1), None, None, Some(0));
        assert_eq!(t.resolve(Some(1), Some("d"), Some("f")), (0, Source::Character(Level::Overall)));
        assert_eq!(t.target(2, "d", "f"), 5, "other characters keep everyone's");
        t.set(Some(1), Some("d"), None, Some(4));
        t.set(Some(1), Some("d"), Some("f"), Some(6));
        assert_eq!((t.target(1, "d", "f"), t.target(1, "d", "g"), t.target(1, "x", "y")), (6, 4, 0));
        // the broader places show what they'd give
        assert_eq!(t.resolve(Some(1), Some("d"), None), (4, Source::Character(Level::Doctrine)));
        assert_eq!(t.resolve(None, Some("d"), None), (3, Source::Everyone(Level::Doctrine)));
        assert_eq!((t.exact(Some(1), Some("d"), Some("g")), t.exact(Some(1), Some("d"), None)), (None, Some(4)));
        // clearing goes back to inheriting, and an emptied character is dropped
        t.set(Some(1), Some("d"), Some("f"), None);
        assert_eq!(t.target(1, "d", "f"), 4);
        t.clear_character(1);
        assert_eq!(t.target(1, "d", "f"), 5);
        t.set(Some(2), None, None, Some(1));
        t.set(Some(2), None, None, None);
        assert!(t.characters.is_empty());
        let back: Targets = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn stock_against_one_of_each() {
        // 5 fits; the pilot (Minmatar Frigate 2) flies the three needing level ≤ 2
        let d7 = doctrine(
            "d7",
            vec![fit("a", RIFTER, Some(1)), fit("b", RIFTER, Some(2)), fit("c", SLASHER, None), fit("d", RIFTER, Some(4)), fit("e", SLASHER, Some(5))],
        );
        let docs = vec![(vec!["B"], &d7)];
        let poor = pilot(1, 2, Some(vec![]));
        let inv = character_inventory(&poor, &docs, &one_each(), &prices(), Options { hull_only: false }).unwrap();
        let line = &inv.doctrines[0];
        assert_eq!((line.flyable.len(), line.fits(), line.stock(), line.short()), (3, 5, 0, 3), "flies 3 of 5, has none");
        assert_eq!(inv.cost(), 2.0 * 1_501_000.0 + 400_000.0);

        // one fitted Rifter, one packaged Rifter: the fitted one goes to the fit it matches
        let rich = pilot(
            2,
            2,
            Some(vec![
                asset(10, RIFTER, None, "Hangar", false, 1),
                asset(11, RIFTER, None, "Hangar", true, 1),
                asset(12, DISRUPTOR, Some(11), "MedSlot0", true, 1),
            ]),
        );
        let inv = character_inventory(&rich, &docs, &one_each(), &prices(), Options { hull_only: false }).unwrap();
        let h: Vec<Holding> = inv.doctrines[0].flyable.iter().map(|l| l.holding()).collect();
        assert_eq!(h, [Holding::Fitted, Holding::Hulls, Holding::Short]);
        assert_eq!(inv.doctrines[0].flyable[0].ships[0].item_id, 11);
        assert_eq!(inv.doctrines[0].flyable[0].ships[0].location, "Jita IV - Moon 4");
        assert_eq!(inv.to_buy.len(), 1);
        assert_eq!((inv.to_buy[0].cost.basis, inv.to_buy[0].quantity), (Basis::Hull, 1));

        // hull-only fits count when asked
        let inv = character_inventory(&poor, &docs, &one_each(), &prices(), Options { hull_only: true }).unwrap();
        assert_eq!(inv.doctrines[0].flyable.len(), 5);
        assert_eq!(inv.doctrines[0].flyable[3].readiness, Readiness::HullOnly);
    }

    #[test]
    fn targets_count_ships_and_buy_the_rest() {
        let d = doctrine("d", vec![fit("a", RIFTER, Some(1)), fit("b", RIFTER, None), fit("s", SLASHER, None)]);
        let docs = vec![(vec!["G"], &d)];
        let mut t = Targets::default();
        t.set(None, Some("d"), Some("a"), Some(3));
        t.set(None, Some("d"), Some("s"), Some(0));
        // a stack of 2 packaged Rifters and an assembled one, which is fit b as it is (b lists no modules): fit a
        // gets the stack
        let c = pilot(
            1,
            1,
            Some(vec![asset(1, RIFTER, None, "Hangar", false, 2), asset(2, RIFTER, None, "Hangar", true, 1), asset(3, SLASHER, None, "Hangar", true, 1)]),
        );
        let inv = character_inventory(&c, &docs, &t, &prices(), Options { hull_only: false }).unwrap();
        let l = &inv.doctrines[0].flyable;
        assert_eq!(l.iter().map(|l| (l.target, l.stock())).collect::<Vec<_>>(), [(3, 2), (1, 1), (0, 0)]);
        assert_eq!(l[0].ships.iter().map(|s| (s.item_id, s.count)).collect::<Vec<_>>(), [(1, 2)]);
        assert_eq!(l[2].holding(), Holding::NotStocked);
        assert_eq!((inv.target(), inv.stock(), inv.short()), (4, 3, 1));
        assert_eq!(inv.to_buy.iter().map(|p| (p.fit.id.as_str(), p.quantity)).collect::<Vec<_>>(), [("a", 1)]);
        assert_eq!(inv.cost(), 1_501_000.0);

        // a target for this character only
        t.set(Some(1), Some("d"), None, Some(2));
        let inv = character_inventory(&c, &docs, &t, &prices(), Options { hull_only: false }).unwrap();
        assert_eq!(inv.doctrines[0].flyable.iter().map(|l| l.target).collect::<Vec<_>>(), [2, 2, 2]);
        assert_eq!((inv.stock(), inv.short()), (4, 2), "three Rifters and the Slasher");
    }

    #[test]
    fn shared_hulls_are_bought_once() {
        // two doctrines want Rifters: one wants two, the other one; owning none, two Rifters are bought
        let d1 = doctrine("d1", vec![fit("a", RIFTER, Some(1)), fit("b", RIFTER, None)]);
        let d2 = doctrine("d2", vec![fit("c", RIFTER, Some(1)), fit("s", SLASHER, None)]);
        let docs = vec![(vec!["G"], &d1), (vec!["G"], &d2)];
        let c = pilot(1, 1, Some(vec![]));
        let inv = character_inventory(&c, &docs, &one_each(), &prices(), Options { hull_only: false }).unwrap();
        assert_eq!(inv.short(), 4);
        let bought: Vec<(&str, &str, usize)> = inv.to_buy.iter().map(|p| (p.fit.id.as_str(), p.doctrine.id.as_str(), p.wanted_by)).collect();
        assert_eq!(bought, [("s", "d2", 1), ("a", "d1", 2), ("b", "d1", 2)]);
        assert_eq!(inv.cost(), 400_000.0 + 1_501_000.0 + 500_000.0);
        assert!(inv.cost() < inv.doctrines.iter().map(|d| d.cost()).sum::<f64>());

        // one Rifter owned covers d2's Rifter fit and one of d1's
        let c = pilot(2, 1, Some(vec![asset(1, RIFTER, None, "Hangar", false, 1)]));
        let inv = character_inventory(&c, &docs, &one_each(), &prices(), Options { hull_only: false }).unwrap();
        assert_eq!(inv.stock(), 2);
        assert_eq!(inv.to_buy.iter().map(|p| p.fit.id.as_str()).collect::<Vec<_>>(), ["s", "b"]);

        // d2 wanting three of its Rifter fit now decides: three bought for it, none for d1 on top
        let mut t = Targets::default();
        t.set(None, Some("d2"), Some("c"), Some(3));
        let c = pilot(3, 1, Some(vec![]));
        let inv = character_inventory(&c, &docs, &t, &prices(), Options { hull_only: false }).unwrap();
        let bought: Vec<(&str, u32)> = inv.to_buy.iter().map(|p| (p.fit.id.as_str(), p.quantity)).collect();
        assert_eq!(bought, [("s", 1), ("c", 3)]);
    }

    #[test]
    fn shopping_list_as_items_and_multibuy() {
        let d = doctrine("d", vec![fit("a", RIFTER, Some(1)), fit("s", SLASHER, None)]);
        let docs = vec![(vec!["G"], &d)];
        let mut t = Targets::default();
        t.set(None, None, None, Some(2));
        let a = pilot(1, 1, Some(vec![]));
        let b = pilot(2, 1, Some(vec![asset(1, SLASHER, None, "Hangar", false, 1)]));
        let ia = character_inventory(&a, &docs, &t, &prices(), Options { hull_only: false }).unwrap();
        let ib = character_inventory(&b, &docs, &t, &prices(), Options { hull_only: false }).unwrap();
        let all = items(ia.to_buy.iter().chain(&ib.to_buy));
        let got: Vec<(&str, u64)> = all.iter().map(|i| (i.name.as_str(), i.quantity)).collect();
        // ships first: 4 Rifters (2 each), 3 Slashers (2 + 1); then two disruptors and 100 charges per Rifter
        assert_eq!(got, [("Rifter", 4), ("Slasher", 3), ("EMP S", 400), ("Warp Disruptor II", 4)]);
        assert_eq!(multibuy(&all[..2]), "Rifter x4\nSlasher x3\n");
    }

    #[test]
    fn groups_by_account_and_totals() {
        let d = doctrine("d", vec![fit("a", RIFTER, Some(1)), fit("s", SLASHER, None)]);
        let docs = vec![(vec!["G"], &d)];
        let mut accounts = AccountsDoc::default();
        accounts.accounts.insert("b".into(), AccountInfo { name: "Account B".into() });
        accounts.assignments.insert(3, "b".into());
        accounts.assignments.insert(1, "gone".into());
        let mut unskilled = Character::new(4);
        unskilled.assets = Some(AssetsData::default());
        let chars = vec![
            pilot(1, 1, Some(vec![asset(1, SLASHER, None, "Hangar", false, 1)])),
            pilot(2, 1, None),
            pilot(3, 1, Some(vec![])),
            Arc::new(unskilled),
        ];
        let groups = inventory(&chars, &accounts, &docs, &one_each(), &prices(), Options { hull_only: false });
        let shape: Vec<(&str, Vec<i64>)> = groups.iter().map(|g| (g.name.as_str(), g.characters.iter().map(|c| c.character_id).collect())).collect();
        assert_eq!(shape, [("Account B", vec![3]), ("No account", vec![1, 2])], "unknown accounts and missing skills");
        assert_eq!(groups[0].cost(), 1_501_000.0 + 400_000.0);
        let no_assets = &groups[1].characters[1];
        assert!(!no_assets.assets_loaded && no_assets.to_buy.is_empty());
        let t = totals(&groups);
        assert_eq!(
            t,
            Totals {
                characters: 2,
                flyable: 4,
                target: 4,
                stock: 1,
                fitted: 0,
                short: 3,
                to_buy: 3,
                cost: 2.0 * 1_501_000.0 + 400_000.0,
                fit_priced: 2,
                hull_priced: 1,
                partly_priced: 0
            }
        );
    }
}
