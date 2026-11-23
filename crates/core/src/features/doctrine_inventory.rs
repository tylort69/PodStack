//! Doctrine ship inventory: for each character and doctrine, the fits it can fly, which of those it already owns a
//! ship for (from its assets, by hull type, and whether that ship is fitted as the doctrine says), where those ships
//! are, and what the missing ones cost to buy. Characters are grouped by account, the player's own grouping.
//!
//! Ownership is worked out per doctrine: inside one doctrine each fit needs its own ship (two Ishtar fits need two
//! Ishtars), but the same ship counts for every doctrine it fits, since a pilot flies one doctrine at a time. The
//! shopping list follows that: a hull missing for several doctrines is bought once per character, as many as the
//! doctrine needing the most of them.
//!
//! Prices are per unit and come from the caller (the page uses Jita's lowest sell orders). A missing fit that lists
//! modules is priced as the whole fit (hull, modules, charges, drones); a fit that's only a hull is priced as the hull.

use super::accounts::AccountsDoc;
use super::doctrines::{self, Doctrine, DoctrineFit, Readiness};
use crate::character::Character;
use crate::domains::assets::AssetsData;
use crate::sde::SHIPS;
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

/// How a character holds a doctrine fit it can fly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Holding {
    /// No ship of that hull.
    Missing,
    /// The hull, packaged or fitted some other way.
    Hull,
    /// The hull with the fit's modules on it.
    Fitted,
}

impl Holding {
    pub fn label(self) -> &'static str {
        match self {
            Holding::Fitted => "Owned, fitted",
            Holding::Hull => "Owned hull",
            Holding::Missing => "Missing",
        }
    }
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

/// A doctrine fit a character can fly, and how it holds it.
#[derive(Clone, Debug)]
pub struct FitLine<'a> {
    pub fit: &'a DoctrineFit,
    pub readiness: Readiness,
    pub holding: Holding,
    /// The ship counted for it (when owned).
    pub ship: Option<OwnedShip>,
    /// What buying it costs (when missing).
    pub cost: Option<Cost>,
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
    pub fn owned(&self) -> usize {
        self.flyable.iter().filter(|l| l.holding != Holding::Missing).count()
    }
    pub fn missing(&self) -> impl Iterator<Item = &FitLine<'a>> {
        self.flyable.iter().filter(|l| l.holding == Holding::Missing)
    }
    /// What buying every missing flyable fit of this doctrine costs.
    pub fn cost(&self) -> f64 {
        self.missing().filter_map(|l| l.cost.as_ref()).map(|c| c.isk).sum()
    }
}

/// Something a character still has to buy (see the module docs for how doctrines share hulls).
#[derive(Clone, Debug)]
pub struct Purchase<'a> {
    pub fit: &'a DoctrineFit,
    /// The doctrine this purchase is counted for.
    pub doctrine: &'a Doctrine,
    /// How many doctrines want this hull (and fly it missing).
    pub wanted_by: usize,
    pub cost: Cost,
}

/// One character against every doctrine.
#[derive(Clone, Debug)]
pub struct CharacterInventory<'a> {
    pub character_id: i64,
    pub name: String,
    /// Without assets, nothing can be said about ownership: such a character is left out of totals.
    pub assets_loaded: bool,
    pub doctrines: Vec<DoctrineLine<'a>>,
    pub to_buy: Vec<Purchase<'a>>,
}

impl CharacterInventory<'_> {
    pub fn flyable(&self) -> usize {
        self.doctrines.iter().map(|d| d.flyable.len()).sum()
    }
    pub fn owned(&self) -> usize {
        self.doctrines.iter().map(|d| d.owned()).sum()
    }
    pub fn missing(&self) -> usize {
        self.doctrines.iter().map(|d| d.missing().count()).sum()
    }
    /// Doctrines with at least one fit it can fly.
    pub fn doctrines_flyable(&self) -> usize {
        self.doctrines.iter().filter(|d| !d.flyable.is_empty()).count()
    }
    /// What buying everything in [`Self::to_buy`] costs.
    pub fn cost(&self) -> f64 {
        self.to_buy.iter().map(|p| p.cost.isk).sum()
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
                ok.then_some(FitLine { fit: f, readiness: r, holding: Holding::Missing, ship: None, cost: None })
            })
            .collect();
        allocate(&mut flyable, &ships);
        for l in flyable.iter_mut().filter(|l| l.holding == Holding::Missing) {
            l.cost = Some(fit_cost(l.fit, prices));
        }
        lines.push(DoctrineLine { path: path.clone(), doctrine: d, flyable });
    }
    let to_buy = if c.assets.is_some() { shopping_list(&lines) } else { Vec::new() };
    Some(CharacterInventory { character_id: c.id, name: c.name(), assets_loaded: c.assets.is_some(), doctrines: lines, to_buy })
}

/// Gives each fit of one doctrine its own ship: first ships fitted as that fit, then any remaining hull of the type.
fn allocate(lines: &mut [FitLine<'_>], ships: &[OwnedShip]) {
    let mut left: Vec<u32> = ships.iter().map(|s| s.count).collect();
    for pass in [Holding::Fitted, Holding::Hull] {
        for l in lines.iter_mut().filter(|l| l.holding == Holding::Missing) {
            let found = ships
                .iter()
                .enumerate()
                .filter(|(i, s)| left[*i] > 0 && s.type_id == l.fit.ship_id)
                // the hull pass prefers assembled ships (ready to undock after a refit) over packaged ones
                .filter(|(_, s)| pass == Holding::Hull || fitted_as(s, l.fit))
                .min_by_key(|(_, s)| s.packaged)
                .map(|(i, _)| i);
            if let Some(i) = found {
                left[i] -= 1;
                l.holding = pass;
                l.ship = Some(ships[i].clone());
            }
        }
    }
}

/// Per hull, the doctrine missing the most of it (the priciest of those on a tie) decides what's bought.
fn shopping_list<'a>(lines: &[DoctrineLine<'a>]) -> Vec<Purchase<'a>> {
    // hull → doctrine index → missing fits of that hull
    let mut by_hull: BTreeMap<i64, Vec<(usize, Vec<&FitLine<'a>>)>> = BTreeMap::new();
    for (i, d) in lines.iter().enumerate() {
        let mut per: BTreeMap<i64, Vec<&FitLine<'a>>> = BTreeMap::new();
        for l in d.missing() {
            per.entry(l.fit.ship_id).or_default().push(l);
        }
        for (hull, fits) in per {
            by_hull.entry(hull).or_default().push((i, fits));
        }
    }
    let mut out = Vec::new();
    for wants in by_hull.into_values() {
        let isk = |f: &[&FitLine]| f.iter().filter_map(|l| l.cost.as_ref()).map(|c| c.isk).sum::<f64>();
        let Some((i, fits)) = wants.iter().max_by(|a, b| a.1.len().cmp(&b.1.len()).then(isk(&a.1).total_cmp(&isk(&b.1))).then(b.0.cmp(&a.0))) else {
            continue;
        };
        for l in fits {
            out.push(Purchase { fit: l.fit, doctrine: lines[*i].doctrine, wanted_by: wants.len(), cost: l.cost.clone().unwrap_or_else(|| fit_cost(l.fit, &HashMap::new())) });
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
    prices: &HashMap<i64, f64>,
    opts: Options,
) -> Vec<AccountInventory<'a>> {
    let mut groups: Vec<AccountInventory<'a>> =
        accounts.list().into_iter().map(|a| AccountInventory { account: Some(a.id), name: a.name, characters: Vec::new() }).collect();
    let mut none = AccountInventory { account: None, name: "No account".into(), characters: Vec::new() };
    for c in chars {
        let Some(inv) = character_inventory(c, doctrines, prices, opts) else { continue };
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
    pub owned: usize,
    pub fitted: usize,
    pub missing: usize,
    /// What buying everything missing costs (only characters with assets loaded).
    pub cost: f64,
    /// Purchases priced as full fits, and as hulls.
    pub fit_priced: usize,
    pub hull_priced: usize,
    /// Purchases with an item that has no price.
    pub partly_priced: usize,
}

pub fn totals(groups: &[AccountInventory]) -> Totals {
    let mut t = Totals::default();
    for c in groups.iter().flat_map(|g| &g.characters).filter(|c| c.assets_loaded) {
        t.characters += 1;
        t.flyable += c.flyable();
        t.owned += c.owned();
        t.fitted += c.doctrines.iter().flat_map(|d| &d.flyable).filter(|l| l.holding == Holding::Fitted).count();
        t.missing += c.missing();
        t.cost += c.cost();
        for p in &c.to_buy {
            match p.cost.basis {
                Basis::Fit => t.fit_priced += 1,
                Basis::Hull => t.hull_priced += 1,
            }
            t.partly_priced += !p.cost.complete() as usize;
        }
    }
    t
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

    #[test]
    fn owned_versus_missing_per_doctrine() {
        // 5 fits; the pilot (Minmatar Frigate 2) flies the three needing level ≤ 2
        let d7 = doctrine(
            "d7",
            vec![fit("a", RIFTER, Some(1)), fit("b", RIFTER, Some(2)), fit("c", SLASHER, None), fit("d", RIFTER, Some(4)), fit("e", SLASHER, Some(5))],
        );
        let docs = vec![(vec!["B"], &d7)];
        let poor = pilot(1, 2, Some(vec![]));
        let inv = character_inventory(&poor, &docs, &prices(), Options { hull_only: false }).unwrap();
        let line = &inv.doctrines[0];
        assert_eq!((line.flyable.len(), line.fits(), line.owned()), (3, 5, 0), "flies 3 of 5, owns none of them");
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
        let inv = character_inventory(&rich, &docs, &prices(), Options { hull_only: false }).unwrap();
        let h: Vec<Holding> = inv.doctrines[0].flyable.iter().map(|l| l.holding).collect();
        assert_eq!(h, [Holding::Fitted, Holding::Hull, Holding::Missing]);
        assert_eq!(inv.doctrines[0].flyable[0].ship.as_ref().map(|s| s.item_id), Some(11));
        assert_eq!(inv.doctrines[0].flyable[0].ship.as_ref().map(|s| s.location.as_str()), Some("Jita IV - Moon 4"));
        assert_eq!(inv.to_buy.len(), 1);
        assert_eq!(inv.to_buy[0].cost.basis, Basis::Hull);

        // hull-only fits count when asked
        let inv = character_inventory(&poor, &docs, &prices(), Options { hull_only: true }).unwrap();
        assert_eq!(inv.doctrines[0].flyable.len(), 5);
        assert_eq!(inv.doctrines[0].flyable[3].readiness, Readiness::HullOnly);
    }

    #[test]
    fn shared_hulls_are_bought_once() {
        // two doctrines want Rifters: one wants two, the other one; owning none, two Rifters are bought
        let d1 = doctrine("d1", vec![fit("a", RIFTER, Some(1)), fit("b", RIFTER, None)]);
        let d2 = doctrine("d2", vec![fit("c", RIFTER, Some(1)), fit("s", SLASHER, None)]);
        let docs = vec![(vec!["G"], &d1), (vec!["G"], &d2)];
        let c = pilot(1, 1, Some(vec![]));
        let inv = character_inventory(&c, &docs, &prices(), Options { hull_only: false }).unwrap();
        assert_eq!(inv.missing(), 4);
        let bought: Vec<(&str, &str, usize)> = inv.to_buy.iter().map(|p| (p.fit.id.as_str(), p.doctrine.id.as_str(), p.wanted_by)).collect();
        assert_eq!(bought, [("s", "d2", 1), ("a", "d1", 2), ("b", "d1", 2)]);
        assert_eq!(inv.cost(), 400_000.0 + 1_501_000.0 + 500_000.0);
        assert!(inv.cost() < inv.doctrines.iter().map(|d| d.cost()).sum::<f64>());

        // one Rifter owned covers d2's Rifter fit and one of d1's
        let c = pilot(2, 1, Some(vec![asset(1, RIFTER, None, "Hangar", false, 1)]));
        let inv = character_inventory(&c, &docs, &prices(), Options { hull_only: false }).unwrap();
        assert_eq!(inv.owned(), 2);
        assert_eq!(inv.to_buy.iter().map(|p| p.fit.id.as_str()).collect::<Vec<_>>(), ["s", "b"]);
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
        let groups = inventory(&chars, &accounts, &docs, &prices(), Options { hull_only: false });
        let shape: Vec<(&str, Vec<i64>)> = groups.iter().map(|g| (g.name.as_str(), g.characters.iter().map(|c| c.character_id).collect())).collect();
        assert_eq!(shape, [("Account B", vec![3]), ("No account", vec![1, 2])], "unknown accounts and missing skills");
        assert_eq!(groups[0].cost(), 1_501_000.0 + 400_000.0);
        let no_assets = &groups[1].characters[1];
        assert!(!no_assets.assets_loaded && no_assets.to_buy.is_empty());
        let t = totals(&groups);
        assert_eq!(
            t,
            Totals { characters: 2, flyable: 4, owned: 1, fitted: 0, missing: 3, cost: 2.0 * 1_501_000.0 + 400_000.0, fit_priced: 2, hull_priced: 1, partly_priced: 0 }
        );
    }
}
