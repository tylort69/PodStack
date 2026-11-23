//! Contract history and money.
//!
//! ESI lists a character's (and its corporation's) contracts for 30 days only, so every contract PodStack sees is
//! kept in an archive (`contracts_archive.json`) that grows from the first refresh on: history that answers "how much
//! did I spend on alliance ships in the last six months".
//!
//! [`flows`] turns a contract into what it meant for each of the user's characters (or corporations) involved: ISK
//! in and out, items in and out, and a [`Category`]. Contracts between two of the user's own characters are marked
//! internal, so totals across all characters can leave them out while each character's own figures keep them.
//! Items are valued when PodStack first sees the contract finished ([`Valuation`]), at the hub's prices of that day,
//! so the profit of a buyback or a ship purchase doesn't move with today's prices.

use crate::character::Character;
use crate::domains::contracts::{Contract, ContractItem};
use crate::domains::Cx;
use crate::sde::MARKET;
use crate::{store, Core, Event};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

// ---------- archive ----------

/// A contract as last seen, with who saw it and what its items were worth.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ArchivedContract {
    pub contract: Contract,
    /// Where it was listed: "c:<character id>" or "corp:<corporation id>".
    #[serde(default)]
    pub sources: BTreeSet<String>,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
    #[serde(default)]
    pub valuation: Option<Valuation>,
}

/// The items' worth at the hub on the day the contract was first seen finished.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Valuation {
    pub at: DateTime<Utc>,
    /// The station whose prices were used.
    pub station: i64,
    /// Items that come with the contract, at the highest buy order and the lowest sell order.
    pub included_buy: f64,
    pub included_sell: f64,
    /// Items asked for in exchange.
    pub asked_buy: f64,
    pub asked_sell: f64,
    /// Types with no orders at the hub (worth nothing in the figures above).
    #[serde(default)]
    pub unpriced: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Archive {
    pub contracts: BTreeMap<i64, ArchivedContract>,
}

impl Archive {
    /// When the archive started: the oldest first sighting, which is how far back totals are complete.
    pub fn since(&self) -> Option<DateTime<Utc>> {
        self.contracts.values().map(|a| a.first_seen).min()
    }
    pub fn get(&self, id: i64) -> Option<&ArchivedContract> {
        self.contracts.get(&id)
    }
}

const ARCHIVE_DOC: &str = "contracts_archive";

static ARCHIVE: RwLock<Option<Arc<Archive>>> = RwLock::new(None);
/// Serializes read-modify-write of the archive: several characters refresh at once.
static WRITE: Mutex<()> = Mutex::new(());

/// The archive as it is now (cheap: it's shared, and read from disk only once).
pub fn archive_snapshot() -> Arc<Archive> {
    if let Some(a) = ARCHIVE.read().unwrap().as_ref() {
        return a.clone();
    }
    let mut slot = ARCHIVE.write().unwrap();
    slot.get_or_insert_with(|| Arc::new(store::load_doc::<Archive>(ARCHIVE_DOC))).clone()
}

fn replace(core: &Core, a: Archive) {
    store::save_doc(ARCHIVE_DOC, &a);
    *ARCHIVE.write().unwrap() = Some(Arc::new(a));
    core.emit(Event::Doc(ARCHIVE_DOC.into()));
}

/// Merges what one listing (a character's or its corporation's) shows into the archive. Contracts the listing no
/// longer shows that were still open and have since expired are marked expired: ESI drops them after 30 days
/// without saying how they ended.
pub fn archive(core: &Core, character: i64, corp: Option<i64>, listed: &[Contract]) {
    let _guard = WRITE.lock().unwrap();
    let now = Utc::now();
    let source = match corp {
        Some(c) => format!("corp:{c}"),
        None => format!("c:{character}"),
    };
    let mut a = (*archive_snapshot()).clone();
    let before = serde_json::to_string(&a.contracts).unwrap_or_default();
    let ids: HashSet<i64> = listed.iter().map(|c| c.contract_id).collect();
    for c in listed {
        let e = a.contracts.entry(c.contract_id).or_insert_with(|| ArchivedContract {
            contract: c.clone(),
            sources: BTreeSet::new(),
            first_seen: now,
            last_seen: now,
            valuation: None,
        });
        let (items, loaded, bids) = (e.contract.items.clone(), e.contract.items_loaded, e.contract.bids.clone());
        e.contract = c.clone();
        // another listing may have had the items or bids when this one couldn't load them
        if !c.items_loaded && loaded {
            e.contract.items = items;
            e.contract.items_loaded = true;
        }
        if c.bids.is_empty() && !bids.is_empty() {
            e.contract.bids = bids;
        }
        e.sources.insert(source.clone());
        e.last_seen = now;
    }
    for e in a.contracts.values_mut() {
        if e.sources.contains(&source) && !ids.contains(&e.contract.contract_id) && !e.contract.is_completed() && e.contract.date_expired < now
        {
            // an accepted courier can outlive its expiry date while it runs, so only unaccepted ones are expired
            if e.contract.status == "outstanding" {
                e.contract.status = "expired".into();
            }
        }
    }
    if serde_json::to_string(&a.contracts).unwrap_or_default() != before {
        replace(core, a);
    }
}

/// Items the archive already has for these contracts.
pub fn known_items(ids: impl IntoIterator<Item = i64>) -> HashMap<i64, Vec<ContractItem>> {
    let a = archive_snapshot();
    ids.into_iter()
        .filter_map(|id| a.get(id).filter(|e| e.contract.items_loaded).map(|e| (id, e.contract.items.clone())))
        .collect()
}

/// Statuses whose items get valued once.
fn is_final(c: &Contract) -> bool {
    c.is_finished() || c.status == "failed"
}

static VALUING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Values the items of finished contracts that haven't been valued yet, at the price hub of the contracts settings.
pub async fn value_new(cx: &Cx) {
    // one at a time is enough: the next character's refresh finds nothing left
    let Ok(_guard) = VALUING.try_lock() else { return };
    let a = archive_snapshot();
    let todo: Vec<i64> = a
        .contracts
        .values()
        .filter(|e| e.valuation.is_none() && is_final(&e.contract) && e.contract.items_loaded)
        .map(|e| e.contract.contract_id)
        .collect();
    if todo.is_empty() {
        return;
    }
    let (station, region) = hub(&settings());
    let types: Vec<i64> = {
        let mut t: Vec<i64> = todo.iter().filter_map(|id| a.get(*id)).flat_map(|e| e.contract.items.iter().map(|i| i.type_id)).collect();
        t.sort_unstable();
        t.dedup();
        t
    };
    let prices = if types.is_empty() { HashMap::new() } else { cx.core.market.best_prices(cx.esi(), region, Some(station), &types, false).await };
    let now = Utc::now();
    let _w = WRITE.lock().unwrap();
    let mut a = (*archive_snapshot()).clone();
    for id in todo {
        let Some(e) = a.contracts.get_mut(&id) else { continue };
        let mut v = Valuation { at: now, station, ..Default::default() };
        let mut unpriced = HashSet::new();
        for i in &e.contract.items {
            let p = prices.get(&i.type_id).copied().unwrap_or_default();
            if p.buy.is_none() && p.sell.is_none() {
                unpriced.insert(i.type_id);
            }
            // blueprint copies have no market: their original's price would be wildly wrong
            let q = if i.is_blueprint_copy() { 0.0 } else { i.quantity as f64 };
            let (b, s) = (p.buy.unwrap_or(0.0) * q, p.sell.unwrap_or(0.0) * q);
            if i.is_included {
                v.included_buy += b;
                v.included_sell += s;
            } else {
                v.asked_buy += b;
                v.asked_sell += s;
            }
        }
        v.unpriced = unpriced.len() as u32;
        e.valuation = Some(v);
    }
    replace(&cx.core, a);
}

// ---------- settings, tags and notes ----------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ContractSettings {
    /// Station whose prices value items (Jita 4-4 by default).
    pub price_station: i64,
    /// Count contracts between the user's own characters in the all-characters totals.
    pub count_internal: bool,
    /// Value items at sell orders (true) or buy orders.
    pub value_at_sell: bool,
}

impl Default for ContractSettings {
    fn default() -> Self {
        ContractSettings { price_station: 60003760, count_internal: false, value_at_sell: true }
    }
}

/// The user's own data about contracts: settings, tags and notes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ContractsDoc {
    pub settings: ContractSettings,
    /// Tags per contract ("doctrine", "SRP", "buyback"), for totals by tag.
    pub tags: BTreeMap<i64, Vec<String>>,
    pub notes: BTreeMap<i64, String>,
}

pub const DOC: &str = "contracts";

pub fn load_doc() -> ContractsDoc {
    store::load_doc(DOC)
}

pub fn save_doc(core: &Core, d: &ContractsDoc) {
    store::save_doc(DOC, d);
    core.emit(Event::Doc(DOC.into()));
}

pub fn settings() -> ContractSettings {
    load_doc().settings
}

/// (station, region) of the price hub.
pub fn hub(s: &ContractSettings) -> (i64, i64) {
    crate::sde::TRADE_HUBS
        .iter()
        .find(|h| h.1 == s.price_station)
        .map(|h| (h.1, h.3))
        .unwrap_or((60003760, 10000002))
}

impl ContractsDoc {
    /// Every tag in use, sorted.
    pub fn all_tags(&self) -> Vec<String> {
        let set: BTreeSet<&String> = self.tags.values().flatten().collect();
        set.into_iter().cloned().collect()
    }
}

// ---------- who's who ----------

/// The user's side: their characters and those characters' corporations.
#[derive(Clone, Debug, Default)]
pub struct Mine {
    pub characters: BTreeMap<i64, String>,
    pub corporations: BTreeMap<i64, String>,
}

impl Mine {
    pub fn of(chars: &[Arc<Character>]) -> Self {
        let mut m = Mine::default();
        for c in chars {
            m.characters.insert(c.id, c.name());
            if let Some(i) = &c.info {
                if i.corporation_id > 0 {
                    let name = i.corporation.as_ref().map(|c| c.name.clone()).unwrap_or_else(|| format!("Corporation #{}", i.corporation_id));
                    m.corporations.insert(i.corporation_id, name);
                }
            }
        }
        m
    }
    pub fn is_mine(&self, id: i64) -> bool {
        self.characters.contains_key(&id) || self.corporations.contains_key(&id)
    }
    pub fn name(&self, p: Party) -> String {
        match p {
            Party::Character(id) => self.characters.get(&id).cloned().unwrap_or_else(|| format!("#{id}")),
            Party::Corporation(id) => self.corporations.get(&id).cloned().unwrap_or_else(|| format!("Corporation #{id}")),
        }
    }
}

/// One of the user's characters or corporations.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Party {
    Character(i64),
    Corporation(i64),
}

impl Party {
    pub fn id(self) -> i64 {
        match self {
            Party::Character(i) | Party::Corporation(i) => i,
        }
    }
}

/// Which end of the contract the user's party was on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Issuer,
    Acceptor,
}

/// What a contract was, from one side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    /// Paid ISK for items (an item exchange or a won auction).
    Bought,
    /// Got ISK for items (including selling to a buyback).
    Sold,
    /// Hauled someone's courier contract and got its reward.
    Hauled,
    /// Paid a hauler's reward.
    Shipped,
    /// A courier that failed: the hauler lost the collateral to the issuer.
    CourierFailed,
    /// Items given or received for nothing.
    Gift,
    /// Items swapped for items, or ISK both ways.
    Trade,
    Loan,
}

impl Category {
    pub const ALL: [Category; 8] = [
        Category::Bought,
        Category::Sold,
        Category::Hauled,
        Category::Shipped,
        Category::CourierFailed,
        Category::Gift,
        Category::Trade,
        Category::Loan,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Category::Bought => "Bought",
            Category::Sold => "Sold",
            Category::Hauled => "Hauled",
            Category::Shipped => "Shipped",
            Category::CourierFailed => "Courier failed",
            Category::Gift => "Gift",
            Category::Trade => "Trade",
            Category::Loan => "Loan",
        }
    }
    pub fn icon(self) -> &'static str {
        match self {
            Category::Bought => "shopping_cart",
            Category::Sold => "sell",
            Category::Hauled => "local_shipping",
            Category::Shipped => "inventory_2",
            Category::CourierFailed => "report",
            Category::Gift => "redeem",
            Category::Trade => "swap_horiz",
            Category::Loan => "handshake",
        }
    }
}

/// A finished contract from one side.
#[derive(Clone, Debug, PartialEq)]
pub struct Flow {
    pub contract_id: i64,
    pub date: DateTime<Utc>,
    pub party: Party,
    pub side: Side,
    pub category: Category,
    /// Both ends are the user's.
    pub internal: bool,
    /// The other end: (id, name). None for a public contract nobody accepted (which never flows).
    pub counterparty: Option<(i64, String)>,
    /// The issuer's corporation when the other end issued it (alliance and corp ship sellers), else None.
    pub counterparty_corp: Option<(i64, String)>,
    /// "public", "personal", "corporation" or "alliance".
    pub availability: String,
    pub isk_in: f64,
    pub isk_out: f64,
    /// (type id, quantity) received and given.
    pub items_in: Vec<(i64, i64)>,
    pub items_out: Vec<(i64, i64)>,
    /// What the received and given items were worth (see [`Valuation`]; None until valued).
    pub value_in: Option<f64>,
    pub value_out: Option<f64>,
    pub volume: f64,
}

impl Flow {
    pub fn net(&self) -> f64 {
        self.isk_in - self.isk_out
    }
    /// ISK and item value together: what the contract gained or lost, when the items have been valued.
    pub fn net_worth(&self) -> Option<f64> {
        let v_in = if self.items_in.is_empty() { Some(0.0) } else { self.value_in };
        let v_out = if self.items_out.is_empty() { Some(0.0) } else { self.value_out };
        Some(self.net() + v_in? - v_out?)
    }
}

fn items_of(c: &Contract, included: bool) -> Vec<(i64, i64)> {
    c.items.iter().filter(|i| i.is_included == included && !i.is_blueprint_copy()).map(|i| (i.type_id, i.quantity)).collect()
}

/// The user's parties on each end of the contract.
pub fn sides(c: &Contract, mine: &Mine) -> (Option<Party>, Option<Party>) {
    let issuer = if c.for_corporation {
        mine.corporations.contains_key(&c.issuer_corporation_id).then_some(Party::Corporation(c.issuer_corporation_id))
    } else {
        mine.characters.contains_key(&c.issuer_id).then_some(Party::Character(c.issuer_id))
    };
    let acceptor = c.acceptor_id.and_then(|id| {
        if mine.characters.contains_key(&id) {
            Some(Party::Character(id))
        } else if mine.corporations.contains_key(&id) {
            Some(Party::Corporation(id))
        } else {
            None
        }
    });
    (issuer, acceptor)
}

/// What a finished (or failed) contract meant for each of the user's parties on it: none when it's still open,
/// ended without a deal, or isn't the user's.
pub fn flows(c: &Contract, mine: &Mine, valuation: Option<&Valuation>, value_at_sell: bool) -> Vec<Flow> {
    let failed = c.kind == "courier" && c.status == "failed";
    if !(c.is_finished() || failed) {
        return vec![];
    }
    let (issuer, acceptor) = sides(c, mine);
    let internal = issuer.is_some() && acceptor.is_some();
    let issuer_ref = if c.for_corporation { (c.issuer_corporation_id, c.issuer_corporation_name.clone()) } else { (c.issuer_id, c.issuer_name.clone()) };
    let acceptor_ref = c.acceptor_id.map(|id| (id, c.acceptor_name.clone().unwrap_or_else(|| format!("#{id}"))));
    let included = items_of(c, true);
    let asked = items_of(c, false);
    let (v_inc, v_ask) = match valuation {
        Some(v) if value_at_sell => (Some(v.included_sell), Some(v.asked_sell)),
        Some(v) => (Some(v.included_buy), Some(v.asked_buy)),
        None => (None, None),
    };
    let mut out = Vec::new();
    for (party, side) in [(issuer, Side::Issuer), (acceptor, Side::Acceptor)] {
        let Some(party) = party else { continue };
        let is_issuer = side == Side::Issuer;
        let (mut isk_in, mut isk_out) = (0.0, 0.0);
        let (items_in, items_out, value_in, value_out);
        let category;
        match c.kind.as_str() {
            "courier" => {
                let reward = c.reward.unwrap_or(0.0);
                let collateral = c.collateral.unwrap_or(0.0);
                (items_in, items_out, value_in, value_out) = (vec![], vec![], None, None);
                if failed {
                    // the issuer keeps the collateral for the lost cargo; the reward was never paid
                    if is_issuer { isk_in = collateral } else { isk_out = collateral }
                    category = Category::CourierFailed;
                } else {
                    if is_issuer { isk_out = reward } else { isk_in = reward }
                    category = if is_issuer { Category::Shipped } else { Category::Hauled };
                }
            }
            "auction" => {
                let price = c.auction_price().unwrap_or(0.0);
                if is_issuer {
                    isk_in = price;
                    (items_in, items_out, value_in, value_out) = (vec![], included.clone(), None, v_inc);
                    category = Category::Sold;
                } else {
                    isk_out = price;
                    (items_in, items_out, value_in, value_out) = (included.clone(), vec![], v_inc, None);
                    category = Category::Bought;
                }
            }
            "loan" => {
                (items_in, items_out, value_in, value_out) = (vec![], vec![], None, None);
                category = Category::Loan;
            }
            _ => {
                let price = c.price.unwrap_or(0.0);
                let reward = c.reward.unwrap_or(0.0);
                if is_issuer {
                    (isk_in, isk_out) = (price, reward);
                    (items_in, items_out, value_in, value_out) = (asked.clone(), included.clone(), v_ask, v_inc);
                } else {
                    (isk_in, isk_out) = (reward, price);
                    (items_in, items_out, value_in, value_out) = (included.clone(), asked.clone(), v_inc, v_ask);
                }
                let net = isk_in - isk_out;
                category = match (items_in.is_empty(), items_out.is_empty()) {
                    (false, true) if net < 0.0 => Category::Bought,
                    (true, false) if net > 0.0 => Category::Sold,
                    (false, true) | (true, false) if net == 0.0 => Category::Gift,
                    (false, false) if net < 0.0 => Category::Bought,
                    (false, false) if net > 0.0 => Category::Sold,
                    _ => Category::Trade,
                };
            }
        }
        let other = if is_issuer { acceptor_ref.clone() } else { Some(issuer_ref.clone()) };
        let counterparty_corp = (!is_issuer).then(|| (c.issuer_corporation_id, c.issuer_corporation_name.clone()));
        out.push(Flow {
            contract_id: c.contract_id,
            date: c.date_completed.or(c.date_accepted).unwrap_or(c.date_issued),
            party,
            side,
            category,
            internal,
            counterparty: other,
            counterparty_corp,
            availability: c.availability.clone(),
            isk_in,
            isk_out,
            items_in,
            items_out,
            value_in,
            value_out,
            volume: c.volume.unwrap_or(0.0),
        });
    }
    out
}

/// Every flow in the archive (and anything listed now that it hasn't caught yet), oldest first.
pub fn all_flows(chars: &[Arc<Character>], archive: &Archive, value_at_sell: bool) -> Vec<Flow> {
    let mine = Mine::of(chars);
    let mut out: Vec<Flow> = Vec::new();
    for e in archive.contracts.values() {
        out.extend(flows(&e.contract, &mine, e.valuation.as_ref(), value_at_sell));
    }
    out.sort_by_key(|f| (f.date, f.contract_id));
    out
}

/// Every contract known: the archive's, plus what the characters list that it doesn't have yet, newest first.
/// Paired with its archive entry when there is one.
pub fn all_known(chars: &[Arc<Character>], archive: &Archive) -> Vec<ArchivedContract> {
    let mut by_id: BTreeMap<i64, ArchivedContract> = archive.contracts.clone();
    let now = Utc::now();
    for ch in chars {
        for (data, source) in [(&ch.contracts, format!("c:{}", ch.id)), (&ch.corp_contracts, format!("corp:{}", ch.info.as_ref().map(|i| i.corporation_id).unwrap_or(0)))] {
            let Some(d) = data else { continue };
            for c in &d.contracts {
                by_id
                    .entry(c.contract_id)
                    .or_insert_with(|| ArchivedContract {
                        contract: c.clone(),
                        sources: BTreeSet::new(),
                        first_seen: now,
                        last_seen: now,
                        valuation: None,
                    })
                    .sources
                    .insert(source.clone());
            }
        }
    }
    let mut out: Vec<ArchivedContract> = by_id.into_values().collect();
    out.sort_by_key(|e| std::cmp::Reverse(e.contract.date_issued));
    out
}

// ---------- totals ----------

/// A period to total over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Period {
    Week,
    #[default]
    Month,
    Quarter,
    HalfYear,
    Year,
    All,
}

impl Period {
    pub const ALL: [Period; 6] = [Period::Week, Period::Month, Period::Quarter, Period::HalfYear, Period::Year, Period::All];
    pub fn label(self) -> &'static str {
        match self {
            Period::Week => "7 days",
            Period::Month => "30 days",
            Period::Quarter => "90 days",
            Period::HalfYear => "6 months",
            Period::Year => "1 year",
            Period::All => "All time",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Period::Week => "7d",
            Period::Month => "30d",
            Period::Quarter => "90d",
            Period::HalfYear => "6m",
            Period::Year => "1y",
            Period::All => "all",
        }
    }
    pub fn from_key(k: &str) -> Period {
        Period::ALL.into_iter().find(|p| p.key() == k).unwrap_or_default()
    }
    pub fn start(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let days = match self {
            Period::Week => 7,
            Period::Month => 30,
            Period::Quarter => 90,
            Period::HalfYear => 182,
            Period::Year => 365,
            Period::All => return None,
        };
        Some(now - Duration::days(days))
    }
    pub fn contains(self, t: DateTime<Utc>, now: DateTime<Utc>) -> bool {
        self.start(now).is_none_or(|s| t >= s)
    }
    /// Day buckets for short periods, months for long ones.
    pub fn by_month(self) -> bool {
        !matches!(self, Period::Week | Period::Month | Period::Quarter)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    pub count: usize,
    pub isk_in: f64,
    pub isk_out: f64,
    /// Items received and given, at their valuation (unvalued items count as 0).
    pub value_in: f64,
    pub value_out: f64,
    pub volume: f64,
}

impl Totals {
    pub fn add(&mut self, f: &Flow) {
        self.count += 1;
        self.isk_in += f.isk_in;
        self.isk_out += f.isk_out;
        self.value_in += f.value_in.unwrap_or(0.0);
        self.value_out += f.value_out.unwrap_or(0.0);
        self.volume += f.volume;
    }
    pub fn net(&self) -> f64 {
        self.isk_in - self.isk_out
    }
    pub fn of<'a>(flows: impl IntoIterator<Item = &'a Flow>) -> Totals {
        let mut t = Totals::default();
        for f in flows {
            t.add(f);
        }
        t
    }
}

/// Totals grouped by a key, largest turnover first.
pub fn group_by<'a, K: Ord + Clone>(flows: impl IntoIterator<Item = &'a Flow>, key: impl Fn(&Flow) -> Option<K>) -> Vec<(K, Totals)> {
    let mut m: BTreeMap<K, Totals> = BTreeMap::new();
    for f in flows {
        if let Some(k) = key(f) {
            m.entry(k).or_default().add(f);
        }
    }
    let mut v: Vec<(K, Totals)> = m.into_iter().collect();
    v.sort_by(|a, b| (b.1.isk_in + b.1.isk_out).total_cmp(&(a.1.isk_in + a.1.isk_out)));
    v
}

/// Totals per day or per month (first day of the month), oldest first, with empty buckets filled in from `start`.
pub fn timeline<'a>(flows: impl IntoIterator<Item = &'a Flow>, start: NaiveDate, end: NaiveDate, by_month: bool) -> Vec<(NaiveDate, Totals)> {
    let bucket = |d: NaiveDate| if by_month { d.with_day(1).unwrap_or(d) } else { d };
    let mut m: BTreeMap<NaiveDate, Totals> = BTreeMap::new();
    let mut d = bucket(start);
    while d <= end {
        m.insert(d, Totals::default());
        d = if by_month {
            if d.month() == 12 { NaiveDate::from_ymd_opt(d.year() + 1, 1, 1) } else { NaiveDate::from_ymd_opt(d.year(), d.month() + 1, 1) }.unwrap()
        } else {
            d + Duration::days(1)
        };
    }
    for f in flows {
        let k = bucket(f.date.date_naive());
        if k >= bucket(start) && k <= end {
            m.entry(k).or_default().add(f);
        }
    }
    m.into_iter().collect()
}

/// The market category ("Ships", "Ship Equipment", …) of a type: its top market group.
pub fn item_category(type_id: i64) -> String {
    MARKET.get(type_id).and_then(|t| MARKET.path(t.group).first().map(|g| g.name.clone())).unwrap_or_else(|| "Other".into())
}

/// Bought or sold quantities per type, with the ISK paid shared out by each type's share of the items' value (or of
/// the item count when they weren't valued).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemTotals {
    pub type_id: i64,
    pub quantity: i64,
    pub isk: f64,
    pub contracts: usize,
}

pub fn by_item<'a>(flows: impl IntoIterator<Item = &'a Flow>, received: bool) -> Vec<ItemTotals> {
    let mut m: HashMap<i64, ItemTotals> = HashMap::new();
    for f in flows {
        let (items, isk) = if received { (&f.items_in, f.isk_out - f.isk_in) } else { (&f.items_out, f.isk_in - f.isk_out) };
        if items.is_empty() {
            continue;
        }
        let total_q: i64 = items.iter().map(|i| i.1).sum();
        for &(t, q) in items {
            let e = m.entry(t).or_insert_with(|| ItemTotals { type_id: t, ..Default::default() });
            e.quantity += q;
            e.contracts += 1;
            if total_q > 0 {
                e.isk += isk.max(0.0) * q as f64 / total_q as f64;
            }
        }
    }
    let mut v: Vec<ItemTotals> = m.into_values().collect();
    v.sort_by(|a, b| b.isk.total_cmp(&a.isk).then(b.quantity.cmp(&a.quantity)));
    v
}

// ---------- open exposure ----------

/// ISK tied up in open contracts.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Exposure {
    /// Asking prices of the user's open sell contracts.
    pub listed: f64,
    pub listed_count: usize,
    /// Collateral the user's haulers put up on couriers they're running.
    pub hauling_collateral: f64,
    pub hauling_count: usize,
    /// Collateral of the user's own couriers in transit: cargo at risk.
    pub shipping_collateral: f64,
    pub shipping_count: usize,
    /// Rewards the user owes haulers on accepted couriers.
    pub shipping_rewards: f64,
}

pub fn exposure<'a>(contracts: impl IntoIterator<Item = &'a Contract>, mine: &Mine) -> Exposure {
    let mut e = Exposure::default();
    for c in contracts {
        if c.is_completed() {
            continue;
        }
        let (issuer, acceptor) = sides(c, mine);
        match (c.kind.as_str(), c.status.as_str()) {
            ("courier", "in_progress") => {
                if acceptor.is_some() {
                    e.hauling_collateral += c.collateral.unwrap_or(0.0);
                    e.hauling_count += 1;
                }
                if issuer.is_some() {
                    e.shipping_collateral += c.collateral.unwrap_or(0.0);
                    e.shipping_rewards += c.reward.unwrap_or(0.0);
                    e.shipping_count += 1;
                }
            }
            ("item_exchange" | "auction", "outstanding") if issuer.is_some() => {
                e.listed += c.price.unwrap_or(0.0).max(0.0);
                e.listed_count += 1;
            }
            _ => {}
        }
    }
    e
}

// ---------- scopes, roles and filters ----------

/// Whose figures to show: everyone's together, or one character's or corporation's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    Of(Party),
}

impl Scope {
    /// "all", "c:<id>" or "corp:<id>", for remembering the choice.
    pub fn key(self) -> String {
        match self {
            Scope::All => "all".into(),
            Scope::Of(Party::Character(id)) => format!("c:{id}"),
            Scope::Of(Party::Corporation(id)) => format!("corp:{id}"),
        }
    }
    pub fn from_key(k: &str) -> Scope {
        let id = |s: &str| s.parse::<i64>().ok();
        if let Some(i) = k.strip_prefix("c:").and_then(id) {
            Scope::Of(Party::Character(i))
        } else if let Some(i) = k.strip_prefix("corp:").and_then(id) {
            Scope::Of(Party::Corporation(i))
        } else {
            Scope::All
        }
    }
    /// Whether a flow counts in this scope. Across all characters a contract between two of them is a transfer, so
    /// it's left out unless the settings say otherwise; one character's own figures always keep it.
    pub fn includes(self, f: &Flow, count_internal: bool) -> bool {
        match self {
            Scope::All => count_internal || !f.internal,
            Scope::Of(p) => f.party == p,
        }
    }
}

impl Mine {
    /// Only one party of the user's: for what one character or corporation has tied up.
    pub fn only(&self, p: Party) -> Mine {
        let mut m = Mine::default();
        match p {
            Party::Character(id) => {
                if let Some(n) = self.characters.get(&id) {
                    m.characters.insert(id, n.clone());
                }
            }
            Party::Corporation(id) => {
                if let Some(n) = self.corporations.get(&id) {
                    m.corporations.insert(id, n.clone());
                }
            }
        }
        m
    }
    fn party_of(&self, id: i64) -> Option<Party> {
        if self.characters.contains_key(&id) {
            Some(Party::Character(id))
        } else if self.corporations.contains_key(&id) {
            Some(Party::Corporation(id))
        } else {
            None
        }
    }
}

/// How one of the user's characters or corporations is on a contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Issued,
    Accepted,
    /// Assigned to them, not accepted (yet) by them.
    Assigned,
}

impl Role {
    pub const ALL: [Role; 3] = [Role::Issued, Role::Accepted, Role::Assigned];
    pub fn label(self) -> &'static str {
        match self {
            Role::Issued => "Issued",
            Role::Accepted => "Accepted",
            Role::Assigned => "Assigned",
        }
    }
}

/// Every way the user's parties are on a contract. A contract issued on behalf of a corporation counts as issued by
/// both the corporation and the character who put it up.
pub fn roles(c: &Contract, mine: &Mine) -> Vec<(Party, Role)> {
    let mut out = Vec::new();
    let (issuer, acceptor) = sides(c, mine);
    if let Some(p) = issuer {
        out.push((p, Role::Issued));
    }
    if c.for_corporation && mine.characters.contains_key(&c.issuer_id) {
        out.push((Party::Character(c.issuer_id), Role::Issued));
    }
    if let Some(p) = acceptor {
        out.push((p, Role::Accepted));
    }
    if let Some(p) = c.assignee_id.and_then(|id| mine.party_of(id)) {
        if acceptor != Some(p) {
            out.push((p, Role::Assigned));
        }
    }
    out
}

/// Lower-case text a search looks through: type, status, title, parties, items and locations.
pub fn search_text(c: &Contract) -> String {
    let mut parts: Vec<&str> = vec![&c.kind, &c.status, &c.title, &c.issuer_name, &c.issuer_corporation_name, &c.availability];
    parts.extend(c.assignee_name.as_deref());
    parts.extend(c.acceptor_name.as_deref());
    for p in [&c.start_location, &c.end_location].into_iter().flatten() {
        parts.push(&p.name);
        parts.extend(p.system_name.as_deref());
        parts.extend(p.region_name.as_deref());
    }
    parts.extend(c.items.iter().map(|i| i.name.as_str()));
    parts.join("\n").replace('_', " ").to_lowercase()
}

/// What the Contracts tab shows. Empty fields don't filter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filter {
    /// Some(false): open only; Some(true): completed only.
    pub completed: Option<bool>,
    /// "item_exchange", "auction", "courier" or "loan".
    pub kind: Option<String>,
    pub party: Option<Party>,
    pub role: Option<Role>,
    pub availability: Option<String>,
    /// On the contract's last date ([`Contract::last_date`]).
    pub period: Option<Period>,
    pub tag: Option<String>,
    /// What it was for the user's parties (finished contracts only).
    pub category: Option<Category>,
    /// A character or corporation on the other end (issuer, issuer's corporation, assignee or acceptor).
    pub counterparty: Option<i64>,
    /// An item's market category ([`item_category`]).
    pub item_category: Option<String>,
    pub type_id: Option<i64>,
    /// Words that must all appear in [`search_text`].
    pub query: String,
}

impl Filter {
    /// `flows` and `roles` are the contract's ([`flows`], [`roles`]), `tags` its tags.
    pub fn matches(&self, c: &Contract, flows: &[Flow], roles: &[(Party, Role)], tags: &[String], now: DateTime<Utc>) -> bool {
        if self.completed.is_some_and(|done| c.is_completed() != done) {
            return false;
        }
        if self.kind.as_ref().is_some_and(|k| *k != c.kind) {
            return false;
        }
        let role_ok = |r: &(Party, Role)| self.party.is_none_or(|p| r.0 == p) && self.role.is_none_or(|x| r.1 == x);
        if (self.party.is_some() || self.role.is_some()) && !roles.iter().any(role_ok) {
            return false;
        }
        if self.availability.as_ref().is_some_and(|a| *a != c.availability) {
            return false;
        }
        if self.period.is_some_and(|p| !p.contains(c.last_date(), now)) {
            return false;
        }
        if self.tag.as_ref().is_some_and(|t| !tags.contains(t)) {
            return false;
        }
        if let Some(cat) = self.category {
            if !flows.iter().any(|f| f.category == cat && self.party.is_none_or(|p| f.party == p)) {
                return false;
            }
        }
        if let Some(id) = self.counterparty {
            let ids = [Some(c.issuer_id), Some(c.issuer_corporation_id), c.assignee_id, c.acceptor_id];
            if !ids.contains(&Some(id)) {
                return false;
            }
        }
        if self.type_id.is_some_and(|t| !c.items.iter().any(|i| i.type_id == t)) {
            return false;
        }
        if self.item_category.as_ref().is_some_and(|cat| !c.items.iter().any(|i| item_category(i.type_id) == *cat)) {
            return false;
        }
        let q = self.query.trim().to_lowercase();
        q.is_empty() || {
            let text = search_text(c);
            q.split_whitespace().all(|w| text.contains(w))
        }
    }
}

/// Totals per tag; a contract with two tags counts under both.
pub fn by_tag<'a>(flows: impl IntoIterator<Item = &'a Flow>, tags: &BTreeMap<i64, Vec<String>>) -> Vec<(String, Totals)> {
    let mut m: BTreeMap<String, Totals> = BTreeMap::new();
    for f in flows {
        for t in tags.get(&f.contract_id).into_iter().flatten() {
            m.entry(t.clone()).or_default().add(f);
        }
    }
    let mut v: Vec<(String, Totals)> = m.into_iter().collect();
    v.sort_by(|a, b| (b.1.isk_in + b.1.isk_out).total_cmp(&(a.1.isk_in + a.1.isk_out)));
    v
}

// ---------- price against value ----------

impl Valuation {
    /// The items that come with the contract, at the chosen orders.
    pub fn included(&self, at_sell: bool) -> f64 {
        if at_sell {
            self.included_sell
        } else {
            self.included_buy
        }
    }
    /// The items asked for in exchange, at the chosen orders.
    pub fn asked(&self, at_sell: bool) -> f64 {
        if at_sell {
            self.asked_sell
        } else {
            self.asked_buy
        }
    }
}

/// What the acceptor pays against what the items they get are worth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Deal {
    /// ISK the acceptor pays (an exchange's price less its reward, an auction's winning bid).
    pub price: f64,
    /// The items they get less the items they give, at the hub.
    pub worth: f64,
}

impl Deal {
    /// Price as a share of the items' worth (0.9 = 10% under market); None when either is nothing.
    pub fn ratio(&self) -> Option<f64> {
        (self.worth > 0.0 && self.price > 0.0).then(|| self.price / self.worth)
    }
}

/// The deal of an item exchange or auction, given what its included and asked-for items are worth.
pub fn deal(c: &Contract, included: f64, asked: f64) -> Option<Deal> {
    let price = match c.kind.as_str() {
        "item_exchange" => c.price.unwrap_or(0.0) - c.reward.unwrap_or(0.0),
        "auction" => c.auction_price().unwrap_or(0.0),
        _ => return None,
    };
    Some(Deal { price, worth: included - asked })
}

// ---------- what needs the user ----------

/// When an accepted courier has to be delivered.
pub fn courier_due(c: &Contract) -> Option<DateTime<Utc>> {
    let days = c.days_to_complete.filter(|d| *d > 0)?;
    if c.kind == "courier" && c.status == "in_progress" {
        Some(c.date_accepted? + Duration::days(days))
    } else {
        None
    }
}

/// How close a hauled courier's deadline has to be to need the user.
pub const DUE_SOON_HOURS: i64 = 24;

/// Contracts waiting on the user: assigned to one of their characters or corporations and not accepted yet, and
/// couriers they're hauling that are due within [`DUE_SOON_HOURS`] (or overdue).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Attention {
    pub assigned: Vec<i64>,
    pub due_soon: Vec<i64>,
}

impl Attention {
    pub fn count(&self) -> usize {
        self.assigned.len() + self.due_soon.len()
    }
}

/// What of `contracts` waits on the user (each contract once).
pub fn attention<'a>(contracts: impl IntoIterator<Item = &'a Contract>, mine: &Mine, now: DateTime<Utc>) -> Attention {
    let mut a = Attention::default();
    let mut seen = HashSet::new();
    for c in contracts {
        if !seen.insert(c.contract_id) {
            continue;
        }
        if c.status == "outstanding" && c.date_expired > now && c.assignee_id.is_some_and(|id| mine.is_mine(id)) {
            a.assigned.push(c.contract_id);
        }
        let hauling = c.acceptor_id.is_some_and(|id| mine.is_mine(id));
        if hauling && courier_due(c).is_some_and(|due| due <= now + Duration::hours(DUE_SOON_HOURS)) {
            a.due_soon.push(c.contract_id);
        }
    }
    a
}

/// What waits on the user across the characters' current contract lists (their own and their corporations').
pub fn attention_of(chars: &[Arc<Character>], now: DateTime<Utc>) -> Attention {
    let mine = Mine::of(chars);
    let listed = chars.iter().flat_map(|c| [&c.contracts, &c.corp_contracts]).flatten().flat_map(|d| d.contracts.iter());
    attention(listed, &mine, now)
}

/// Whether a contract counts as open now: not completed, and not an unaccepted one past its expiry (the archive
/// only learns it expired at the next refresh).
pub fn is_open_now(c: &Contract, now: DateTime<Utc>) -> bool {
    !c.is_completed() && !(c.status == "outstanding" && c.date_expired <= now)
}

// ---------- export ----------

/// Rows as CSV (RFC 4180: fields with commas, quotes or line breaks are quoted).
pub fn csv(rows: &[Vec<String>]) -> String {
    let field = |s: &String| {
        if s.contains([',', '"', '\n', '\r']) {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.clone()
        }
    };
    let mut out = String::new();
    for r in rows {
        out.push_str(&r.iter().map(field).collect::<Vec<_>>().join(","));
        out.push_str("\r\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::contracts::Bid;

    fn mine() -> Mine {
        Mine {
            characters: [(1, "Me".to_string()), (2, "Alt".to_string())].into(),
            corporations: [(500, "My Corp".to_string())].into(),
        }
    }

    fn contract(kind: &str, issuer: i64, acceptor: Option<i64>) -> Contract {
        let t = "2026-06-01T00:00:00Z".parse().unwrap();
        Contract {
            contract_id: 7,
            kind: kind.into(),
            status: "finished".into(),
            issuer_id: issuer,
            issuer_name: format!("#{issuer}"),
            issuer_corporation_id: 900,
            issuer_corporation_name: "Seller Corp".into(),
            acceptor_id: acceptor,
            acceptor_name: acceptor.map(|a| format!("#{a}")),
            availability: "alliance".into(),
            date_issued: t,
            date_expired: t,
            date_completed: Some(t),
            items_loaded: true,
            ..Default::default()
        }
    }

    fn item(type_id: i64, q: i64, included: bool) -> ContractItem {
        ContractItem { record_id: type_id, type_id, name: String::new(), quantity: q, is_included: included, ..Default::default() }
    }

    #[test]
    fn buying_a_ship_from_the_alliance() {
        let mut c = contract("item_exchange", 77, Some(1));
        c.price = Some(250_000_000.0);
        c.items = vec![item(24690, 1, true)];
        let v = Valuation { included_sell: 300_000_000.0, included_buy: 260_000_000.0, ..Default::default() };
        let f = flows(&c, &mine(), Some(&v), true);
        assert_eq!(f.len(), 1);
        let f = &f[0];
        assert_eq!((f.category, f.party, f.side), (Category::Bought, Party::Character(1), Side::Acceptor));
        assert_eq!((f.isk_out, f.isk_in), (250_000_000.0, 0.0));
        assert_eq!(f.items_in, vec![(24690, 1)]);
        assert_eq!(f.net_worth(), Some(50_000_000.0));
        assert_eq!(f.counterparty_corp.as_ref().map(|c| c.1.as_str()), Some("Seller Corp"));
        assert!(!f.internal);
    }

    #[test]
    fn selling_to_a_buyback_and_running_one() {
        // the user sells loot to a buyback corp: issuer gets the price
        let mut c = contract("item_exchange", 2, Some(800));
        c.price = Some(90.0);
        c.items = vec![item(34, 100, true)];
        let f = &flows(&c, &mine(), None, true)[0];
        assert_eq!((f.category, f.isk_in, f.items_out.clone()), (Category::Sold, 90.0, vec![(34, 100)]));
        assert_eq!(f.net_worth(), None, "items not valued yet");

        // the user's corp is the buyback: it accepts and pays
        let mut c = contract("item_exchange", 77, Some(500));
        c.price = Some(90.0);
        c.items = vec![item(34, 100, true)];
        let f = &flows(&c, &mine(), None, true)[0];
        assert_eq!((f.category, f.party, f.isk_out), (Category::Bought, Party::Corporation(500), 90.0));
    }

    #[test]
    fn couriers_auctions_and_internal() {
        let mut c = contract("courier", 77, Some(1));
        c.reward = Some(20e6);
        c.collateral = Some(1e9);
        let f = &flows(&c, &mine(), None, true)[0];
        assert_eq!((f.category, f.isk_in), (Category::Hauled, 20e6));
        c.status = "failed".into();
        let f = &flows(&c, &mine(), None, true)[0];
        assert_eq!((f.category, f.isk_out), (Category::CourierFailed, 1e9));
        c.status = "in_progress".into();
        assert!(flows(&c, &mine(), None, true).is_empty());

        let mut a = contract("auction", 1, Some(66));
        a.price = Some(10.0);
        a.bids = vec![Bid { amount: 15.0, ..Default::default() }, Bid { amount: 12.0, ..Default::default() }];
        assert_eq!(flows(&a, &mine(), None, true)[0].isk_in, 15.0);

        // moving a ship from main to alt: both sides, both internal
        let mut c = contract("item_exchange", 1, Some(2));
        c.items = vec![item(587, 1, true)];
        let f = flows(&c, &mine(), None, true);
        assert_eq!(f.len(), 2);
        assert!(f.iter().all(|f| f.internal && f.category == Category::Gift));
    }

    #[test]
    fn totals_and_timeline() {
        let mut c = contract("item_exchange", 77, Some(1));
        c.price = Some(100.0);
        c.items = vec![item(587, 2, true), item(34, 2, true)];
        let fs = flows(&c, &mine(), None, true);
        let t = Totals::of(&fs);
        assert_eq!((t.count, t.isk_out, t.net()), (1, 100.0, -100.0));
        let items = by_item(&fs, true);
        assert_eq!(items.iter().map(|i| i.isk).sum::<f64>(), 100.0);
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let tl = timeline(&fs, d("2026-04-15"), d("2026-07-02"), true);
        assert_eq!(tl.iter().map(|x| x.0.month()).collect::<Vec<_>>(), vec![4, 5, 6, 7]);
        assert_eq!(tl[2].1.isk_out, 100.0);
        let by_party = group_by(&fs, |f| Some(f.party));
        assert_eq!(by_party[0].0, Party::Character(1));
    }

    #[test]
    fn exposure_counts_open_work() {
        let mut haul = contract("courier", 77, Some(1));
        haul.status = "in_progress".into();
        haul.collateral = Some(5e8);
        let mut listed = contract("item_exchange", 2, None);
        listed.status = "outstanding".into();
        listed.price = Some(3e8);
        let e = exposure([&haul, &listed], &mine());
        assert_eq!((e.hauling_collateral, e.listed, e.listed_count), (5e8, 3e8, 1));
        // one character's share: the alt listed, the main hauls
        let alt = exposure([&haul, &listed], &mine().only(Party::Character(2)));
        assert_eq!((alt.hauling_count, alt.listed_count), (0, 1));
    }

    #[test]
    fn scopes_leave_out_internal_moves_across_everyone() {
        let mut c = contract("item_exchange", 1, Some(2));
        c.items = vec![item(587, 1, true)];
        let fs = flows(&c, &mine(), None, true);
        assert_eq!(fs.iter().filter(|f| Scope::All.includes(f, false)).count(), 0);
        assert_eq!(fs.iter().filter(|f| Scope::All.includes(f, true)).count(), 2);
        let alt = Scope::Of(Party::Character(2));
        assert_eq!(fs.iter().filter(|f| alt.includes(f, false)).count(), 1);
        for s in [Scope::All, alt, Scope::Of(Party::Corporation(500))] {
            assert_eq!(Scope::from_key(&s.key()), s);
        }
        assert_eq!(Scope::from_key("nonsense"), Scope::All);
    }

    #[test]
    fn roles_and_filters() {
        let now: DateTime<Utc> = "2026-06-10T00:00:00Z".parse().unwrap();
        // the corp issues a contract (put up by the main) that the alt is assigned
        let mut c = contract("item_exchange", 1, None);
        c.for_corporation = true;
        c.issuer_corporation_id = 500;
        c.status = "outstanding".into();
        c.assignee_id = Some(2);
        c.title = "Doctrine Ferox".into();
        c.items = vec![ContractItem { name: "Ferox".into(), ..item(37480, 1, true) }];
        let r = roles(&c, &mine());
        assert_eq!(r, vec![(Party::Corporation(500), Role::Issued), (Party::Character(1), Role::Issued), (Party::Character(2), Role::Assigned)]);

        let check = |f: Filter| f.matches(&c, &[], &r, &["doctrine".to_string()], now);
        assert!(check(Filter::default()));
        assert!(check(Filter { completed: Some(false), ..Default::default() }));
        assert!(!check(Filter { completed: Some(true), ..Default::default() }));
        assert!(check(Filter { party: Some(Party::Character(2)), role: Some(Role::Assigned), ..Default::default() }));
        assert!(!check(Filter { party: Some(Party::Character(2)), role: Some(Role::Issued), ..Default::default() }));
        assert!(check(Filter { query: "ferox doctrine".into(), ..Default::default() }));
        assert!(!check(Filter { query: "rifter".into(), ..Default::default() }));
        assert!(check(Filter { tag: Some("doctrine".into()), ..Default::default() }));
        assert!(!check(Filter { tag: Some("srp".into()), ..Default::default() }));
        assert!(check(Filter { type_id: Some(37480), availability: Some("alliance".into()), ..Default::default() }));
        assert!(!check(Filter { kind: Some("courier".into()), ..Default::default() }));
        assert!(check(Filter { period: Some(Period::Month), ..Default::default() }));
        assert!(!check(Filter { period: Some(Period::Week), ..Default::default() }));
        // finished contracts filter by what they were for the user
        c.status = "finished".into();
        c.acceptor_id = Some(77);
        c.price = Some(1e8);
        let fs = flows(&c, &mine(), None, true);
        assert!(Filter { category: Some(Category::Sold), counterparty: Some(77), ..Default::default() }.matches(&c, &fs, &r, &[], now));
        assert!(!Filter { category: Some(Category::Bought), ..Default::default() }.matches(&c, &fs, &r, &[], now));
    }

    #[test]
    fn tag_totals_and_deals() {
        let mut c = contract("item_exchange", 77, Some(1));
        c.price = Some(90.0);
        c.items = vec![item(587, 1, true)];
        let fs = flows(&c, &mine(), None, true);
        let tags: BTreeMap<i64, Vec<String>> = [(7, vec!["srp".to_string(), "doctrine".to_string()])].into();
        let t = by_tag(&fs, &tags);
        assert_eq!(t.len(), 2);
        assert!(t.iter().all(|(_, t)| t.isk_out == 90.0));

        let v = Valuation { included_sell: 100.0, included_buy: 80.0, ..Default::default() };
        let d = deal(&c, v.included(true), v.asked(true)).unwrap();
        assert_eq!(d.ratio(), Some(0.9));
        assert!(deal(&contract("courier", 1, None), 1.0, 0.0).is_none());
    }

    #[test]
    fn what_waits_on_the_user() {
        let now: DateTime<Utc> = "2026-06-10T12:00:00Z".parse().unwrap();
        let mut assigned = contract("item_exchange", 77, None);
        assigned.contract_id = 1;
        assigned.status = "outstanding".into();
        assigned.assignee_id = Some(500);
        assigned.date_expired = now + Duration::days(3);
        let mut expired = assigned.clone();
        expired.contract_id = 2;
        expired.date_expired = now - Duration::hours(1);
        let mut haul = contract("courier", 77, Some(1));
        haul.contract_id = 3;
        haul.status = "in_progress".into();
        haul.days_to_complete = Some(3);
        haul.date_accepted = Some(now - Duration::days(2) - Duration::hours(6));
        let mut later = haul.clone();
        later.contract_id = 4;
        later.date_accepted = Some(now);
        assert_eq!(courier_due(&haul), Some(now + Duration::hours(18)));
        let a = attention([&assigned, &expired, &haul, &later, &assigned], &mine(), now);
        assert_eq!(a, Attention { assigned: vec![1], due_soon: vec![3] });
        assert_eq!(a.count(), 2);
        assert!(is_open_now(&assigned, now) && !is_open_now(&expired, now));
    }

    #[test]
    fn csv_quotes_what_needs_it() {
        let rows = vec![vec!["Type".to_string(), "Title".to_string()], vec!["Courier".into(), "Jita, \"fast\"".into()]];
        assert_eq!(csv(&rows), "Type,Title\r\nCourier,\"Jita, \"\"fast\"\"\"\r\n");
    }
}
