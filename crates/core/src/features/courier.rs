//! Courier contracts: what the user's characters are hauling and shipping now, what hauling has paid over time,
//! courier services' rate cards with a quote calculator, and a finder for public courier contracts.
//!
//! Routes come from the stargate map in CCP's static data (`sde::MAP`). A courier page needs many routes at once (a
//! region's public couriers, a history of hauls), so [`RouteTree`] works out every route from one system in one go
//! and [`tree`] keeps the last few hundred of them.
//!
//! Rate cards and the page's choices are stored in the "courier" document ([`CourierDoc`]).

use crate::character::Character;
use crate::domains::contracts::Contract;
use crate::esi::EsiError;
use crate::features::contracts::{sides, Category, Flow, Mine, Party};
use crate::fmt;
use crate::public_contracts::PublicContract;
use crate::sde::{Map, Route, RoutePref, SecurityClass, MAP};
use crate::universe::Place;
use crate::{store, Core, Event};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

// ---------- routes ----------

/// Every route from one system, as the in-game autopilot would pick them with a route preference.
#[derive(Debug, Clone)]
pub struct RouteTree {
    source: i64,
    prev: HashMap<i64, i64>,
    hops: HashMap<i64, u32>,
}

/// What a jump into a system costs: like `Map::route`, a system the preference avoids costs as much as many
/// ordinary jumps, so it's only used when there's no other way.
fn step_cost(map: &Map, id: i64, pref: RoutePref) -> u64 {
    const AVOID: u64 = 50_000;
    let high = map.system(id).map(|s| s.security_class() == SecurityClass::High).unwrap_or(false);
    match pref {
        RoutePref::Safer if !high => AVOID,
        RoutePref::LessSecure if high => AVOID,
        _ => 1,
    }
}

impl RouteTree {
    /// None when the system isn't in the map.
    pub fn new(map: &Map, source: i64, pref: RoutePref) -> Option<RouteTree> {
        map.system(source)?;
        let mut dist: HashMap<i64, u64> = HashMap::from([(source, 0)]);
        let mut prev: HashMap<i64, i64> = HashMap::new();
        let mut hops: HashMap<i64, u32> = HashMap::from([(source, 0)]);
        let mut heap = std::collections::BinaryHeap::from([std::cmp::Reverse((0u64, source))]);
        while let Some(std::cmp::Reverse((d, at))) = heap.pop() {
            if dist.get(&at).is_some_and(|&best| d > best) {
                continue;
            }
            let h = hops[&at];
            for &next in map.neighbours(at) {
                let nd = d + step_cost(map, next, pref);
                if dist.get(&next).is_none_or(|&best| nd < best) {
                    dist.insert(next, nd);
                    prev.insert(next, at);
                    hops.insert(next, h + 1);
                    heap.push(std::cmp::Reverse((nd, next)));
                }
            }
        }
        Some(RouteTree { source, prev, hops })
    }

    pub fn source(&self) -> i64 {
        self.source
    }

    /// Jumps to `to`, None when no gates lead there.
    pub fn jumps(&self, to: i64) -> Option<usize> {
        self.hops.get(&to).map(|h| *h as usize)
    }

    /// The route to `to`, both ends included.
    pub fn route(&self, to: i64) -> Option<Route> {
        if to == self.source {
            return Some(Route { systems: vec![to] });
        }
        if !self.prev.contains_key(&to) {
            return None;
        }
        let mut systems = vec![to];
        let mut at = to;
        while let Some(&p) = self.prev.get(&at) {
            systems.push(p);
            at = p;
        }
        systems.reverse();
        Some(Route { systems })
    }

    /// Systems within `max` jumps (on this preference's routes).
    pub fn within(&self, max: usize) -> HashSet<i64> {
        self.hops.iter().filter(|(_, h)| **h as usize <= max).map(|(id, _)| *id).collect()
    }
}

/// Route trees worked out so far, by (map build, source, preference). The map's address tells builds apart: a
/// newer static data build swaps in a new map, whose routes may differ.
type TreeKey = (usize, i64, u8);
static TREES: Mutex<Option<HashMap<TreeKey, Arc<RouteTree>>>> = Mutex::new(None);
/// Trees kept at most (each is a few hundred kilobytes).
const MAX_TREES: usize = 256;

fn pref_key(p: RoutePref) -> u8 {
    match p {
        RoutePref::Shortest => 0,
        RoutePref::Safer => 1,
        RoutePref::LessSecure => 2,
    }
}

/// Every route from `source`, from the cache when worked out before.
pub fn tree(source: i64, pref: RoutePref) -> Option<Arc<RouteTree>> {
    let map: &Map = &MAP;
    let key = (map as *const Map as usize, source, pref_key(pref));
    if let Some(t) = TREES.lock().unwrap().as_ref().and_then(|m| m.get(&key)) {
        return Some(t.clone());
    }
    let t = Arc::new(RouteTree::new(map, source, pref)?);
    let mut slot = TREES.lock().unwrap();
    let m = slot.get_or_insert_with(HashMap::new);
    if m.len() >= MAX_TREES {
        m.clear();
    }
    m.insert(key, t.clone());
    Some(t)
}

/// The route between two systems.
pub fn route(from: i64, to: i64, pref: RoutePref) -> Option<Route> {
    tree(from, pref)?.route(to)
}

/// What matters about a route for hauling.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RouteSummary {
    pub jumps: usize,
    /// The lowest security on the way, as EVE shows it.
    pub min_security: f64,
    /// Systems by security class, start included: (high, low, null).
    pub counts: (usize, usize, usize),
}

impl RouteSummary {
    pub fn of(r: &Route) -> RouteSummary {
        RouteSummary { jumps: r.jumps(), min_security: r.min_security().unwrap_or(0.0), counts: r.security_counts() }
    }
    pub fn high_sec(&self) -> bool {
        self.counts.1 == 0 && self.counts.2 == 0
    }
    /// "12 jumps · 0.5" or "12 jumps · 3 low · 2 null".
    pub fn describe(&self) -> String {
        let jumps = format!("{} jump{}", self.jumps, if self.jumps == 1 { "" } else { "s" });
        if self.high_sec() {
            format!("{jumps}, all high-sec")
        } else {
            let mut parts = vec![jumps];
            if self.counts.1 > 0 {
                parts.push(format!("{} low-sec", self.counts.1));
            }
            if self.counts.2 > 0 {
                parts.push(format!("{} null-sec", self.counts.2));
            }
            parts.join(", ")
        }
    }
}

/// The route summary between two systems.
pub fn summary(from: i64, to: i64, pref: RoutePref) -> Option<RouteSummary> {
    route(from, to, pref).map(|r| RouteSummary::of(&r))
}

/// ISK per jump: a same-system courier counts as one jump (it still takes undocking and docking).
pub fn per_jump(isk: f64, jumps: usize) -> f64 {
    isk / jumps.max(1) as f64
}

// ---------- rate cards ----------

/// How a courier service prices a contract. Every part adds up; the minimum applies to the sum.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Pricing {
    pub base: f64,
    pub per_jump: f64,
    pub per_m3: f64,
    pub per_m3_jump: f64,
    /// A share of the collateral (0.01 = 1%).
    pub collateral_rate: f64,
    pub minimum: f64,
    /// Rewards are rounded up to a multiple of this (0: whole ISK).
    pub round_to: f64,
}

/// A fixed price for a route (both ways when `both_ways`).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FlatRoute {
    pub from: i64,
    pub to: i64,
    pub both_ways: bool,
    pub price: f64,
    /// The price is per m³ rather than for the whole contract.
    pub per_m3: bool,
    /// The service's collateral share is added on top.
    pub collateral_fee: bool,
}

impl FlatRoute {
    pub fn serves(&self, from: i64, to: i64) -> bool {
        (self.from == from && self.to == to) || (self.both_ways && self.from == to && self.to == from)
    }
}

/// A courier service's rate card.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Service {
    pub id: String,
    pub name: String,
    /// Who the contract is assigned to (a character or corporation name).
    pub assignee: String,
    /// The contract's description.
    pub description: String,
    pub pricing: Pricing,
    pub max_volume: Option<f64>,
    pub max_collateral: Option<f64>,
    pub max_jumps: Option<usize>,
    /// The route the service flies, for jumps and security.
    pub route: RoutePref,
    /// Only routes that never leave high-sec.
    pub high_sec_only: bool,
    /// Systems it picks up from and delivers to; empty means anywhere.
    pub pickups: Vec<i64>,
    pub deliveries: Vec<i64>,
    pub days_to_complete: i64,
    pub expiration_days: i64,
    pub flat_routes: Vec<FlatRoute>,
}

impl Default for Service {
    fn default() -> Self {
        Service {
            id: String::new(),
            name: String::new(),
            assignee: String::new(),
            description: String::new(),
            pricing: Pricing::default(),
            max_volume: None,
            max_collateral: None,
            max_jumps: None,
            route: RoutePref::Safer,
            high_sec_only: false,
            pickups: vec![],
            deliveries: vec![],
            days_to_complete: 3,
            expiration_days: 7,
            flat_routes: vec![],
        }
    }
}

impl Service {
    pub fn flat_route(&self, from: i64, to: i64) -> Option<&FlatRoute> {
        self.flat_routes.iter().find(|r| r.serves(from, to))
    }
    /// The rate card in one line: "250k + 1.2m/jump + 300/m³ · 1% collateral · min 5m".
    pub fn describe(&self) -> String {
        let p = &self.pricing;
        let mut parts = Vec::new();
        if p.base > 0.0 {
            parts.push(fmt::isk_short(p.base));
        }
        if p.per_jump > 0.0 {
            parts.push(format!("{}/jump", fmt::isk_short(p.per_jump)));
        }
        if p.per_m3 > 0.0 {
            parts.push(format!("{}/m³", fmt::isk_short(p.per_m3)));
        }
        if p.per_m3_jump > 0.0 {
            parts.push(format!("{}/m³/jump", fmt::isk_short(p.per_m3_jump)));
        }
        let mut out = if parts.is_empty() { String::new() } else { parts.join(" + ") };
        let mut extra = Vec::new();
        if p.collateral_rate > 0.0 {
            extra.push(format!("{}% of collateral", percent(p.collateral_rate)));
        }
        if p.minimum > 0.0 {
            extra.push(format!("min {}", fmt::isk_short(p.minimum)));
        }
        if !self.flat_routes.is_empty() {
            extra.push(format!("{} flat route{}", self.flat_routes.len(), if self.flat_routes.len() == 1 { "" } else { "s" }));
        }
        if !extra.is_empty() {
            if !out.is_empty() {
                out.push_str(" · ");
            }
            out.push_str(&extra.join(" · "));
        }
        if out.is_empty() {
            "No pricing yet".into()
        } else {
            out
        }
    }
}

/// A fraction as a percent without trailing zeros: 0.015 → "1.5".
pub fn percent(fraction: f64) -> String {
    let s = format!("{:.4}", fraction * 100.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A limit of the service the contract breaks.
#[derive(Clone, Debug, PartialEq)]
pub enum Problem {
    /// No stargate route between the systems (or a system isn't known).
    NoRoute,
    TooBig { max: f64 },
    TooMuchCollateral { max: f64 },
    TooFar { max: usize },
    LeavesHighSec { min_security: f64 },
    PickupNotServed,
    DeliveryNotServed,
}

impl Problem {
    pub fn label(&self) -> String {
        match self {
            Problem::NoRoute => "No stargate route between these systems".into(),
            Problem::TooBig { max } => format!("Over the {} m³ volume limit", fmt::int(*max as i64)),
            Problem::TooMuchCollateral { max } => format!("Over the {} collateral limit", fmt::isk_short(*max)),
            Problem::TooFar { max } => format!("Longer than the {max} jump limit"),
            Problem::LeavesHighSec { min_security } => format!("The route leaves high-sec (down to {min_security:.1})"),
            Problem::PickupNotServed => "The service doesn't pick up from this system".into(),
            Problem::DeliveryNotServed => "The service doesn't deliver to this system".into(),
        }
    }
}

/// A priced contract.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Quote {
    /// None when the price depends on jumps and there's no route.
    pub reward: Option<f64>,
    /// What the reward is made of: (label, ISK).
    pub parts: Vec<(String, f64)>,
    pub minimum_applied: bool,
    /// The service's flat price for this route was used.
    pub flat: bool,
    pub route: Option<RouteSummary>,
    pub problems: Vec<Problem>,
}

impl Quote {
    pub fn ok(&self) -> bool {
        self.problems.is_empty() && self.reward.is_some()
    }
    pub fn isk_per_jump(&self) -> Option<f64> {
        Some(per_jump(self.reward?, self.route?.jumps))
    }
}

fn round_up(v: f64, to: f64) -> f64 {
    if to > 0.0 {
        (v / to - 1e-9).ceil() * to
    } else {
        v.ceil()
    }
}

/// Prices a contract with the route already worked out (None: no route known).
pub fn quote_with(s: &Service, from: i64, to: i64, volume: f64, collateral: f64, route: Option<RouteSummary>) -> Quote {
    let p = &s.pricing;
    let volume = volume.max(0.0);
    let collateral = collateral.max(0.0);
    let mut q = Quote { route, ..Default::default() };
    let flat = s.flat_route(from, to);
    let mut parts: Vec<(String, f64)> = Vec::new();
    let collateral_part = |parts: &mut Vec<(String, f64)>| {
        if p.collateral_rate > 0.0 && collateral > 0.0 {
            parts.push((format!("{}% of collateral", percent(p.collateral_rate)), p.collateral_rate * collateral));
        }
    };
    let priced = if let Some(f) = flat {
        q.flat = true;
        if f.per_m3 {
            parts.push((format!("Flat rate: {} per m³", fmt::isk_short(f.price)), f.price * volume));
        } else {
            parts.push(("Flat rate for this route".into(), f.price));
        }
        if f.collateral_fee {
            collateral_part(&mut parts);
        }
        true
    } else {
        let needs_jumps = p.per_jump != 0.0 || p.per_m3_jump != 0.0;
        if p.base != 0.0 {
            parts.push(("Base fee".into(), p.base));
        }
        if p.per_m3 != 0.0 {
            parts.push((format!("{} per m³", fmt::isk_short(p.per_m3)), p.per_m3 * volume));
        }
        if let Some(r) = route {
            if p.per_jump != 0.0 {
                parts.push((format!("{} per jump × {}", fmt::isk_short(p.per_jump), r.jumps), p.per_jump * r.jumps as f64));
            }
            if p.per_m3_jump != 0.0 {
                parts.push((format!("{} per m³ per jump", fmt::isk_short(p.per_m3_jump)), p.per_m3_jump * volume * r.jumps as f64));
            }
        }
        collateral_part(&mut parts);
        route.is_some() || !needs_jumps
    };
    if priced {
        let sum: f64 = parts.iter().map(|p| p.1).sum();
        let reward = if sum < p.minimum {
            q.minimum_applied = true;
            p.minimum
        } else {
            sum
        };
        q.reward = Some(round_up(reward, p.round_to));
    }
    q.parts = parts;

    if route.is_none() && flat.is_none() {
        q.problems.push(Problem::NoRoute);
    }
    if let Some(max) = s.max_volume.filter(|m| volume > *m) {
        q.problems.push(Problem::TooBig { max });
    }
    if let Some(max) = s.max_collateral.filter(|m| collateral > *m) {
        q.problems.push(Problem::TooMuchCollateral { max });
    }
    if flat.is_none() {
        // a flat-rate route is one the service flies, whatever the other limits say
        if let (Some(max), Some(r)) = (s.max_jumps, route) {
            if r.jumps > max {
                q.problems.push(Problem::TooFar { max });
            }
        }
        if let Some(r) = route.filter(|r| s.high_sec_only && !r.high_sec()) {
            q.problems.push(Problem::LeavesHighSec { min_security: r.min_security });
        }
        if !s.pickups.is_empty() && !s.pickups.contains(&from) {
            q.problems.push(Problem::PickupNotServed);
        }
        if !s.deliveries.is_empty() && !s.deliveries.contains(&to) {
            q.problems.push(Problem::DeliveryNotServed);
        }
    }
    q
}

/// Prices a contract between two systems, on the service's route preference.
pub fn quote(s: &Service, from: i64, to: i64, volume: f64, collateral: f64) -> Quote {
    quote_with(s, from, to, volume, collateral, summary(from, to, s.route))
}

/// Whole ISK with separators, as EVE's contract window takes it.
fn isk_whole(v: f64) -> String {
    format!("{} ISK", fmt::decimal(v.round(), 0))
}

/// What to fill in when creating the contract in game.
pub fn contract_text(s: &Service, from: &str, to: &str, volume: f64, collateral: f64, q: &Quote) -> String {
    let mut lines = vec!["Contract type: Courier".to_string()];
    let assignee = if s.assignee.trim().is_empty() { "(set an assignee for this service)" } else { s.assignee.trim() };
    lines.push(format!("Availability: Private, assigned to {assignee}"));
    lines.push(format!("Pick up: {from}"));
    lines.push(format!("Ship to: {to}"));
    lines.push(format!("Reward: {}", q.reward.map(isk_whole).unwrap_or_else(|| "(no route: can't price)".into())));
    lines.push(format!("Collateral: {}", isk_whole(collateral)));
    lines.push(format!("Expiration: {} day{}", s.expiration_days, if s.expiration_days == 1 { "" } else { "s" }));
    lines.push(format!("Days to complete: {}", s.days_to_complete));
    if !s.description.trim().is_empty() {
        lines.push(format!("Description: {}", s.description.trim()));
    }
    lines.push(format!("Volume: {} m³", fmt::decimal(volume, 2)));
    if let Some(r) = q.route {
        lines.push(format!("Route: {}", r.describe()));
    }
    lines.join("\n")
}

// ---------- the document ----------

/// A hauler hull's cargo, for the finder's volume filter. Users set these to what their fitted, skilled ships carry.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Hull {
    pub name: String,
    pub capacity: f64,
}

/// Starting points for the volume presets, meant to be edited: cargo depends on fitting and skills.
pub fn default_hulls() -> Vec<Hull> {
    [("Blockade runner", 10_000.0), ("Deep space transport", 60_000.0), ("Jump freighter", 350_000.0), ("Freighter", 1_000_000.0)]
        .into_iter()
        .map(|(n, c)| Hull { name: n.into(), capacity: c })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum FinderSort {
    #[default]
    IskPerJump,
    Reward,
    IskPerM3,
    Nearest,
}

impl FinderSort {
    pub const ALL: [FinderSort; 4] = [FinderSort::IskPerJump, FinderSort::Reward, FinderSort::IskPerM3, FinderSort::Nearest];
    pub fn label(self) -> &'static str {
        match self {
            FinderSort::IskPerJump => "ISK per jump",
            FinderSort::Reward => "Reward",
            FinderSort::IskPerM3 => "ISK per m³",
            FinderSort::Nearest => "Nearest pickup",
        }
    }
}

/// The finder's filters.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FinderPrefs {
    pub region: i64,
    /// Where the hauler is: jumps to the pickup are counted from here.
    pub origin: Option<i64>,
    /// Only pickups within this many jumps of `origin`.
    pub max_pickup_jumps: Option<usize>,
    pub max_volume: Option<f64>,
    pub max_collateral: Option<f64>,
    pub min_reward: Option<f64>,
    pub min_isk_per_jump: Option<f64>,
    pub high_sec_only: bool,
    /// ISK per jump counts the jumps to the pickup too.
    pub count_travel: bool,
    pub sort: FinderSort,
}

impl Default for FinderPrefs {
    fn default() -> Self {
        FinderPrefs {
            region: 10000002,
            origin: None,
            max_pickup_jumps: None,
            max_volume: None,
            max_collateral: None,
            min_reward: None,
            min_isk_per_jump: None,
            high_sec_only: false,
            count_travel: false,
            sort: FinderSort::IskPerJump,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CourierDoc {
    pub services: Vec<Service>,
    pub hulls: Vec<Hull>,
    /// The route preference jumps and security are worked out with on the Active and History views and the finder.
    pub route: RoutePref,
    pub finder: FinderPrefs,
}

impl Default for CourierDoc {
    fn default() -> Self {
        CourierDoc { services: vec![], hulls: default_hulls(), route: RoutePref::Shortest, finder: FinderPrefs::default() }
    }
}

pub const DOC: &str = "courier";

pub fn load_doc() -> CourierDoc {
    store::load_doc(DOC)
}

pub fn save_doc(core: &Core, d: &CourierDoc) {
    store::save_doc(DOC, d);
    core.emit(Event::Doc(DOC.into()));
}

/// Saves the finder's filters without telling the pages: they change as the user types, and a rebuild of the page
/// would take the focus away.
pub fn save_finder(f: &FinderPrefs) {
    let mut d = load_doc();
    if d.finder != *f {
        d.finder = f.clone();
        store::save_doc(DOC, &d);
    }
}

// ---------- active couriers ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    /// One of the user's characters (or corporations) accepted it and is carrying the cargo.
    Hauling,
    /// The user issued it: waiting for a hauler or in transit.
    Shipping,
    /// Someone assigned it to the user, who hasn't accepted it yet.
    Assigned,
}

/// An open courier from one side.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveCourier {
    pub contract: Contract,
    pub role: Role,
    pub party: Party,
    /// When it must be delivered (accepted) or when it expires (not accepted yet).
    pub due: DateTime<Utc>,
}

impl ActiveCourier {
    pub fn accepted(&self) -> bool {
        self.contract.status == "in_progress"
    }
    pub fn from_system(&self) -> Option<i64> {
        self.contract.start_location.as_ref().and_then(|p| p.system_id)
    }
    pub fn to_system(&self) -> Option<i64> {
        self.contract.end_location.as_ref().and_then(|p| p.system_id)
    }
    pub fn reward(&self) -> f64 {
        self.contract.reward.unwrap_or(0.0)
    }
    pub fn collateral(&self) -> f64 {
        self.contract.collateral.unwrap_or(0.0)
    }
    pub fn volume(&self) -> f64 {
        self.contract.volume.unwrap_or(0.0)
    }
}

/// When an open courier must be delivered (`date_accepted + days_to_complete`) or, before anyone accepts it, when
/// it expires.
pub fn due(c: &Contract) -> DateTime<Utc> {
    match (c.status.as_str(), c.date_accepted, c.days_to_complete) {
        ("in_progress", Some(acc), Some(days)) if days > 0 => acc + Duration::days(days),
        _ => c.date_expired,
    }
}

/// Contracts the characters and their corporations list now, each once.
pub fn listed(chars: &[Arc<Character>]) -> Vec<Contract> {
    let mut by_id: BTreeMap<i64, Contract> = BTreeMap::new();
    for ch in chars {
        for data in [&ch.contracts, &ch.corp_contracts].into_iter().flatten() {
            for c in &data.contracts {
                by_id.entry(c.contract_id).or_insert_with(|| c.clone());
            }
        }
    }
    by_id.into_values().collect()
}

/// Open couriers the user is on, soonest due first. A courier between two of the user's own characters shows up
/// both as hauled and as shipped.
pub fn active<'a>(contracts: impl IntoIterator<Item = &'a Contract>, mine: &Mine, now: DateTime<Utc>) -> Vec<ActiveCourier> {
    let mut out = Vec::new();
    for c in contracts {
        if c.kind != "courier" {
            continue;
        }
        let open = match c.status.as_str() {
            "in_progress" => true,
            // ESI keeps listing an unaccepted courier as outstanding for a while after it expires
            "outstanding" => c.date_expired > now,
            _ => false,
        };
        if !open {
            continue;
        }
        let (issuer, acceptor) = sides(c, mine);
        let push = |out: &mut Vec<ActiveCourier>, role, party| out.push(ActiveCourier { contract: c.clone(), role, party, due: due(c) });
        if let (Some(p), "in_progress") = (acceptor, c.status.as_str()) {
            push(&mut out, Role::Hauling, p);
        }
        if let Some(p) = issuer {
            push(&mut out, Role::Shipping, p);
        }
        if c.status == "outstanding" {
            if let Some(id) = c.assignee_id {
                let party = if mine.characters.contains_key(&id) {
                    Some(Party::Character(id))
                } else if mine.corporations.contains_key(&id) {
                    Some(Party::Corporation(id))
                } else {
                    None
                };
                if let Some(p) = party {
                    push(&mut out, Role::Assigned, p);
                }
            }
        }
    }
    out.sort_by_key(|a| (a.due, a.contract.contract_id));
    out
}

/// What one hauler is carrying.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Load {
    pub count: usize,
    pub collateral: f64,
    pub reward: f64,
    pub volume: f64,
    pub next_due: Option<DateTime<Utc>>,
    pub overdue: usize,
}

impl Load {
    fn add(&mut self, a: &ActiveCourier, now: DateTime<Utc>) {
        self.count += 1;
        self.collateral += a.collateral();
        self.reward += a.reward();
        self.volume += a.volume();
        self.next_due = Some(self.next_due.map_or(a.due, |d| d.min(a.due)));
        if a.due <= now {
            self.overdue += 1;
        }
    }
}

/// Load per hauler (couriers being hauled), most collateral first.
pub fn loads(active: &[ActiveCourier], now: DateTime<Utc>) -> Vec<(Party, Load)> {
    let mut m: BTreeMap<Party, Load> = BTreeMap::new();
    for a in active.iter().filter(|a| a.role == Role::Hauling) {
        m.entry(a.party).or_default().add(a, now);
    }
    let mut v: Vec<(Party, Load)> = m.into_iter().collect();
    v.sort_by(|a, b| b.1.collateral.total_cmp(&a.1.collateral));
    v
}

/// The headline numbers of the Active view.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ActiveTotals {
    pub hauling: Load,
    /// The user's couriers someone has accepted.
    pub in_transit: Load,
    /// The user's couriers nobody has accepted yet.
    pub waiting: Load,
    pub assigned: Load,
    /// Couriers being hauled that are due within 24 hours (or overdue).
    pub due_soon: usize,
}

pub fn totals(active: &[ActiveCourier], now: DateTime<Utc>) -> ActiveTotals {
    let mut t = ActiveTotals::default();
    for a in active {
        match a.role {
            Role::Hauling => {
                t.hauling.add(a, now);
                if a.due <= now + Duration::hours(24) {
                    t.due_soon += 1;
                }
            }
            Role::Shipping if a.accepted() => t.in_transit.add(a, now),
            Role::Shipping => t.waiting.add(a, now),
            Role::Assigned => t.assigned.add(a, now),
        }
    }
    t
}

// ---------- history ----------

/// Courier totals over some flows.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct HaulTotals {
    /// Delivered couriers.
    pub count: usize,
    pub reward: f64,
    pub collateral: f64,
    pub volume: f64,
    /// Jumps of the delivered couriers whose route is known, and their reward.
    pub jumps: usize,
    pub reward_with_jumps: f64,
    pub failed: usize,
    /// Collateral lost (hauling) or received (shipping) on failed couriers.
    pub failed_collateral: f64,
}

impl HaulTotals {
    pub fn isk_per_jump(&self) -> Option<f64> {
        (self.reward_with_jumps > 0.0 || self.jumps > 0).then(|| self.reward_with_jumps / self.jumps.max(1) as f64)
    }
    pub fn isk_per_m3(&self) -> Option<f64> {
        (self.volume > 0.0).then(|| self.reward / self.volume)
    }
    /// Failed couriers out of all that ended (delivered or failed).
    pub fn failure_rate(&self) -> Option<f64> {
        let ended = self.count + self.failed;
        (ended > 0).then(|| self.failed as f64 / ended as f64)
    }
    fn add(&mut self, f: &Flow, c: Option<&Contract>, jumps: Option<usize>) {
        let collateral = c.and_then(|c| c.collateral).unwrap_or(0.0);
        if f.category == Category::CourierFailed {
            self.failed += 1;
            self.failed_collateral += collateral;
            return;
        }
        let reward = f.isk_in + f.isk_out;
        self.count += 1;
        self.reward += reward;
        self.collateral += collateral;
        self.volume += f.volume;
        if let Some(j) = jumps {
            self.jumps += j.max(1);
            self.reward_with_jumps += reward;
        }
    }
}

/// One pickup system → delivery system pair.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct RouteStats {
    pub from: i64,
    pub to: i64,
    pub jumps: Option<usize>,
    pub totals: HaulTotals,
}

impl RouteStats {
    pub fn average_reward(&self) -> f64 {
        self.totals.reward / self.totals.count.max(1) as f64
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct History {
    pub hauled: HaulTotals,
    pub shipped: HaulTotals,
    /// Per character or corporation: (hauled, shipped), busiest first.
    pub by_party: Vec<(Party, HaulTotals, HaulTotals)>,
    /// Routes, most couriers first.
    pub routes: Vec<RouteStats>,
}

/// Courier history from flows (the courier ones are picked out). `contract` finds a flow's contract (for its
/// locations and collateral); `jumps` the jumps between two systems.
pub fn history<'a>(
    flows: impl IntoIterator<Item = &'a Flow>,
    contract: impl Fn(i64) -> Option<&'a Contract>,
    jumps: impl Fn(i64, i64) -> Option<usize>,
) -> History {
    let mut h = History::default();
    let mut parties: BTreeMap<Party, (HaulTotals, HaulTotals)> = BTreeMap::new();
    let mut routes: BTreeMap<(i64, i64), RouteStats> = BTreeMap::new();
    let mut jump_cache: HashMap<(i64, i64), Option<usize>> = HashMap::new();
    for f in flows {
        if !matches!(f.category, Category::Hauled | Category::Shipped | Category::CourierFailed) {
            continue;
        }
        let c = contract(f.contract_id);
        let ends = c.and_then(|c| Some((c.start_location.as_ref()?.system_id?, c.end_location.as_ref()?.system_id?)));
        let j = ends.and_then(|(a, b)| *jump_cache.entry((a, b)).or_insert_with(|| jumps(a, b)));
        let hauling = f.side == crate::features::contracts::Side::Acceptor;
        let (total, party) = if hauling { (&mut h.hauled, &mut parties.entry(f.party).or_default().0) } else { (&mut h.shipped, &mut parties.entry(f.party).or_default().1) };
        total.add(f, c, j);
        party.add(f, c, j);
        if let Some((a, b)) = ends {
            routes.entry((a, b)).or_insert_with(|| RouteStats { from: a, to: b, jumps: j, ..Default::default() }).totals.add(f, c, j);
        }
    }
    h.by_party = parties.into_iter().map(|(p, (a, b))| (p, a, b)).collect();
    h.by_party.sort_by_key(|p| std::cmp::Reverse(p.1.count + p.2.count + p.1.failed + p.2.failed));
    h.routes = routes.into_values().collect();
    h.routes.sort_by(|a, b| (b.totals.count + b.totals.failed).cmp(&(a.totals.count + a.totals.failed)).then(b.totals.reward.total_cmp(&a.totals.reward)));
    h
}

// ---------- finder ----------

/// A public courier with its ends resolved and its routes worked out.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct FinderRow {
    pub contract: PublicContract,
    pub issuer: String,
    pub pickup: Option<Place>,
    pub delivery: Option<Place>,
    /// The route on each preference (`RoutePref::ALL` order), None when it can't be worked out.
    pub routes: [Option<RouteSummary>; 3],
}

impl FinderRow {
    pub fn pickup_system(&self) -> Option<i64> {
        self.pickup.as_ref().and_then(|p| p.system_id)
    }
    pub fn delivery_system(&self) -> Option<i64> {
        self.delivery.as_ref().and_then(|p| p.system_id)
    }
    pub fn route(&self, pref: RoutePref) -> Option<&RouteSummary> {
        self.routes[pref_key(pref) as usize].as_ref()
    }
    /// A route exists that never leaves high-sec.
    pub fn high_sec_possible(&self) -> bool {
        self.route(RoutePref::Safer).is_some_and(|r| r.high_sec())
    }
    pub fn reward(&self) -> f64 {
        self.contract.reward.unwrap_or(0.0)
    }
    pub fn volume(&self) -> f64 {
        self.contract.volume.unwrap_or(0.0)
    }
    pub fn collateral(&self) -> f64 {
        self.contract.collateral.unwrap_or(0.0)
    }
    /// Reward per jump on `pref`'s route, plus `travel` jumps to get to the pickup when given.
    pub fn isk_per_jump(&self, pref: RoutePref, travel: Option<usize>) -> Option<f64> {
        let r = self.route(pref)?;
        Some(per_jump(self.reward(), r.jumps + travel.unwrap_or(0)))
    }
}

/// Whether a row passes the filters. `travel` is the jumps from the hauler's system to the pickup (None: not known,
/// or no system chosen).
pub fn passes(row: &FinderRow, f: &FinderPrefs, pref: RoutePref, travel: Option<usize>) -> bool {
    if let (Some(max), Some(_)) = (f.max_pickup_jumps, f.origin) {
        if travel.is_none_or(|t| t > max) {
            return false;
        }
    }
    if f.max_volume.is_some_and(|m| row.volume() > m) {
        return false;
    }
    if f.max_collateral.is_some_and(|m| row.collateral() > m) {
        return false;
    }
    if f.min_reward.is_some_and(|m| row.reward() < m) {
        return false;
    }
    if let Some(m) = f.min_isk_per_jump {
        let t = if f.count_travel { travel } else { None };
        if row.isk_per_jump(pref, t).is_none_or(|v| v < m) {
            return false;
        }
    }
    !f.high_sec_only || row.high_sec_possible()
}

/// Sorts rows best first. `travel` gives a row's jumps to its pickup.
pub fn sort_rows(rows: &mut [&FinderRow], f: &FinderPrefs, pref: RoutePref, travel: impl Fn(&FinderRow) -> Option<usize>) {
    let key = |r: &FinderRow| -> f64 {
        match f.sort {
            FinderSort::IskPerJump => r.isk_per_jump(pref, if f.count_travel { travel(r) } else { None }).unwrap_or(-1.0),
            FinderSort::Reward => r.reward(),
            FinderSort::IskPerM3 => {
                if r.volume() > 0.0 {
                    r.reward() / r.volume()
                } else {
                    r.reward()
                }
            }
            FinderSort::Nearest => -(travel(r).map(|t| t as f64).unwrap_or(f64::MAX)),
        }
    };
    rows.sort_by(|a, b| key(b).total_cmp(&key(a)).then(b.reward().total_cmp(&a.reward())));
}

/// How far loading the finder has got.
#[derive(Clone, Debug, PartialEq)]
pub enum FinderProgress {
    Listing,
    Locations { done: usize, total: usize },
    Routes { done: usize, total: usize },
}

impl FinderProgress {
    pub fn label(&self) -> String {
        match self {
            FinderProgress::Listing => "Loading the region's public contracts…".into(),
            FinderProgress::Locations { done, total } => format!("Looking up pickup and delivery locations: {done} of {total}"),
            FinderProgress::Routes { done, total } => format!("Working out routes: {done} of {total} pickup systems"),
        }
    }
    pub fn fraction(&self) -> f64 {
        match self {
            FinderProgress::Listing => 0.05,
            FinderProgress::Locations { done, total } => 0.05 + 0.75 * *done as f64 / (*total).max(1) as f64,
            FinderProgress::Routes { done, total } => 0.8 + 0.2 * *done as f64 / (*total).max(1) as f64,
        }
    }
}

const STRUCTURE_SCOPE: &str = "esi-universe.read_structures.v1";
/// Location lookups in flight at once.
const PLACE_CONCURRENCY: usize = 20;

/// A region's public couriers with their locations and routes. Structures are looked up through any character
/// allowed to read structures; one nobody has docking access to stays unknown, and so does its route.
pub async fn load_finder(core: &Core, region: i64, force: bool, progress: impl Fn(FinderProgress)) -> Result<Vec<FinderRow>, EsiError> {
    progress(FinderProgress::Listing);
    let all = core.public_contracts.region(&core.esi, region, force).await?;
    let couriers: Vec<PublicContract> = all.iter().filter(|c| c.kind == "courier").cloned().collect();
    let ids: Vec<i64> = {
        let set: HashSet<i64> = couriers.iter().flat_map(|c| [c.start_location_id, c.end_location_id]).flatten().collect();
        set.into_iter().collect()
    };
    let auth = match core.auth.all().into_iter().find(|a| a.has_scope(STRUCTURE_SCOPE) && !a.needs_reauth()) {
        Some(who) => core.auth.auth(&core.esi.http, who.id, 120).await.ok(),
        None => None,
    };
    let mut places: HashMap<i64, Place> = HashMap::new();
    let total = ids.len();
    progress(FinderProgress::Locations { done: 0, total });
    for chunk in ids.chunks(PLACE_CONCURRENCY) {
        places.extend(core.universe.places(&core.esi, chunk.iter().copied(), auth.as_ref()).await);
        progress(FinderProgress::Locations { done: places.len(), total });
    }
    let issuers = core.universe.names(&core.esi, couriers.iter().map(|c| if c.for_corporation { c.issuer_corporation_id } else { c.issuer_id })).await;
    core.universe.flush();

    let mut rows: Vec<FinderRow> = couriers
        .into_iter()
        .map(|c| {
            let who = if c.for_corporation { c.issuer_corporation_id } else { c.issuer_id };
            FinderRow {
                issuer: issuers.get(&who).cloned().unwrap_or_default(),
                pickup: c.start_location_id.and_then(|id| places.get(&id).cloned()),
                delivery: c.end_location_id.and_then(|id| places.get(&id).cloned()),
                contract: c,
                routes: [None; 3],
            }
        })
        .collect();
    let sources: Vec<i64> = {
        let set: HashSet<i64> = rows.iter().filter(|r| r.delivery_system().is_some()).filter_map(|r| r.pickup_system()).collect();
        set.into_iter().collect()
    };
    let total = sources.len();
    progress(FinderProgress::Routes { done: 0, total });
    // routes are pure computation: off the async workers, a pickup system at a time
    let mut trees: HashMap<i64, [Option<Arc<RouteTree>>; 3]> = HashMap::new();
    for (i, chunk) in sources.chunks(8).enumerate() {
        let chunk = chunk.to_vec();
        let done = tokio::task::spawn_blocking(move || {
            chunk.into_iter().map(|s| (s, RoutePref::ALL.map(|p| tree(s, p)))).collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();
        trees.extend(done);
        progress(FinderProgress::Routes { done: ((i + 1) * 8).min(total), total });
    }
    for r in &mut rows {
        let (Some(a), Some(b)) = (r.pickup_system(), r.delivery_system()) else { continue };
        if let Some(t) = trees.get(&a) {
            for (i, tr) in t.iter().enumerate() {
                r.routes[i] = tr.as_ref().and_then(|tr| tr.route(b)).map(|x| RouteSummary::of(&x));
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::contracts::{flows, Side};

    const JITA: i64 = 30000142;
    const PERIMETER: i64 = 30000144;
    const AMARR: i64 = 30002187;

    fn service() -> Service {
        Service {
            name: "Test Haul".into(),
            assignee: "Test Corp".into(),
            pricing: Pricing { base: 1_000_000.0, per_jump: 500_000.0, per_m3: 100.0, per_m3_jump: 10.0, collateral_rate: 0.01, minimum: 0.0, round_to: 0.0 },
            ..Default::default()
        }
    }

    fn summary_of(jumps: usize) -> RouteSummary {
        RouteSummary { jumps, min_security: 0.9, counts: (jumps + 1, 0, 0) }
    }

    #[test]
    fn prices_add_up() {
        let s = service();
        let q = quote_with(&s, JITA, AMARR, 10_000.0, 1e9, Some(summary_of(10)));
        // 1m + 5m + 1m + 1m + 10m
        assert_eq!(q.reward, Some(18_000_000.0));
        assert_eq!(q.parts.len(), 5);
        assert!(q.ok());
        assert_eq!(q.isk_per_jump(), Some(1_800_000.0));
    }

    #[test]
    fn minimum_and_rounding() {
        let mut s = service();
        s.pricing.minimum = 50_000_000.0;
        let q = quote_with(&s, JITA, AMARR, 10_000.0, 1e9, Some(summary_of(10)));
        assert_eq!((q.reward, q.minimum_applied), (Some(50_000_000.0), true));
        s.pricing.minimum = 0.0;
        s.pricing.round_to = 1_000_000.0;
        let q = quote_with(&s, JITA, AMARR, 10_001.0, 1e9, Some(summary_of(10)));
        // 18,000,000 + 100 + 100 → rounded up
        assert_eq!(q.reward, Some(19_000_000.0));
        let q = quote_with(&s, JITA, AMARR, 10_000.0, 1e9, Some(summary_of(10)));
        assert_eq!(q.reward, Some(18_000_000.0), "an exact multiple stays");
        assert_eq!(round_up(12.2, 0.0), 13.0);
    }

    #[test]
    fn flat_routes_win() {
        let mut s = service();
        s.max_jumps = Some(2);
        s.pickups = vec![PERIMETER];
        s.flat_routes = vec![FlatRoute { from: JITA, to: AMARR, both_ways: true, price: 800.0, per_m3: true, collateral_fee: true }];
        let q = quote_with(&s, AMARR, JITA, 10_000.0, 1e9, Some(summary_of(10)));
        assert!(q.flat);
        assert_eq!(q.reward, Some(18_000_000.0)); // 8m + 1% of 1b
        assert!(q.problems.is_empty(), "flat routes ignore the jump and system limits: {:?}", q.problems);
        s.flat_routes[0].both_ways = false;
        let q = quote_with(&s, AMARR, JITA, 10_000.0, 1e9, Some(summary_of(10)));
        assert!(!q.flat);
        assert!(q.problems.contains(&Problem::TooFar { max: 2 }));
        assert!(q.problems.contains(&Problem::PickupNotServed));
        assert!(!q.problems.contains(&Problem::DeliveryNotServed));
        s.flat_routes[0] = FlatRoute { from: AMARR, to: JITA, price: 25e6, ..Default::default() };
        let q = quote_with(&s, AMARR, JITA, 10_000.0, 1e9, None);
        assert_eq!(q.reward, Some(25e6), "flat price without collateral fee or a route");
        assert!(q.ok());
    }

    #[test]
    fn limits() {
        let mut s = service();
        s.max_volume = Some(5_000.0);
        s.max_collateral = Some(5e8);
        s.high_sec_only = true;
        let low = RouteSummary { jumps: 4, min_security: 0.3, counts: (3, 2, 0) };
        let q = quote_with(&s, JITA, AMARR, 10_000.0, 1e9, Some(low));
        assert_eq!(
            q.problems,
            vec![Problem::TooBig { max: 5_000.0 }, Problem::TooMuchCollateral { max: 5e8 }, Problem::LeavesHighSec { min_security: 0.3 }]
        );
        assert!(!q.ok());
        // without a route, a price that needs jumps can't be given
        let q = quote_with(&s, JITA, 1, 1.0, 1.0, None);
        assert_eq!(q.reward, None);
        assert!(q.problems.contains(&Problem::NoRoute));
        s.pricing.per_jump = 0.0;
        s.pricing.per_m3_jump = 0.0;
        assert_eq!(quote_with(&s, JITA, 1, 1.0, 0.0, None).reward, Some(1_000_100.0));
    }

    #[test]
    fn quotes_on_the_map_and_contract_text() {
        let s = service();
        let q = quote(&s, JITA, PERIMETER, 1_000.0, 0.0);
        assert_eq!(q.route.map(|r| r.jumps), Some(1));
        assert!(q.route.unwrap().high_sec());
        let text = contract_text(&s, "Jita", "Perimeter", 1_000.0, 0.0, &q);
        assert!(text.contains("assigned to Test Corp"));
        assert!(text.contains(&format!("Reward: {} ISK", fmt::decimal(q.reward.unwrap(), 0))));
        assert!(text.contains("Days to complete: 3"));
    }

    #[test]
    fn route_trees_match_the_map() {
        for pref in RoutePref::ALL {
            let t = tree(JITA, pref).unwrap();
            let direct = MAP.route(JITA, AMARR, pref).unwrap();
            assert_eq!(t.jumps(AMARR), Some(direct.jumps()), "{pref:?}");
            let r = t.route(AMARR).unwrap();
            assert_eq!((r.systems[0], *r.systems.last().unwrap(), r.jumps()), (JITA, AMARR, direct.jumps()));
        }
        assert_eq!(route(JITA, JITA, RoutePref::Shortest).unwrap().jumps(), 0);
        assert!(tree(1, RoutePref::Shortest).is_none());
        let near = tree(JITA, RoutePref::Shortest).unwrap().within(1);
        assert!(near.contains(&PERIMETER) && near.contains(&JITA) && !near.contains(&AMARR));
        assert!(summary(JITA, AMARR, RoutePref::Safer).unwrap().high_sec());
        assert_eq!(per_jump(10.0, 0), 10.0);
    }

    fn mine() -> Mine {
        Mine { characters: [(1, "Hauler".to_string()), (2, "Alt".to_string())].into(), corporations: [(500, "My Corp".to_string())].into() }
    }

    fn place(system: i64) -> Option<Place> {
        Some(Place { id: system, system_id: Some(system), kind: "system".into(), ..Default::default() })
    }

    fn courier(id: i64, status: &str, issuer: i64, acceptor: Option<i64>) -> Contract {
        let t: DateTime<Utc> = "2026-06-01T00:00:00Z".parse().unwrap();
        Contract {
            contract_id: id,
            kind: "courier".into(),
            status: status.into(),
            issuer_id: issuer,
            issuer_corporation_id: 900,
            acceptor_id: acceptor,
            date_issued: t,
            date_expired: t + Duration::days(7),
            date_accepted: acceptor.map(|_| t + Duration::hours(1)),
            date_completed: (status == "finished" || status == "failed").then_some(t + Duration::days(1)),
            days_to_complete: Some(3),
            reward: Some(10e6),
            collateral: Some(1e9),
            volume: Some(5_000.0),
            start_location: place(JITA),
            end_location: place(AMARR),
            ..Default::default()
        }
    }

    #[test]
    fn active_sides_and_loads() {
        let now: DateTime<Utc> = "2026-06-03T12:00:00Z".parse().unwrap();
        let hauling = courier(1, "in_progress", 77, Some(1));
        let mut shipped = courier(2, "outstanding", 2, None);
        shipped.assignee_id = Some(1);
        let internal = courier(3, "in_progress", 2, Some(1));
        let mut expired = courier(4, "outstanding", 2, None);
        expired.date_expired = now - Duration::hours(1);
        let foreign = courier(5, "in_progress", 77, Some(88));
        let list = [hauling, shipped, internal, expired, foreign];
        let a = active(&list, &mine(), now);
        let roles: Vec<(i64, Role)> = a.iter().map(|a| (a.contract.contract_id, a.role)).collect();
        assert_eq!(roles.len(), 5);
        assert!(roles.contains(&(1, Role::Hauling)));
        assert!(roles.contains(&(2, Role::Shipping)) && roles.contains(&(2, Role::Assigned)));
        assert!(roles.contains(&(3, Role::Hauling)) && roles.contains(&(3, Role::Shipping)));
        // accepted at 01:00 on the 1st with 3 days: due 01:00 on the 4th
        assert_eq!(a[0].due, "2026-06-04T01:00:00Z".parse::<DateTime<Utc>>().unwrap());
        let t = totals(&a, now);
        assert_eq!((t.hauling.count, t.hauling.collateral, t.due_soon), (2, 2e9, 2));
        assert_eq!((t.in_transit.count, t.waiting.count, t.assigned.count), (1, 1, 1));
        let l = loads(&a, now);
        assert_eq!(l.len(), 1);
        assert_eq!((l[0].0, l[0].1.count, l[0].1.volume), (Party::Character(1), 2, 10_000.0));
    }

    #[test]
    fn history_totals_and_routes() {
        let m = mine();
        let done = courier(1, "finished", 77, Some(1));
        let mut done2 = courier(2, "finished", 77, Some(2));
        done2.reward = Some(30e6);
        let failed = courier(3, "failed", 77, Some(1));
        let mut shipped = courier(4, "finished", 1, Some(66));
        shipped.end_location = place(PERIMETER);
        let contracts = [done, done2, failed, shipped];
        let fl: Vec<Flow> = contracts.iter().flat_map(|c| flows(c, &m, None, true)).collect();
        assert_eq!(fl.iter().filter(|f| f.side == Side::Acceptor).count(), 3);
        let by_id: HashMap<i64, &Contract> = contracts.iter().map(|c| (c.contract_id, c)).collect();
        let h = history(&fl, |id| by_id.get(&id).copied(), |a, b| if (a, b) == (JITA, AMARR) { Some(10) } else { None });
        assert_eq!((h.hauled.count, h.hauled.reward, h.hauled.failed), (2, 40e6, 1));
        assert_eq!(h.hauled.isk_per_jump(), Some(2e6));
        assert_eq!(h.hauled.isk_per_m3(), Some(4_000.0));
        assert!((h.hauled.failure_rate().unwrap() - 1.0 / 3.0).abs() < 1e-9);
        assert_eq!(h.hauled.failed_collateral, 1e9);
        assert_eq!((h.shipped.count, h.shipped.reward, h.shipped.isk_per_jump()), (1, 10e6, None));
        assert_eq!(h.routes[0].from, JITA);
        assert_eq!(h.routes[0].to, AMARR);
        assert_eq!((h.routes[0].totals.count, h.routes[0].totals.failed, h.routes[0].average_reward()), (2, 1, 20e6));
        assert_eq!(h.by_party[0].0, Party::Character(1));
        assert_eq!((h.by_party[0].1.count, h.by_party[0].2.count), (1, 1));
    }

    #[test]
    fn finder_filters_and_sorts() {
        let row = |id: i64, reward: f64, volume: f64, jumps: usize, high: bool| FinderRow {
            contract: PublicContract { contract_id: id, kind: "courier".into(), reward: Some(reward), volume: Some(volume), collateral: Some(1e8), ..Default::default() },
            pickup: place(JITA),
            delivery: place(AMARR),
            routes: [
                Some(RouteSummary { jumps, min_security: if high { 0.5 } else { 0.2 }, counts: (jumps, if high { 0 } else { 1 }, 0) }),
                Some(RouteSummary { jumps: jumps + 2, min_security: if high { 0.5 } else { 0.2 }, counts: (jumps, if high { 0 } else { 1 }, 0) }),
                None,
            ],
            ..Default::default()
        };
        let a = row(1, 10e6, 1_000.0, 10, true);
        let b = row(2, 20e6, 50_000.0, 40, false);
        let mut f = FinderPrefs::default();
        let s = RoutePref::Shortest;
        assert!(passes(&a, &f, s, None) && passes(&b, &f, s, None));
        f.max_volume = Some(10_000.0);
        assert!(passes(&a, &f, s, None) && !passes(&b, &f, s, None));
        f.max_volume = None;
        f.high_sec_only = true;
        assert!(!passes(&b, &f, s, None));
        f.high_sec_only = false;
        f.min_isk_per_jump = Some(900_000.0);
        assert!(passes(&a, &f, s, None) && !passes(&b, &f, s, None));
        assert!(!passes(&a, &f, RoutePref::Safer, None), "10m over 12 jumps");
        f.count_travel = true;
        assert!(!passes(&a, &f, s, Some(5)));
        f.min_isk_per_jump = None;
        f.origin = Some(JITA);
        f.max_pickup_jumps = Some(3);
        assert!(passes(&a, &f, s, Some(3)) && !passes(&a, &f, s, Some(4)) && !passes(&a, &f, s, None));
        assert_eq!(b.isk_per_jump(RoutePref::LessSecure, None), None);

        let mut rows = vec![&b, &a];
        let f = FinderPrefs::default();
        sort_rows(&mut rows, &f, s, |_| None);
        assert_eq!(rows[0].contract.contract_id, 1, "1m/jump beats 0.5m/jump");
        let f = FinderPrefs { sort: FinderSort::Reward, ..Default::default() };
        sort_rows(&mut rows, &f, s, |_| None);
        assert_eq!(rows[0].contract.contract_id, 2);
        let f = FinderPrefs { sort: FinderSort::Nearest, ..Default::default() };
        sort_rows(&mut rows, &f, s, |r| Some(if r.contract.contract_id == 1 { 3 } else { 9 }));
        assert_eq!(rows[0].contract.contract_id, 1);
    }

    #[test]
    fn doc_defaults() {
        let d: CourierDoc = serde_json::from_str("{}").unwrap();
        assert_eq!(d.hulls, default_hulls());
        assert_eq!(d.finder.region, 10000002);
        let d: CourierDoc = serde_json::from_str(r#"{"hulls":[],"services":[{"name":"X"}]}"#).unwrap();
        assert!(d.hulls.is_empty(), "a user who removed every preset keeps none");
        assert_eq!((d.services[0].days_to_complete, d.services[0].route), (3, RoutePref::Safer));
    }
}
