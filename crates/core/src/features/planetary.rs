//! Planetary industry calculations: production chains (what a product takes, down to raw resources and which planet
//! kinds have them), factory economics with customs taxes, how each colony is doing (extraction, factory balance,
//! storage fill, imports it needs and exports it makes, its status), the weekly maintenance list across every
//! character, planet finding, and saved production plans with build-out progress.
//!
//! Game data comes from `crate::pi::PI` (CCP's static data); prices from the market; the customs office rate and
//! alert thresholds are user settings.

use crate::domains::industry::Colony;
use crate::pi::{PinKind, Tier, PI};
use crate::sde::{SecurityClass, MAP};
use crate::{store, Character, Core, Event};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

// ---------- settings and saved state ----------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PiSettings {
    /// Customs office tax rate as a fraction (NPC offices in high-sec charge 10%; player offices set their own).
    pub customs_tax: f64,
    /// Extractors ending within this many hours are a warning, within `critical_hours` critical.
    pub warning_hours: f64,
    pub critical_hours: f64,
    /// Storage this full (fraction) needs a pickup soon / now.
    pub storage_warning: f64,
    pub storage_critical: f64,
    /// Trade hub (by name) for prices.
    pub price_hub: Option<String>,
}

impl Default for PiSettings {
    fn default() -> Self {
        PiSettings { customs_tax: 0.10, warning_hours: 24.0, critical_hours: 6.0, storage_warning: 0.75, storage_critical: 0.95, price_hub: None }
    }
}

/// Saved PI state: settings, maintenance tasks marked done, production plans.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PiDoc {
    pub settings: PiSettings,
    /// Task key → when it was marked done. Keys include the colony's last update, so new data reopens them.
    pub done: BTreeMap<String, DateTime<Utc>>,
    pub plans: Vec<PiPlan>,
}

pub const DOC: &str = "pi";

pub fn load() -> PiDoc {
    store::load_doc(DOC)
}

pub fn save(core: &Core, doc: &PiDoc) {
    store::save_doc(DOC, doc);
    core.emit(Event::Doc(DOC.into()));
}

/// Days a task stays marked done. Its key changes once the colony's data does, so older marks only take up space.
pub const DONE_KEEP_DAYS: i64 = 14;

impl PiDoc {
    pub fn is_done(&self, key: &str) -> bool {
        self.done.contains_key(key)
    }

    /// Marks a maintenance task done (or open again), and forgets marks older than `DONE_KEEP_DAYS`.
    pub fn set_done(&mut self, key: &str, done: bool, now: DateTime<Utc>) {
        if done {
            self.done.insert(key.to_string(), now);
        } else {
            self.done.remove(key);
        }
        self.prune_done(now);
    }

    pub fn prune_done(&mut self, now: DateTime<Utc>) {
        let cutoff = now - Duration::days(DONE_KEEP_DAYS);
        self.done.retain(|_, t| *t >= cutoff);
    }
}

// ---------- chains ----------

/// One product in a chain, with how much of it a target rate needs.
#[derive(Clone, Debug, PartialEq)]
pub struct ChainItem {
    pub type_id: i64,
    pub tier: Tier,
    /// Units per hour needed.
    pub per_hour: f64,
    /// Factories needed to make that (0 for raw resources).
    pub factories: f64,
    pub schematic: Option<i64>,
}

/// Everything `product` takes at `per_hour` units an hour, summed per type, highest tier first.
pub fn chain(product: i64, per_hour: f64) -> Vec<ChainItem> {
    let d = &*PI;
    let mut need: HashMap<i64, f64> = HashMap::new();
    fn walk(d: &crate::pi::PiData, t: i64, rate: f64, need: &mut HashMap<i64, f64>, depth: usize) {
        *need.entry(t).or_default() += rate;
        if depth > 8 {
            return;
        }
        if let Some(s) = d.schematic_for(t) {
            // units of each input per hour = cycles needed per hour × input per cycle
            let cycles_per_hour = rate / s.output.1 as f64;
            for (i, q) in &s.inputs {
                walk(d, *i, cycles_per_hour * *q as f64, need, depth + 1);
            }
        }
    }
    walk(d, product, per_hour, &mut need, 0);
    let mut out: Vec<ChainItem> = need
        .into_iter()
        .map(|(t, r)| {
            let s = d.schematic_for(t);
            ChainItem {
                type_id: t,
                tier: d.tier(t).unwrap_or(0),
                per_hour: r,
                factories: s.map(|s| r / s.per_hour()).unwrap_or(0.0),
                schematic: s.map(|s| s.id),
            }
        })
        .collect();
    out.sort_by(|a, b| b.tier.cmp(&a.tier).then_with(|| d.name(a.type_id).cmp(&d.name(b.type_id))));
    out
}

/// A product's recipe as a tree, for drawing: (type, units per parent cycle, children).
#[derive(Clone, Debug, PartialEq)]
pub struct ChainNode {
    pub type_id: i64,
    pub quantity: i64,
    pub children: Vec<ChainNode>,
}

pub fn tree(product: i64, quantity: i64) -> ChainNode {
    fn build(t: i64, q: i64, depth: usize) -> ChainNode {
        let d = &*PI;
        let children = match d.schematic_for(t) {
            Some(s) if depth < 8 => s.inputs.iter().map(|(i, n)| build(*i, *n, depth + 1)).collect(),
            _ => Vec::new(),
        };
        ChainNode { type_id: t, quantity: q, children }
    }
    build(product, quantity, 0)
}

/// Planet kinds that together have every raw resource `product` needs: a small covering set (greedy), or None when
/// some resource exists on no planet.
pub fn planets_for(product: i64) -> Option<Vec<&'static str>> {
    let d = &*PI;
    let mut missing: HashSet<i64> = d.raw_inputs(product).into_iter().collect();
    let mut chosen = Vec::new();
    while !missing.is_empty() {
        let best = d.kinds.iter().max_by_key(|k| k.resources.iter().filter(|r| missing.contains(r)).count())?;
        let covered: Vec<i64> = best.resources.iter().copied().filter(|r| missing.contains(r)).collect();
        if covered.is_empty() {
            return None;
        }
        for r in covered {
            missing.remove(&r);
        }
        chosen.push(best.key);
    }
    Some(chosen)
}

// ---------- economics ----------

/// Customs tax for moving `units` of `type_id` through a launchpad or command center: export at the full rate,
/// import at the structure's import factor (half).
pub fn customs_tax(type_id: i64, units: f64, rate: f64, import: bool) -> f64 {
    let d = &*PI;
    let base = d.commodity(type_id).map(|c| c.tax_base).unwrap_or(0.0);
    // the static data gives launchpads 1.0 export and 0.5 import factors
    let factor = d
        .pins
        .values()
        .find(|p| p.kind == PinKind::Launchpad)
        .and_then(|p| if import { p.import_tax } else { p.export_tax })
        .unwrap_or(if import { 0.5 } else { 1.0 });
    base * units * rate * factor
}

/// One factory running a product for an hour, buying its inputs and selling its output.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FactoryHour {
    pub output_per_hour: f64,
    pub revenue: f64,
    pub input_cost: f64,
    /// Importing the inputs and exporting the output through customs.
    pub taxes: f64,
    pub profit: f64,
    /// Inputs without a price (the cost leaves them out).
    pub unpriced: usize,
}

/// `prices`: what a unit sells for (output) and costs (inputs).
pub fn factory_hour(product: i64, sell: &HashMap<i64, f64>, buy: &HashMap<i64, f64>, rate: f64) -> Option<FactoryHour> {
    let s = PI.schematic_for(product)?;
    let cycles = 3600.0 / s.cycle as f64;
    let out_units = s.output.1 as f64 * cycles;
    let mut f = FactoryHour { output_per_hour: out_units, revenue: sell.get(&product).copied().unwrap_or(0.0) * out_units, ..Default::default() };
    for (i, q) in &s.inputs {
        let units = *q as f64 * cycles;
        match buy.get(i) {
            Some(p) => f.input_cost += p * units,
            None => f.unpriced += 1,
        }
        f.taxes += customs_tax(*i, units, rate, true);
    }
    f.taxes += customs_tax(product, out_units, rate, false);
    f.profit = f.revenue - f.input_cost - f.taxes;
    Some(f)
}

// ---------- colonies ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Health {
    Expired,
    Critical,
    Warning,
    Ok,
}

impl Health {
    pub fn label(self) -> &'static str {
        match self {
            Health::Expired => "Expired",
            Health::Critical => "Critical",
            Health::Warning => "Warning",
            Health::Ok => "OK",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StorageFill {
    pub pin_id: i64,
    pub kind: PinKind,
    pub name: String,
    pub used: f64,
    pub capacity: f64,
    /// (type, units), most volume first.
    pub contents: Vec<(i64, i64)>,
}

impl StorageFill {
    pub fn fraction(&self) -> f64 {
        if self.capacity > 0.0 {
            self.used / self.capacity
        } else {
            0.0
        }
    }
}

/// How a colony is doing.
#[derive(Clone, Debug, PartialEq)]
pub struct ColonyReport {
    pub health: Health,
    /// Why, in a few words ("2 extractors expired", "launchpad 96% full").
    pub reasons: Vec<String>,
    /// When the first running extractor stops.
    pub next_expiry: Option<DateTime<Utc>>,
    pub extractors_running: usize,
    pub extractors_stopped: usize,
    /// Units per hour extracted, per raw resource (at the current program's average).
    pub extraction: BTreeMap<i64, f64>,
    /// Factories per schematic.
    pub factories: BTreeMap<i64, usize>,
    /// Factories that haven't started a cycle within two cycle lengths: waiting for inputs.
    pub idle_factories: usize,
    /// Net units per hour: positive = made here and left over (export), negative = used here but not made (import).
    pub balance: BTreeMap<i64, f64>,
    pub storage: Vec<StorageFill>,
    /// Command center upgrade level.
    pub level: i64,
}

impl ColonyReport {
    pub fn exports(&self) -> impl Iterator<Item = (i64, f64)> + '_ {
        self.balance.iter().filter(|(_, r)| **r > 1e-9).map(|(t, r)| (*t, *r))
    }
    pub fn imports(&self) -> impl Iterator<Item = (i64, f64)> + '_ {
        self.balance.iter().filter(|(_, r)| **r < -1e-9).map(|(t, r)| (*t, -*r))
    }
    /// Market value per hour of what the colony exports, at `prices`.
    pub fn value_per_hour(&self, prices: &HashMap<i64, f64>) -> f64 {
        self.exports().map(|(t, r)| prices.get(&t).copied().unwrap_or(0.0) * r).sum()
    }
    pub fn fullest(&self) -> Option<&StorageFill> {
        self.storage.iter().max_by(|a, b| a.fraction().total_cmp(&b.fraction()))
    }
}

pub fn report(colony: &Colony, now: DateTime<Utc>, s: &PiSettings) -> ColonyReport {
    let d = &*PI;
    let mut r = ColonyReport {
        health: Health::Ok,
        reasons: Vec::new(),
        next_expiry: None,
        extractors_running: 0,
        extractors_stopped: 0,
        extraction: BTreeMap::new(),
        factories: BTreeMap::new(),
        idle_factories: 0,
        balance: BTreeMap::new(),
        storage: Vec::new(),
        level: colony.upgrade_level,
    };
    for e in &colony.extractors {
        match e.expiry_time {
            Some(end) if end > now => {
                r.extractors_running += 1;
                r.next_expiry = Some(r.next_expiry.map_or(end, |n| n.min(end)));
                if let (Some(t), Some(q), Some(c)) = (e.product_type_id, e.qty_per_cycle, e.cycle_time) {
                    // ESI's qty_per_cycle is the program's base rate; the actual yield decays around it, so this is an
                    // average.
                    let per_hour = q as f64 * 3600.0 / c.max(1) as f64;
                    *r.extraction.entry(t).or_default() += per_hour;
                    *r.balance.entry(t).or_default() += per_hour;
                }
            }
            _ => r.extractors_stopped += 1,
        }
    }
    for p in &colony.pins {
        let Some(pin) = d.pins.get(&p.type_id) else { continue };
        match pin.kind {
            PinKind::Factory => {
                let Some(sid) = p.schematic_id else { continue };
                *r.factories.entry(sid).or_default() += 1;
                if let Some(sc) = d.schematics.get(&sid) {
                    let cycles = 3600.0 / sc.cycle as f64;
                    *r.balance.entry(sc.output.0).or_default() += sc.output.1 as f64 * cycles;
                    for (i, q) in &sc.inputs {
                        *r.balance.entry(*i).or_default() -= *q as f64 * cycles;
                    }
                    let stalled = p.last_cycle_start.is_none_or(|l| now - l > Duration::seconds(sc.cycle * 2));
                    if stalled {
                        r.idle_factories += 1;
                    }
                }
            }
            PinKind::Storage | PinKind::Launchpad | PinKind::CommandCenter => {
                let mut contents = p.contents.clone();
                let vol = |t: i64| d.commodity(t).map(|c| c.volume).unwrap_or(0.0);
                contents.sort_by(|a, b| (vol(b.0) * b.1 as f64).total_cmp(&(vol(a.0) * a.1 as f64)));
                let used = contents.iter().map(|(t, n)| vol(*t) * *n as f64).sum();
                r.storage.push(StorageFill { pin_id: p.pin_id, kind: pin.kind, name: pin.name.clone(), used, capacity: pin.capacity, contents });
            }
            _ => {}
        }
    }
    // tiny float leftovers aren't real imports or exports
    r.balance.retain(|_, v| v.abs() > 1e-6);

    let hours = |t: DateTime<Utc>| (t - now).num_seconds() as f64 / 3600.0;
    let mut health = Health::Ok;
    let mut raise = |h: Health, why: String, r: &mut Vec<String>| {
        health = health.min(h);
        r.push(why);
    };
    let mut reasons = Vec::new();
    if r.extractors_stopped > 0 {
        raise(Health::Expired, format!("{} extractor{} stopped", r.extractors_stopped, if r.extractors_stopped == 1 { "" } else { "s" }), &mut reasons);
    }
    if let Some(n) = r.next_expiry {
        let h = hours(n);
        if h <= s.critical_hours {
            raise(Health::Critical, format!("extractor stops in {:.0}h", h.max(0.0)), &mut reasons);
        } else if h <= s.warning_hours {
            raise(Health::Warning, format!("extractor stops in {:.0}h", h), &mut reasons);
        }
    }
    if let Some(f) = r.fullest() {
        let frac = f.fraction();
        if frac >= s.storage_critical {
            raise(Health::Critical, format!("{} {:.0}% full", short_pin(&f.name), frac * 100.0), &mut reasons);
        } else if frac >= s.storage_warning {
            raise(Health::Warning, format!("{} {:.0}% full", short_pin(&f.name), frac * 100.0), &mut reasons);
        }
    }
    if r.idle_factories > 0 {
        raise(Health::Warning, format!("{} factor{} waiting for input", r.idle_factories, if r.idle_factories == 1 { "y" } else { "ies" }), &mut reasons);
    }
    r.health = health;
    r.reasons = reasons;
    r
}

/// "Barren Launchpad" → "Launchpad".
pub fn short_pin(name: &str) -> String {
    for k in ["Launchpad", "Storage Facility", "Command Center"] {
        if name.ends_with(k) {
            return k.to_string();
        }
    }
    name.to_string()
}

// ---------- characters ----------

#[derive(Clone, Debug, PartialEq)]
pub struct PiCharacter {
    pub max_planets: i64,
    pub planets: usize,
    pub max_level: i64,
    /// (skill, level) for `pi::SKILLS`.
    pub skills: Vec<(i64, i64)>,
}

pub fn character(c: &Character) -> PiCharacter {
    let lvl = |s: i64| c.trained_skill_level(s) as i64;
    PiCharacter {
        max_planets: 1 + lvl(crate::pi::INTERPLANETARY_CONSOLIDATION),
        planets: c.planets.as_ref().map(|p| p.colonies.len()).unwrap_or(0),
        max_level: lvl(crate::pi::COMMAND_CENTER_UPGRADES),
        skills: crate::pi::SKILLS.iter().map(|s| (*s, lvl(*s))).collect(),
    }
}

// ---------- maintenance ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Priority {
    Urgent,
    High,
    Normal,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Priority::Urgent => "Urgent",
            Priority::High => "High",
            Priority::Normal => "Normal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskKind {
    RestartExtractors,
    PickUp,
    Deliver,
    CheckFactories,
}

impl TaskKind {
    pub fn label(self) -> &'static str {
        match self {
            TaskKind::RestartExtractors => "Restart extractors",
            TaskKind::PickUp => "Pick up",
            TaskKind::Deliver => "Deliver",
            TaskKind::CheckFactories => "Check factories",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    /// Stable while the colony data is unchanged: marking done survives refreshes that change nothing.
    pub key: String,
    pub priority: Priority,
    pub kind: TaskKind,
    pub character_id: i64,
    pub character_name: String,
    pub planet_id: i64,
    pub planet_name: String,
    pub detail: String,
    /// For deliveries: where the goods come from (character, planet), when another colony makes them.
    pub source: Option<(i64, String, String)>,
    pub due: Option<DateTime<Utc>>,
}

impl Task {
    /// A delivery from another character's colony needs a hauler or a contract.
    pub fn cross_character(&self) -> bool {
        self.source.as_ref().is_some_and(|s| s.0 != self.character_id)
    }
}

/// Everything that needs doing across all characters' colonies, most urgent first.
pub fn tasks(characters: &[std::sync::Arc<Character>], now: DateTime<Utc>, s: &PiSettings) -> Vec<Task> {
    let d = &*PI;
    let mut out = Vec::new();
    // Who makes what (for delivery sources): type → (character, name, planet).
    let mut makers: HashMap<i64, Vec<(i64, String, String)>> = HashMap::new();
    let mut reports = Vec::new();
    for c in characters {
        for col in c.planets.iter().flat_map(|p| &p.colonies) {
            let rep = report(col, now, s);
            for (t, _) in rep.exports() {
                makers.entry(t).or_default().push((c.id, c.name(), col.name()));
            }
            reports.push((c, col, rep));
        }
    }
    for (c, col, rep) in &reports {
        let key = |k: &str| format!("{}:{}:{}:{}", k, c.id, col.planet_id, col.last_update.timestamp());
        let base = |priority, kind, detail: String, k: &str| Task {
            key: key(k),
            priority,
            kind,
            character_id: c.id,
            character_name: c.name(),
            planet_id: col.planet_id,
            planet_name: col.name(),
            detail,
            source: None,
            due: None,
        };
        if rep.extractors_stopped > 0 {
            out.push(base(Priority::Urgent, TaskKind::RestartExtractors, format!("{} stopped", rep.extractors_stopped), "restart"));
        } else if let Some(n) = rep.next_expiry {
            let h = (n - now).num_seconds() as f64 / 3600.0;
            if h <= s.warning_hours {
                let mut t = base(if h <= s.critical_hours { Priority::Urgent } else { Priority::High }, TaskKind::RestartExtractors, format!("stops in {:.0}h", h.max(0.0)), "restart");
                t.due = Some(n);
                out.push(t);
            }
        }
        if let Some(f) = rep.fullest() {
            if f.fraction() >= s.storage_warning {
                let what: Vec<String> = f.contents.iter().take(3).map(|(t, n)| format!("{} × {}", crate::fmt::int(*n), d.name(*t))).collect();
                let pr = if f.fraction() >= s.storage_critical { Priority::Urgent } else { Priority::High };
                out.push(base(pr, TaskKind::PickUp, format!("{} {:.0}% full: {}", short_pin(&f.name), f.fraction() * 100.0, what.join(", ")), "pickup"));
            }
        }
        for (t, rate) in rep.imports() {
            // Only raise it when the inputs held here run out within a day.
            let held: f64 = col.pins.iter().flat_map(|p| &p.contents).filter(|c| c.0 == t).map(|c| c.1 as f64).sum();
            if held >= rate * 24.0 {
                continue;
            }
            let mut task = base(
                if held < rate * s.critical_hours { Priority::High } else { Priority::Normal },
                TaskKind::Deliver,
                format!("{} ({} / h used, {} here)", d.name(t), crate::fmt::int(rate.round() as i64), crate::fmt::int(held as i64)),
                &format!("deliver{t}"),
            );
            task.source = makers.get(&t).and_then(|m| m.iter().find(|x| !(x.0 == c.id && x.2 == col.name())).cloned());
            out.push(task);
        }
        if rep.idle_factories > 0 && rep.imports().next().is_none() {
            out.push(base(Priority::Normal, TaskKind::CheckFactories, format!("{} idle", rep.idle_factories), "factories"));
        }
    }
    out.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.due.cmp(&b.due)).then_with(|| a.character_name.cmp(&b.character_name)));
    out
}

// ---------- finding planets ----------

#[derive(Clone, Debug, PartialEq)]
pub struct SystemPlanets {
    pub system: i64,
    /// Planets per kind key.
    pub counts: BTreeMap<&'static str, usize>,
}

impl SystemPlanets {
    pub fn total(&self) -> usize {
        self.counts.values().sum()
    }
    pub fn kinds(&self) -> usize {
        self.counts.len()
    }
}

/// Planet kinds in one system.
pub fn system_planets(system: i64) -> SystemPlanets {
    let d = &*PI;
    let mut counts = BTreeMap::new();
    for (t, _) in d.planets_in(system) {
        if let Some(k) = d.kind_of_type(*t) {
            *counts.entry(k.key).or_default() += 1;
        }
    }
    SystemPlanets { system, counts }
}

/// Systems that have every planet kind in `kinds` (none: any), filtered by security and region, most kinds first.
pub fn find_systems(kinds: &[&str], security: Option<SecurityClass>, region: Option<i64>, limit: usize) -> Vec<SystemPlanets> {
    let d = &*PI;
    let mut out: Vec<SystemPlanets> = d
        .systems_with_planets()
        .filter_map(|(id, _)| {
            let sys = MAP.system(*id)?;
            if security.is_some_and(|s| sys.security_class() != s) || region.is_some_and(|r| sys.region != r) {
                return None;
            }
            let sp = system_planets(*id);
            kinds.iter().all(|k| sp.counts.contains_key(k)).then_some(sp)
        })
        .collect();
    out.sort_by(|a, b| b.kinds().cmp(&a.kinds()).then(b.total().cmp(&a.total())).then(a.system.cmp(&b.system)));
    out.truncate(limit);
    out
}

// ---------- plans ----------

/// What a planned planet does.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum PlanetRole {
    /// Extract these raw resources and refine them to P1 on the spot.
    Extract { resources: Vec<i64> },
    /// Run factories of a schematic.
    Factory { schematic: i64, count: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PlannedPlanet {
    pub character_id: Option<i64>,
    /// Planet kind key ("barren").
    pub kind: String,
    pub system_id: Option<i64>,
    pub role: PlanetRole,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct PiPlan {
    pub id: String,
    pub name: String,
    /// The product the plan makes, and how many factories of it.
    pub product: i64,
    pub factories: usize,
    pub planets: Vec<PlannedPlanet>,
}

/// A first plan for `product` with `factories` final factories: one extraction planet per pair of raw resources
/// (on a planet kind that has both when possible), then one factory planet per intermediate and final schematic,
/// characters assigned in order of free planet slots.
pub fn suggest_plan(product: i64, factories: usize, characters: &[std::sync::Arc<Character>], system: Option<i64>) -> PiPlan {
    let d = &*PI;
    let available: Vec<&'static str> = match system {
        Some(s) => system_planets(s).counts.keys().copied().collect(),
        None => d.kinds.iter().map(|k| k.key).collect(),
    };
    let mut raw: Vec<i64> = d.raw_inputs(product);
    let mut planets = Vec::new();
    // pair up raw resources that share a planet kind available here
    while let Some(first) = raw.first().copied() {
        raw.remove(0);
        let kinds: Vec<&'static str> = d.kinds_with(first).iter().map(|k| k.key).filter(|k| available.contains(k)).collect();
        let partner = raw.iter().position(|r| d.kinds_with(*r).iter().any(|k| kinds.contains(&k.key)));
        let mut resources = vec![first];
        let kind = match partner {
            Some(i) => {
                let other = raw.remove(i);
                let k = d.kinds_with(other).iter().map(|k| k.key).find(|k| kinds.contains(k)).unwrap_or(kinds[0]);
                resources.push(other);
                k
            }
            None => kinds.first().copied().unwrap_or_else(|| d.kinds_with(first).first().map(|k| k.key).unwrap_or("barren")),
        };
        planets.push(PlannedPlanet { character_id: None, kind: kind.into(), system_id: system, role: PlanetRole::Extract { resources } });
    }
    // factory planets for P2 and up, sized for the final factories
    let rate = d.schematic_for(product).map(|s| s.per_hour() * factories.max(1) as f64).unwrap_or(0.0);
    let factory_kind = available.first().copied().unwrap_or("barren");
    for item in chain(product, rate).into_iter().filter(|c| c.tier >= 2) {
        if let Some(sid) = item.schematic {
            planets.push(PlannedPlanet {
                character_id: None,
                kind: factory_kind.into(),
                system_id: system,
                role: PlanetRole::Factory { schematic: sid, count: item.factories.ceil().max(1.0) as usize },
            });
        }
    }
    // assign characters with free planet slots
    let mut free: Vec<(i64, i64)> = characters.iter().map(|c| { let p = character(c); (c.id, p.max_planets - p.planets as i64) }).filter(|f| f.1 > 0).collect();
    free.sort_by_key(|f| std::cmp::Reverse(f.1));
    let mut slots = free.into_iter().flat_map(|(id, n)| std::iter::repeat_n(id, n as usize));
    for p in &mut planets {
        p.character_id = slots.next();
    }
    PiPlan {
        id: format!("pi{}", Utc::now().timestamp_nanos_opt().unwrap_or_default()),
        name: format!("{} × {}", d.name(product), factories.max(1)),
        product,
        factories: factories.max(1),
        planets,
    }
}

/// How far the characters' colonies match a plan: per planned planet, whether a colony fills it.
pub fn plan_progress(plan: &PiPlan, characters: &[std::sync::Arc<Character>], now: DateTime<Utc>) -> Vec<bool> {
    let s = PiSettings::default();
    let mut used: HashSet<i64> = HashSet::new();
    plan.planets
        .iter()
        .map(|pp| {
            let cands = characters.iter().filter(|c| pp.character_id.is_none_or(|id| id == c.id)).flat_map(|c| c.planets.iter().flat_map(|p| &p.colonies));
            for col in cands {
                if used.contains(&col.planet_id) || !col.planet_type.eq_ignore_ascii_case(&pp.kind) || pp.system_id.is_some_and(|sid| sid != col.solar_system_id) {
                    continue;
                }
                let rep = report(col, now, &s);
                let fits = match &pp.role {
                    PlanetRole::Extract { resources } => resources.iter().all(|r| col.extractors.iter().any(|e| e.product_type_id == Some(*r))),
                    PlanetRole::Factory { schematic, count } => rep.factories.get(schematic).copied().unwrap_or(0) >= *count,
                };
                if fits {
                    used.insert(col.planet_id);
                    return true;
                }
            }
            false
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::industry::{ColonyPin, Extractor};

    fn by_name(n: &str) -> i64 {
        PI.commodities.values().find(|c| c.name == n).unwrap().id
    }

    #[test]
    fn chains() {
        // One Coolant factory: 5 an hour from 40 Electrolytes + 40 Water an hour, each from 3000 P0 an hour
        let coolant = by_name("Coolant");
        let c = chain(coolant, PI.schematic_for(coolant).unwrap().per_hour());
        let water = c.iter().find(|i| i.type_id == by_name("Water")).unwrap();
        assert!((water.per_hour - 40.0).abs() < 1e-9, "{}", water.per_hour);
        assert!((water.factories - 40.0 / PI.schematic_for(by_name("Water")).unwrap().per_hour()).abs() < 1e-9);
        let aqueous = c.iter().find(|i| i.type_id == by_name("Aqueous Liquids")).unwrap();
        assert_eq!(aqueous.tier, 0);
        assert!(aqueous.per_hour > 1000.0);
        assert_eq!(c[0].type_id, coolant);
        let t = tree(coolant, 1);
        assert_eq!(t.children.len(), 2);
        assert_eq!(t.children[0].children.len(), 1);
        // every P4 can be found on some set of planets
        for p in PI.of_tier(4) {
            assert!(planets_for(p.id).is_some_and(|v| !v.is_empty() && v.len() <= 8), "{}", p.name);
        }
    }

    #[test]
    fn economics() {
        let coolant = by_name("Coolant");
        let mut sell = HashMap::new();
        let mut buy = HashMap::new();
        sell.insert(coolant, 10_000.0);
        buy.insert(by_name("Water"), 500.0);
        buy.insert(by_name("Electrolytes"), 500.0);
        let f = factory_hour(coolant, &sell, &buy, 0.10).unwrap();
        assert_eq!(f.output_per_hour, 5.0);
        assert_eq!(f.revenue, 50_000.0);
        assert_eq!(f.input_cost, 40_000.0);
        assert!(f.taxes > 0.0);
        assert!((f.profit - (10_000.0 - f.taxes)).abs() < 1e-6);
        // exporting 1 unit of a P1 at 10% costs a tenth of its tax base
        let water = by_name("Water");
        let base = PI.commodity(water).unwrap().tax_base;
        assert!((customs_tax(water, 1.0, 0.10, false) - base * 0.10).abs() < 1e-9);
        assert!((customs_tax(water, 1.0, 0.10, true) - base * 0.05).abs() < 1e-9);
    }

    #[test]
    fn colony_reports_and_tasks() {
        let now = Utc::now();
        let aqueous = by_name("Aqueous Liquids");
        let water = by_name("Water");
        let water_s = PI.schematic_for(water).unwrap().id;
        let factory = *PI.schematic_for(water).unwrap().pins.first().unwrap();
        let pad = PI.pins.values().find(|p| p.kind == PinKind::Launchpad).unwrap().id;
        let col = Colony {
            planet_id: 1,
            planet_name: "Test I".into(),
            planet_type: "barren".into(),
            last_update: now,
            extractors: vec![
                Extractor { pin_id: 1, product_type_id: Some(aqueous), qty_per_cycle: Some(6000), cycle_time: Some(1800), expiry_time: Some(now + Duration::hours(3)), ..Default::default() },
                Extractor { pin_id: 2, product_type_id: Some(aqueous), expiry_time: Some(now - Duration::hours(1)), ..Default::default() },
            ],
            pins: vec![
                ColonyPin { pin_id: 3, type_id: factory, schematic_id: Some(water_s), last_cycle_start: Some(now), ..Default::default() },
                ColonyPin { pin_id: 4, type_id: pad, contents: vec![(water, 50_000)], ..Default::default() },
            ],
            ..Default::default()
        };
        let s = PiSettings::default();
        let r = report(&col, now, &s);
        assert_eq!(r.health, Health::Expired);
        assert_eq!(r.extractors_running, 1);
        assert_eq!(r.extraction[&aqueous], 12_000.0);
        // the factory uses 3000 × 2 an hour, extraction is 12,000: 6,000 left over to export, and Water made
        assert!((r.balance[&aqueous] - 6_000.0).abs() < 1e-6);
        assert!(r.balance[&water] > 0.0);
        let fill = r.fullest().unwrap();
        assert!(fill.fraction() > 0.9, "{}", fill.fraction());
        let c = std::sync::Arc::new(Character {
            id: 7,
            planets: Some(crate::domains::industry::PlanetsData { colonies: vec![col] }),
            ..Default::default()
        });
        let t = tasks(std::slice::from_ref(&c), now, &s);
        assert_eq!(t[0].kind, TaskKind::RestartExtractors);
        assert_eq!(t[0].priority, Priority::Urgent);
        assert!(t.iter().any(|t| t.kind == TaskKind::PickUp));
        // a plan for Coolant; the colony above counts as its Water extraction planet only if it extracts both inputs
        let plan = suggest_plan(by_name("Coolant"), 2, std::slice::from_ref(&c), None);
        assert!(plan.planets.iter().any(|p| matches!(p.role, PlanetRole::Factory { .. })));
        assert!(plan.planets.iter().any(|p| matches!(&p.role, PlanetRole::Extract { resources } if !resources.is_empty())));
        assert_eq!(plan_progress(&plan, &[c], now).len(), plan.planets.len());
    }

    #[test]
    fn done_marks() {
        let now = Utc::now();
        let mut doc = PiDoc::default();
        doc.done.insert("old".into(), now - Duration::days(DONE_KEEP_DAYS + 1));
        doc.done.insert("recent".into(), now - Duration::days(2));
        doc.set_done("restart:7:1:0", true, now);
        assert!(doc.is_done("restart:7:1:0"));
        assert!(doc.is_done("recent"));
        // marking anything drops marks past the keep window
        assert!(!doc.is_done("old"));
        doc.set_done("restart:7:1:0", false, now);
        assert!(!doc.is_done("restart:7:1:0"));
        assert_eq!(doc.done.len(), 1);
    }

    #[test]
    fn finder() {
        let found = find_systems(&["barren", "gas", "temperate"], Some(SecurityClass::High), None, 20);
        assert!(!found.is_empty());
        assert!(found.iter().all(|s| s.counts.contains_key("gas") && MAP.system(s.system).unwrap().security_class() == SecurityClass::High));
        assert!(system_planets(30000142).total() > 0);
    }
}
