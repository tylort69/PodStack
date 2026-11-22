//! Trading: order book statistics, fees and margins, EVE's price ticks, how the player's orders stand against the
//! competition, profit tracking from wallet transactions, and item appraisal.
//!
//! Order book statistics follow EVE Tycoon's: outliers are dropped first (buy orders under 10% of the highest buy,
//! sell orders over 10× the lowest sell), then each side's price is the "5% average", the volume-weighted average of
//! the best 5% of its volume.
//!
//! Fees (NPC stations, since the 2024 changes):
//!
//! ```text
//!   broker fee = 3% − 0.3% × Broker Relations level  (charged to whoever places an order, buy or sell)
//!   sales tax  = 7.5% × (1 − 11% × Accounting level) (charged on every sale)
//! ```
//!
//! Standings and player structures lower the broker fee further, so the fees per character can be overridden.
//!
//! Profits are matched first in, first out across all characters: each sale uses up the oldest bought units of that
//! type still unsold. Units sold with no recorded purchase (from loot, industry, contracts, or bought more than the
//! kept history ago) use the player's custom cost for that type, or count as unknown cost. A transaction paid a
//! broker fee when one of the player's orders at that location for that type had exactly that price; otherwise it
//! took someone else's order and paid none. The trade settings (fee overrides, custom costs) are the `trade` document.

use crate::character::Character;
use crate::domains::wallet::{MarketOrder, Transaction};
use crate::market::BookOrder;
use crate::sde::{self, MARKET};
use crate::store;
use crate::{Core, Event};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

pub const BROKER_RELATIONS: i64 = 3446;
pub const ACCOUNTING: i64 = 16622;

// ---------------------------------------------------------------------------------------------------------------
// Order book statistics

/// One side of an order book, without outliers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SideStats {
    pub orders: usize,
    pub volume: i64,
    /// Orders dropped as outliers, and the price beyond which an order counts as one.
    pub outliers: usize,
    pub threshold: Option<f64>,
    /// Highest buy or lowest sell.
    pub best: Option<f64>,
    /// Volume-weighted average of the best 5% of the volume.
    pub five_percent: Option<f64>,
    /// Volume-weighted average of every order.
    pub average: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BookStats {
    pub buy: SideStats,
    pub sell: SideStats,
}

impl BookStats {
    /// Lowest sell − highest buy.
    pub fn spread(&self) -> Option<f64> {
        Some(self.sell.best? - self.buy.best?)
    }
    /// 5% sell − 5% buy, what EVE Tycoon calls the average margin.
    pub fn margin(&self) -> Option<f64> {
        Some(self.sell.five_percent? - self.buy.five_percent?)
    }
}

pub fn book_stats<'a>(orders: impl IntoIterator<Item = &'a BookOrder>) -> BookStats {
    let (mut buys, mut sells): (Vec<&BookOrder>, Vec<&BookOrder>) = orders.into_iter().partition(|o| o.is_buy_order);
    buys.sort_by(|a, b| b.price.total_cmp(&a.price));
    sells.sort_by(|a, b| a.price.total_cmp(&b.price));
    let buy_threshold = buys.first().map(|o| o.price * 0.1);
    let sell_threshold = sells.first().map(|o| o.price * 10.0);
    let buy_kept: Vec<&BookOrder> = buys.iter().copied().filter(|o| buy_threshold.is_none_or(|t| o.price >= t)).collect();
    let sell_kept: Vec<&BookOrder> = sells.iter().copied().filter(|o| sell_threshold.is_none_or(|t| o.price <= t)).collect();
    BookStats {
        buy: side(&buy_kept, buys.len() - buy_kept.len(), buy_threshold),
        sell: side(&sell_kept, sells.len() - sell_kept.len(), sell_threshold),
    }
}

/// `orders` best first.
fn side(orders: &[&BookOrder], outliers: usize, threshold: Option<f64>) -> SideStats {
    let volume: i64 = orders.iter().map(|o| o.volume_remain).sum();
    let mut s = SideStats { orders: orders.len(), volume, outliers, threshold, best: orders.first().map(|o| o.price), ..Default::default() };
    if volume > 0 {
        let value: f64 = orders.iter().map(|o| o.price * o.volume_remain as f64).sum();
        s.average = Some(value / volume as f64);
        // at least one unit, so thin books still get a price
        let target = ((volume as f64) * 0.05).ceil().max(1.0) as i64;
        let (mut taken, mut value) = (0i64, 0.0);
        for o in orders {
            let n = o.volume_remain.min(target - taken);
            taken += n;
            value += n as f64 * o.price;
            if taken >= target {
                break;
            }
        }
        s.five_percent = Some(value / taken as f64);
    }
    s
}

// ---------------------------------------------------------------------------------------------------------------
// Fees and margins

/// Fee rates as fractions (0.03 is 3%).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fees {
    pub broker: f64,
    pub tax: f64,
}

impl Default for Fees {
    fn default() -> Self {
        Fees::from_skills(0, 0)
    }
}

impl Fees {
    pub fn from_skills(broker_relations: u8, accounting: u8) -> Self {
        Fees { broker: 0.03 - 0.003 * broker_relations as f64, tax: 0.075 * (1.0 - 0.11 * accounting as f64) }
    }
    /// From the character's trained skills, at an NPC station with no standings.
    pub fn of_character(c: &Character) -> Self {
        Fees::from_skills(c.trained_skill_level(BROKER_RELATIONS), c.trained_skill_level(ACCOUNTING))
    }
}

/// The margin calculator: buying with a buy order and selling with a sell order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Margin {
    pub buy_broker: f64,
    pub sell_broker: f64,
    pub tax: f64,
    /// Price plus broker fee.
    pub total_buy: f64,
    /// Price minus broker fee and tax.
    pub total_sell: f64,
    pub profit: f64,
    /// Profit as a share of the sell price.
    pub margin: Option<f64>,
    /// Profit as a share of what was spent.
    pub roi: Option<f64>,
}

pub fn margin(buy: f64, sell: f64, buy_broker: f64, sell_broker: f64, tax: f64) -> Margin {
    let m_buy = buy * buy_broker;
    let m_sell = sell * sell_broker;
    let m_tax = sell * tax;
    let total_buy = buy + m_buy;
    let total_sell = sell - m_sell - m_tax;
    let profit = total_sell - total_buy;
    Margin {
        buy_broker: m_buy,
        sell_broker: m_sell,
        tax: m_tax,
        total_buy,
        total_sell,
        profit,
        margin: (sell > 0.0).then(|| profit / sell),
        roi: (total_buy > 0.0).then(|| profit / total_buy),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Price ticks

/// EVE only accepts prices with at most 4 significant digits (and whole cents): the smallest step at `price`.
pub fn tick(price: f64) -> f64 {
    if price <= 0.0 {
        return 0.01;
    }
    let digits = price.log10().floor() as i32;
    10f64.powi(digits - 3).max(0.01)
}

fn round_to(price: f64, step: f64) -> f64 {
    (price / step).round() * step
}

/// The highest valid price below `price`: what undercutting a sell order at `price` costs.
pub fn price_below(price: f64) -> f64 {
    let p = round_to(price, tick(price));
    let mut below = round_to(p - tick(p), tick(p));
    // crossing a power of ten down makes the step finer (1000 → 999.9)
    let finer = tick(below);
    if finer < tick(p) {
        below = round_to(p - finer, finer);
    }
    below.max(0.01)
}

/// The lowest valid price above `price`: what outbidding a buy order at `price` costs.
pub fn price_above(price: f64) -> f64 {
    let p = round_to(price, tick(price));
    round_to(p + tick(p), tick(p + tick(p)))
}

// ---------------------------------------------------------------------------------------------------------------
// The player's orders against the competition

#[derive(Clone, Debug, PartialEq)]
pub struct Competition {
    /// The best competing price (lowest sell or highest buy), if anyone competes.
    pub best: Option<f64>,
    /// Someone has a better price.
    pub outbid: bool,
    /// Competing orders.
    pub competitors: usize,
    /// The order's place among all orders on its side (1 is the best price).
    pub rank: usize,
    /// The price to set to be the best again (one tick better than the best competitor), or the current price if
    /// the order already leads.
    pub suggested: f64,
}

/// Whether an order competes for the same buyers or sellers as `mine`: sell orders at the same station, buy orders
/// that can buy at the same station (same station; same system unless station range; anywhere for region range).
/// Jump ranges are treated as the system, since PodStack has no stargate map.
fn competes(mine: &MarketOrder, system: Option<i64>, o: &BookOrder) -> bool {
    if o.is_buy_order != mine.is_buy_order {
        return false;
    }
    if !mine.is_buy_order || o.location_id == mine.location_id {
        return o.location_id == mine.location_id;
    }
    match o.range.as_str() {
        "station" => false,
        "region" => true,
        _ => system == Some(o.system_id),
    }
}

/// How `mine` stands in its region's order book. `own` are the ids of every order of the player's characters, which
/// never count as competition.
pub fn competition(mine: &MarketOrder, book: &[BookOrder], own: &HashSet<i64>) -> Competition {
    let system = book.iter().find(|o| o.order_id == mine.order_id || o.location_id == mine.location_id).map(|o| o.system_id);
    let rivals: Vec<&BookOrder> = book.iter().filter(|o| !own.contains(&o.order_id) && competes(mine, system, o)).collect();
    let better = |p: f64| if mine.is_buy_order { p > mine.price } else { p < mine.price };
    let best = if mine.is_buy_order {
        rivals.iter().map(|o| o.price).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p))))
    } else {
        rivals.iter().map(|o| o.price).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.min(p))))
    };
    let outbid = best.is_some_and(better);
    let suggested = match best {
        Some(b) if outbid || b == mine.price => {
            if mine.is_buy_order {
                price_above(b)
            } else {
                price_below(b)
            }
        }
        _ => mine.price,
    };
    Competition {
        best,
        outbid,
        competitors: rivals.len(),
        rank: rivals.iter().filter(|o| better(o.price)).count() + 1,
        suggested,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Settings

/// The `trade` document.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct TradeSettings {
    /// Per character id: fees replacing the ones from skills.
    #[serde(default)]
    pub fees: HashMap<i64, Fees>,
    /// Per type id: the unit cost of items sold without a recorded purchase.
    #[serde(default)]
    pub custom_costs: HashMap<i64, f64>,
}

pub fn settings() -> TradeSettings {
    store::load_doc("trade")
}

pub fn save_settings(core: &Core, s: &TradeSettings) {
    store::save_doc("trade", s);
    core.emit(Event::Doc("trade".into()));
}

/// A character's fees: its override, or from its skills.
pub fn fees_for(s: &TradeSettings, c: &Character) -> Fees {
    s.fees.get(&c.id).copied().unwrap_or_else(|| Fees::of_character(c))
}

// ---------------------------------------------------------------------------------------------------------------
// Profit tracking

/// A sale matched against what the sold units cost.
#[derive(Clone, Debug, PartialEq)]
pub struct Sale {
    pub character: i64,
    pub transaction_id: i64,
    pub date: DateTime<Utc>,
    pub type_id: i64,
    pub name: String,
    pub quantity: i64,
    pub unit_price: f64,
    pub location: String,
    /// Price × quantity.
    pub revenue: f64,
    pub tax: f64,
    /// Broker fee for the sell order (0 when the sale filled someone's buy order).
    pub broker: f64,
    /// What the units cost, broker fees on the purchase included. Only for the units with a known cost.
    pub cost: f64,
    /// Units with a known cost (bought, or a custom cost).
    pub matched: i64,
    /// Revenue − fees − cost, counting only the units with a known cost.
    pub profit: f64,
}

impl Sale {
    pub fn fully_matched(&self) -> bool {
        self.matched == self.quantity
    }
    /// Profit as a share of the matched revenue.
    pub fn margin(&self) -> Option<f64> {
        let rev = self.unit_price * self.matched as f64;
        (rev > 0.0).then(|| self.profit / rev)
    }
}

/// Units bought and not yet sold.
#[derive(Clone, Debug, PartialEq)]
pub struct Holding {
    pub type_id: i64,
    pub name: String,
    pub quantity: i64,
    /// Total cost of those units, broker fees included.
    pub cost: f64,
    pub oldest: DateTime<Utc>,
}

impl Holding {
    pub fn unit_cost(&self) -> f64 {
        if self.quantity > 0 {
            self.cost / self.quantity as f64
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradeReport {
    /// Newest first.
    pub sales: Vec<Sale>,
    /// The "item warehouse": bought units not sold yet, by type.
    pub holdings: Vec<Holding>,
    /// Purchases in the history: count and ISK spent (broker fees included).
    pub bought: (usize, f64),
}

#[derive(Clone, Copy)]
struct Lot {
    quantity: i64,
    unit_cost: f64,
    date: DateTime<Utc>,
}

/// Whether one of the player's orders made this transaction (so it paid a broker fee).
fn by_own_order(t: &Transaction, orders: &[MarketOrder]) -> bool {
    orders.iter().any(|o| o.type_id == t.type_id && o.is_buy_order == t.is_buy && o.location_id == t.location_id && o.price == t.unit_price && o.issued <= t.date)
}

pub fn trade_report(chars: &[Arc<Character>], settings: &TradeSettings) -> TradeReport {
    let mut txs: Vec<(i64, &Transaction, Fees, &[MarketOrder])> = Vec::new();
    let mut seen = HashSet::new();
    for c in chars {
        let fees = fees_for(settings, c);
        let orders = c.market_orders.as_ref().map(|o| o.orders.as_slice()).unwrap_or(&[]);
        for t in c.wallet_transactions.iter().flat_map(|d| d.transactions.iter()) {
            // a trade between two of the player's characters shows up in both wallets
            if seen.insert((c.id, t.transaction_id)) {
                txs.push((c.id, t, fees, orders));
            }
        }
    }
    txs.sort_by(|a, b| a.1.date.cmp(&b.1.date).then(a.1.transaction_id.cmp(&b.1.transaction_id)));

    let mut lots: HashMap<i64, VecDeque<Lot>> = HashMap::new();
    let mut names: HashMap<i64, String> = HashMap::new();
    let mut sales = Vec::new();
    let mut bought = (0usize, 0.0f64);
    for (char_id, t, fees, orders) in txs {
        names.entry(t.type_id).or_insert_with(|| t.name.clone());
        let value = t.unit_price * t.quantity as f64;
        let broker = if by_own_order(t, orders) { value * fees.broker } else { 0.0 };
        if t.is_buy {
            let total = value + broker;
            bought.0 += 1;
            bought.1 += total;
            if t.quantity > 0 {
                lots.entry(t.type_id).or_default().push_back(Lot { quantity: t.quantity, unit_cost: total / t.quantity as f64, date: t.date });
            }
            continue;
        }
        let tax = value * fees.tax;
        let (mut left, mut cost, mut matched) = (t.quantity, 0.0, 0i64);
        if let Some(queue) = lots.get_mut(&t.type_id) {
            while left > 0 {
                let Some(lot) = queue.front_mut() else { break };
                let n = lot.quantity.min(left);
                cost += n as f64 * lot.unit_cost;
                matched += n;
                left -= n;
                lot.quantity -= n;
                if lot.quantity == 0 {
                    queue.pop_front();
                }
            }
        }
        if left > 0 {
            if let Some(&c) = settings.custom_costs.get(&t.type_id) {
                cost += left as f64 * c;
                matched += left;
            }
        }
        let share = if t.quantity > 0 { matched as f64 / t.quantity as f64 } else { 0.0 };
        let profit = (value - tax - broker) * share - cost;
        sales.push(Sale {
            character: char_id,
            transaction_id: t.transaction_id,
            date: t.date,
            type_id: t.type_id,
            name: t.name.clone(),
            quantity: t.quantity,
            unit_price: t.unit_price,
            location: t.location.name.clone(),
            revenue: value,
            tax,
            broker,
            cost,
            matched,
            profit,
        });
    }
    sales.reverse();
    let mut holdings: Vec<Holding> = lots
        .into_iter()
        .filter_map(|(type_id, q)| {
            let quantity: i64 = q.iter().map(|l| l.quantity).sum();
            (quantity > 0).then(|| Holding {
                type_id,
                name: names.get(&type_id).cloned().unwrap_or_default(),
                quantity,
                cost: q.iter().map(|l| l.quantity as f64 * l.unit_cost).sum(),
                oldest: q.iter().map(|l| l.date).min().unwrap(),
            })
        })
        .collect();
    holdings.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    TradeReport { sales, holdings, bought }
}

/// Profit totals of some sales.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProfitTotals {
    pub sales: usize,
    pub revenue: f64,
    pub cost: f64,
    pub fees: f64,
    pub profit: f64,
    /// Sales with units of unknown cost.
    pub unmatched: usize,
}

impl ProfitTotals {
    pub fn margin(&self) -> Option<f64> {
        (self.revenue > 0.0).then(|| self.profit / self.revenue)
    }
    fn add(&mut self, s: &Sale) {
        self.sales += 1;
        self.revenue += s.revenue;
        self.cost += s.cost;
        self.fees += s.tax + s.broker;
        self.profit += s.profit;
        if !s.fully_matched() {
            self.unmatched += 1;
        }
    }
}

pub fn totals<'a>(sales: impl IntoIterator<Item = &'a Sale>) -> ProfitTotals {
    let mut t = ProfitTotals::default();
    for s in sales {
        t.add(s);
    }
    t
}

/// Profit per day for the last `days` days (oldest first, days without sales included).
pub fn daily_profit(sales: &[Sale], days: i64, now: DateTime<Utc>) -> Vec<(NaiveDate, ProfitTotals)> {
    let today = now.date_naive();
    let start = today - chrono::Duration::days(days - 1);
    let mut out: Vec<(NaiveDate, ProfitTotals)> = (0..days).map(|i| (start + chrono::Duration::days(i), ProfitTotals::default())).collect();
    for s in sales {
        let d = s.date.date_naive();
        if d >= start && d <= today {
            out[(d - start).num_days() as usize].1.add(s);
        }
    }
    out
}

/// Totals per type, most profitable first.
pub fn by_item(sales: &[Sale]) -> Vec<(i64, String, ProfitTotals, i64)> {
    let mut m: HashMap<i64, (String, ProfitTotals, i64)> = HashMap::new();
    for s in sales {
        let e = m.entry(s.type_id).or_insert_with(|| (s.name.clone(), ProfitTotals::default(), 0));
        e.1.add(s);
        e.2 += s.quantity;
    }
    let mut v: Vec<_> = m.into_iter().map(|(id, (n, t, q))| (id, n, t, q)).collect();
    v.sort_by(|a, b| b.2.profit.total_cmp(&a.2.profit));
    v
}

// ---------------------------------------------------------------------------------------------------------------
// Appraisal

/// What a pasted item list contained.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedItems {
    /// (type id, quantity), in the order first seen, quantities of repeated items added up.
    pub items: Vec<(i64, i64)>,
    /// Lines that didn't name a market item.
    pub unknown: Vec<String>,
}

/// "1,234", "1 234", "1.234" (thousands) or "12": a whole quantity.
fn parse_quantity(s: &str) -> Option<i64> {
    let s = s.trim().trim_start_matches('x').trim_end_matches('x').trim();
    if s.is_empty() || !s.chars().next()?.is_ascii_digit() {
        return None;
    }
    let digits: String = s.chars().filter(|c| !matches!(c, ',' | '.' | ' ' | '\u{a0}' | '\'')).collect();
    digits.parse().ok()
}

/// Finds the item and quantity in one line of a hangar, contract, inventory, EFT fit or plain list.
fn parse_line(line: &str) -> Option<(i64, i64)> {
    let line = line.trim();
    let item = |n: &str| MARKET.by_name(n).map(|t| t.id);
    // hangar / contract / cargo scan columns: name, quantity, …
    if line.contains('\t') {
        let cols: Vec<&str> = line.split('\t').map(str::trim).collect();
        if let Some(id) = item(cols[0]) {
            let q = cols.get(1).and_then(|c| parse_quantity(c)).unwrap_or(1);
            return Some((id, q));
        }
        if let (Some(q), Some(id)) = (parse_quantity(cols[0]), cols.get(1).and_then(|c| item(c))) {
            return Some((id, q));
        }
    }
    // EFT header "[Ship, Fit name]"
    if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
        return item(inner.split(',').next()?).map(|id| (id, 1));
    }
    if let Some(id) = item(line) {
        return Some((id, 1));
    }
    // "Name x5", "Name 5", "5x Name", "5 Name"
    if let Some((name, q)) = line.rsplit_once([' ', '\t']) {
        if let (Some(id), Some(q)) = (item(name.trim_end_matches(',')), parse_quantity(q)) {
            return Some((id, q));
        }
    }
    if let Some((q, name)) = line.split_once(' ') {
        if let (Some(q), Some(id)) = (parse_quantity(q), item(name)) {
            return Some((id, q));
        }
    }
    // EFT module with a charge: "Heavy Missile Launcher II, Scourge Heavy Missile"
    if let Some((module, _)) = line.split_once(',') {
        return item(module).map(|id| (id, 1));
    }
    None
}

pub fn parse_items(text: &str) -> ParsedItems {
    let mut out = ParsedItems::default();
    let mut index: HashMap<i64, usize> = HashMap::new();
    for line in text.lines() {
        let l = line.trim();
        // EFT empty slots and section breaks
        if l.is_empty() || (l.starts_with("[Empty ") && l.ends_with(" slot]")) {
            continue;
        }
        match parse_line(l) {
            Some((id, q)) => match index.get(&id) {
                Some(&i) => out.items[i].1 += q,
                None => {
                    index.insert(id, out.items.len());
                    out.items.push((id, q));
                }
            },
            None => out.unknown.push(l.to_string()),
        }
    }
    out
}

/// An appraised line.
#[derive(Clone, Debug, PartialEq)]
pub struct Appraised {
    pub type_id: i64,
    pub name: String,
    pub quantity: i64,
    pub volume: f64,
    pub buy: Option<f64>,
    pub sell: Option<f64>,
}

impl Appraised {
    pub fn buy_total(&self) -> f64 {
        self.buy.unwrap_or(0.0) * self.quantity as f64
    }
    pub fn sell_total(&self) -> f64 {
        self.sell.unwrap_or(0.0) * self.quantity as f64
    }
}

pub fn appraise(items: &[(i64, i64)], prices: &HashMap<i64, crate::market::BestPrices>) -> Vec<Appraised> {
    items
        .iter()
        .map(|&(id, q)| {
            let t = MARKET.get(id);
            let p = prices.get(&id).copied().unwrap_or_default();
            Appraised {
                type_id: id,
                name: t.map(|t| t.name.clone()).unwrap_or_default(),
                quantity: q,
                volume: t.map(|t| t.volume).unwrap_or(0.0) * q as f64,
                buy: p.buy,
                sell: p.sell,
            }
        })
        .collect()
}

/// Whether a book order passes the market browser's location filters.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BookFilter {
    #[serde(default)]
    pub exclude_high: bool,
    #[serde(default)]
    pub exclude_low: bool,
    #[serde(default)]
    pub exclude_null: bool,
    #[serde(default)]
    pub exclude_npc: bool,
    #[serde(default)]
    pub exclude_structures: bool,
    /// Only the five trade hub stations.
    #[serde(default)]
    pub hubs_only: bool,
}

impl BookFilter {
    pub fn keep(&self, o: &BookOrder) -> bool {
        if self.hubs_only && !sde::TRADE_HUBS.iter().any(|h| h.1 == o.location_id) {
            return false;
        }
        if (self.exclude_npc && o.in_npc_station()) || (self.exclude_structures && !o.in_npc_station()) {
            return false;
        }
        match sde::MAP.system(o.system_id).map(|s| s.security_class()) {
            Some(sde::SecurityClass::High) => !self.exclude_high,
            Some(sde::SecurityClass::Low) => !self.exclude_low,
            Some(sde::SecurityClass::Null) => !self.exclude_null,
            None => true,
        }
    }
    pub fn is_default(&self) -> bool {
        *self == BookFilter::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn bo(id: i64, buy: bool, price: f64, vol: i64, loc: i64) -> BookOrder {
        BookOrder {
            order_id: id,
            type_id: 34,
            is_buy_order: buy,
            price,
            volume_remain: vol,
            volume_total: vol,
            min_volume: 1,
            location_id: loc,
            system_id: 30000142,
            range: "station".into(),
            issued: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            duration: 90,
            region_id: 10000002,
        }
    }

    #[test]
    fn stats() {
        let book = vec![
            bo(1, false, 10.0, 100, 1),
            bo(2, false, 12.0, 900, 1),
            bo(3, false, 500.0, 5, 1), // outlier: over 10× the lowest sell
            bo(4, true, 9.0, 50, 1),
            bo(5, true, 8.0, 50, 1),
            bo(6, true, 0.5, 1000, 1), // outlier: under 10% of the highest buy
        ];
        let s = book_stats(&book);
        assert_eq!((s.sell.orders, s.sell.outliers, s.sell.volume), (2, 1, 1000));
        assert_eq!(s.sell.best, Some(10.0));
        // 5% of 1000 = 50 units, all at 10
        assert_eq!(s.sell.five_percent, Some(10.0));
        assert_eq!((s.buy.orders, s.buy.outliers, s.buy.volume), (2, 1, 100));
        // 5% of 100 = 5 units at 9
        assert_eq!(s.buy.five_percent, Some(9.0));
        assert_eq!(s.spread(), Some(1.0));
        assert_eq!(book_stats(&[]).sell.best, None);
    }

    #[test]
    fn fees_and_margin() {
        let f = Fees::from_skills(5, 5);
        assert!((f.broker - 0.015).abs() < 1e-12);
        assert!((f.tax - 0.03375).abs() < 1e-12);
        let m = margin(100.0, 120.0, 0.01, 0.01, 0.05);
        assert!((m.total_buy - 101.0).abs() < 1e-9);
        assert!((m.total_sell - 112.8).abs() < 1e-9);
        assert!((m.profit - 11.8).abs() < 1e-9);
        assert_eq!(margin(0.0, 0.0, 0.0, 0.0, 0.0).roi, None);
    }

    #[test]
    fn ticks() {
        assert_eq!(tick(1234.0), 1.0);
        assert_eq!(tick(5.55), 0.01);
        assert_eq!(price_below(1234.0), 1233.0);
        assert!((price_below(1000.0) - 999.9).abs() < 1e-9);
        assert!((price_below(12_340_000.0) - 12_330_000.0).abs() < 1e-3);
        assert!((price_above(9999.0) - 10_000.0).abs() < 1e-9);
        assert!((price_above(10_000.0) - 10_010.0).abs() < 1e-9);
        assert_eq!(price_below(0.01), 0.01);
    }

    fn my_order(id: i64, buy: bool, price: f64) -> MarketOrder {
        MarketOrder { order_id: id, type_id: 34, is_buy_order: buy, price, location_id: 1, state: "active".into(), range: "station".into(), ..Default::default() }
    }

    #[test]
    fn undercuts() {
        let book = vec![bo(10, false, 100.0, 1, 1), bo(11, false, 99.0, 1, 1), bo(12, false, 90.0, 1, 2), bo(13, true, 80.0, 1, 1)];
        let own: HashSet<i64> = [10].into();
        let c = competition(&my_order(10, false, 100.0), &book, &own);
        assert!(c.outbid);
        assert_eq!((c.best, c.competitors, c.rank), (Some(99.0), 1, 2));
        assert!((c.suggested - 98.99).abs() < 1e-9);
        // my other characters' orders don't count
        let own: HashSet<i64> = [10, 11].into();
        let c = competition(&my_order(10, false, 100.0), &book, &own);
        assert!(!c.outbid);
        assert_eq!(c.suggested, 100.0);
        let mut far = bo(14, true, 85.0, 1, 3);
        far.range = "region".into();
        let book = vec![bo(13, true, 80.0, 1, 1), far];
        let c = competition(&my_order(20, true, 81.0), &book, &HashSet::new());
        assert!(c.outbid);
        assert!((c.suggested - 85.01).abs() < 1e-9);
    }

    fn tx(id: i64, day: u32, buy: bool, qty: i64, price: f64) -> Transaction {
        Transaction {
            transaction_id: id,
            date: Utc.with_ymd_and_hms(2026, 3, day, 12, 0, 0).unwrap(),
            type_id: 34,
            name: "Tritanium".into(),
            quantity: qty,
            unit_price: price,
            is_buy: buy,
            location_id: 1,
            ..Default::default()
        }
    }

    #[test]
    fn fifo_profits() {
        let mut c = Character::new(1);
        c.wallet_transactions = Some(crate::domains::wallet::TransactionsData {
            // newest first, as ESI and the kept history store them
            transactions: vec![tx(4, 4, false, 20, 10.0), tx(3, 3, false, 10, 10.0), tx(2, 2, true, 10, 6.0), tx(1, 1, true, 10, 4.0)],
        });
        let mut settings = TradeSettings::default();
        settings.fees.insert(1, Fees { broker: 0.0, tax: 0.0 });
        let r = trade_report(&[Arc::new(c.clone())], &settings);
        assert_eq!(r.sales.len(), 2);
        // oldest sale used the first lot (cost 4)
        let first = &r.sales[1];
        assert_eq!((first.matched, first.cost, first.profit), (10, 40.0, 60.0));
        // the second sale has 10 units at cost 6 and 10 of unknown cost
        let second = &r.sales[0];
        assert_eq!((second.matched, second.cost, second.profit), (10, 60.0, 40.0));
        assert!(!second.fully_matched());
        assert!(r.holdings.is_empty());
        settings.custom_costs.insert(34, 1.0);
        let r = trade_report(&[Arc::new(c)], &settings);
        assert_eq!((r.sales[0].matched, r.sales[0].profit), (20, 130.0));
        let t = totals(&r.sales);
        assert_eq!((t.sales, t.profit, t.unmatched), (2, 190.0, 0));
        let days = daily_profit(&r.sales, 7, Utc.with_ymd_and_hms(2026, 3, 5, 0, 0, 0).unwrap());
        assert_eq!(days.len(), 7);
        assert_eq!(days[5].1.profit, 130.0);
    }

    #[test]
    fn item_lists() {
        let text = "Tritanium\t1,000\tMineral\n\
                    [Rifter, My fit]\n\
                    [Empty High slot]\n\
                    200mm AutoCannon II, Republic Fleet EMP S\n\
                    Hobgoblin II x5\n\
                    3 Tritanium\n\
                    Pyerite 250\n\
                    Not an item at all";
        let p = parse_items(text);
        let rifter = MARKET.by_name("Rifter").unwrap().id;
        let ac = MARKET.by_name("200mm AutoCannon II").unwrap().id;
        let hob = MARKET.by_name("Hobgoblin II").unwrap().id;
        assert_eq!(p.items, vec![(34, 1003), (rifter, 1), (ac, 1), (hob, 5), (35, 250)]);
        assert_eq!(p.unknown, vec!["Not an item at all"]);
    }
}
