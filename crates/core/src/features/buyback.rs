//! Buyback programs and their math, for both ends of a buyback: the player selling loot to one ("what should this
//! pay me?") and the one running it ("is this contract asking the right price?").
//!
//! A [`Program`] prices items at a trade hub (buy orders, sell orders or halfway between), pays a default share of
//! that, and can pay a different share for, or refuse, a market group or a single type. A hauling charge per m³
//! can come off each line. [`appraise`] turns items into an [`Offer`]; [`check`] compares a contract's asking price
//! with it; [`activity`] picks buyback trades out of the contract flows.
//!
//! Programs live in the "buyback" document. The first load adds an example program, once, so a new user has
//! something to edit; it makes no claim about any real service.

use super::contracts::{Archive, Flow, Party, Period};
use crate::market::BestPrices;
use crate::sde::{MARKET, TRADE_HUBS};
use crate::{store, Core, Event};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------- programs ----------

/// Which hub price a program pays a share of.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Basis {
    /// The highest buy order: what the buyback could sell for at once.
    #[default]
    Buy,
    /// The lowest sell order.
    Sell,
    /// Halfway between the two.
    Split,
}

impl Basis {
    pub const ALL: [Basis; 3] = [Basis::Buy, Basis::Sell, Basis::Split];
    pub fn label(self) -> &'static str {
        match self {
            Basis::Buy => "Buy orders",
            Basis::Sell => "Sell orders",
            Basis::Split => "Split (buy + sell) / 2",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Basis::Buy => "buy",
            Basis::Sell => "sell",
            Basis::Split => "split",
        }
    }
    /// The unit price on this basis. Buy and sell need their own side's orders: paying a share of the sell price
    /// for something nobody buys would overpay. Split falls back to whichever side exists.
    pub fn unit(self, p: &BestPrices) -> Option<f64> {
        match self {
            Basis::Buy => p.buy,
            Basis::Sell => p.sell,
            Basis::Split => match (p.buy, p.sell) {
                (Some(b), Some(s)) => Some((b + s) / 2.0),
                (b, s) => b.or(s),
            },
        }
    }
}

/// What a rule covers: a market group (and every group under it) or one type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Target {
    Group(i64),
    Type(i64),
}

impl Target {
    /// "Ships › Frigates" or the type's name.
    pub fn label(self) -> String {
        match self {
            Target::Group(g) => {
                let path: Vec<String> = MARKET.path(g).iter().map(|g| g.name.clone()).collect();
                if path.is_empty() {
                    format!("Market group #{g}")
                } else {
                    path.join(" › ")
                }
            }
            Target::Type(t) => MARKET.get(t).map(|t| t.name.clone()).unwrap_or_else(|| format!("Type #{t}")),
        }
    }
}

/// A rule's effect.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Treatment {
    /// Pays this share (0.85 = 85%) instead of the program's default.
    Rate(f64),
    /// Not bought.
    Refuse,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub target: Target,
    pub treatment: Treatment,
}

/// Who the contracts go to.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum Receiver {
    #[default]
    Unset,
    /// One of the user's characters or corporations (the user runs the buyback).
    Character(i64),
    Corporation(i64),
    /// Someone else, by name (the user sells to their buyback).
    Name(String),
}

impl Receiver {
    /// The user's party, when the user runs this buyback.
    pub fn party(&self) -> Option<Party> {
        match self {
            Receiver::Character(id) => Some(Party::Character(*id)),
            Receiver::Corporation(id) => Some(Party::Corporation(*id)),
            _ => None,
        }
    }
    pub fn is_set(&self) -> bool {
        !matches!(self, Receiver::Unset) && !matches!(self, Receiver::Name(n) if n.trim().is_empty())
    }
    /// Whether a flow belongs to this buyback: the user's party bought as it, or the user sold to it.
    pub fn matches(&self, f: &Flow) -> bool {
        match self {
            Receiver::Unset => true,
            Receiver::Character(id) | Receiver::Corporation(id) => f.party.id() == *id || f.counterparty.as_ref().is_some_and(|c| c.0 == *id),
            Receiver::Name(n) => {
                let n = n.trim();
                f.counterparty.as_ref().is_some_and(|c| c.1.trim().eq_ignore_ascii_case(n))
                    || f.counterparty_corp.as_ref().is_some_and(|c| c.1.trim().eq_ignore_ascii_case(n))
            }
        }
    }
    /// Whether a contract assigned to `assignee` (id and name) goes to this buyback.
    pub fn is_assignee(&self, id: Option<i64>, name: Option<&str>) -> bool {
        match self {
            Receiver::Unset => true,
            Receiver::Character(r) | Receiver::Corporation(r) => id == Some(*r),
            Receiver::Name(n) => name.is_some_and(|a| a.trim().eq_ignore_ascii_case(n.trim())),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Program {
    pub id: u64,
    pub name: String,
    /// The trade hub station whose orders price the items.
    pub station: i64,
    pub basis: Basis,
    /// Default share of the hub price paid (0.9 = 90%).
    pub rate: f64,
    pub rules: Vec<Rule>,
    /// ISK per m³ (packaged) taken off each line for hauling; 0 for none.
    pub haul_per_m3: f64,
    pub receiver: Receiver,
    /// The receiver's name when it's one of the user's characters or corporations (kept for display).
    pub receiver_name: String,
    /// Where the contracts are made, as the program states it.
    pub location: String,
    pub notes: String,
    /// How far over the offer an incoming contract may ask and still only need a look (0.02 = 2%).
    pub tolerance: f64,
}

impl Default for Program {
    fn default() -> Self {
        Program {
            id: 0,
            name: "New buyback".into(),
            station: TRADE_HUBS[0].1,
            basis: Basis::Buy,
            rate: 0.9,
            rules: vec![],
            haul_per_m3: 0.0,
            receiver: Receiver::Unset,
            receiver_name: String::new(),
            location: String::new(),
            notes: String::new(),
            tolerance: 0.02,
        }
    }
}

impl Program {
    /// (hub name, station, region) of the program's prices.
    pub fn hub(&self) -> (&'static str, i64, i64) {
        let h = TRADE_HUBS.iter().find(|h| h.1 == self.station).unwrap_or(&TRADE_HUBS[0]);
        (h.0, h.1, h.3)
    }
    /// Who to make the contract to, for display.
    pub fn receiver_label(&self) -> String {
        match &self.receiver {
            Receiver::Unset => String::new(),
            Receiver::Name(n) => n.trim().to_string(),
            Receiver::Character(id) | Receiver::Corporation(id) => {
                if self.receiver_name.trim().is_empty() {
                    format!("#{id}")
                } else {
                    self.receiver_name.trim().to_string()
                }
            }
        }
    }
    /// One line: "Jita buy · 90% · 3 rules".
    pub fn summary(&self) -> String {
        let mut s = format!("{} {} · {}%", self.hub().0, self.basis.short(), percent(self.rate));
        if !self.rules.is_empty() {
            s.push_str(&format!(" · {} rule{}", self.rules.len(), if self.rules.len() == 1 { "" } else { "s" }));
        }
        if self.haul_per_m3 > 0.0 {
            s.push_str(&format!(" · hauling {} ISK/m³", trim_number(self.haul_per_m3)));
        }
        s
    }
}

/// 0.875 → "87.5".
pub fn percent(fraction: f64) -> String {
    trim_number(fraction * 100.0)
}

fn trim_number(n: f64) -> String {
    let s = format!("{n:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BuybackDoc {
    pub programs: Vec<Program>,
    pub next_id: u64,
    /// The example program was added once (deleting it doesn't bring it back).
    pub seeded: bool,
}

pub const DOC: &str = "buyback";

impl BuybackDoc {
    pub fn get(&self, id: u64) -> Option<&Program> {
        self.programs.iter().find(|p| p.id == id)
    }
    /// Adds a program with a fresh id and returns that id.
    pub fn add(&mut self, mut p: Program) -> u64 {
        self.next_id = self.next_id.max(self.programs.iter().map(|p| p.id).max().unwrap_or(0)) + 1;
        p.id = self.next_id;
        self.programs.push(p);
        self.next_id
    }
    /// Replaces the program with the same id (or adds it when it's new).
    pub fn put(&mut self, p: Program) -> u64 {
        match self.programs.iter_mut().find(|x| x.id == p.id && p.id != 0) {
            Some(x) => {
                *x = p;
                x.id
            }
            None => self.add(p),
        }
    }
    /// A copy named "… (copy)", placed after the original.
    pub fn duplicate(&mut self, id: u64) -> Option<u64> {
        let pos = self.programs.iter().position(|p| p.id == id)?;
        let mut copy = self.programs[pos].clone();
        copy.name = format!("{} (copy)", copy.name);
        let new_id = self.add(copy);
        let p = self.programs.pop()?;
        self.programs.insert(pos + 1, p);
        Some(new_id)
    }
    pub fn remove(&mut self, id: u64) {
        self.programs.retain(|p| p.id != id);
    }
}

/// The programs, with the example added on first use.
pub fn load_doc() -> BuybackDoc {
    let mut d: BuybackDoc = store::load_doc(DOC);
    if !d.seeded {
        d.seeded = true;
        if d.programs.is_empty() {
            d.add(example());
        }
        store::save_doc(DOC, &d);
    }
    d
}

pub fn save_doc(core: &Core, d: &BuybackDoc) {
    store::save_doc(DOC, d);
    core.emit(Event::Doc(DOC.into()));
}

/// A top-level market group by name ("Ships"), from the static data.
fn root_group(name: &str) -> Option<i64> {
    MARKET.roots.iter().copied().find(|r| MARKET.groups.get(r).is_some_and(|g| g.name.eq_ignore_ascii_case(name)))
}

/// A subgroup by name under `parent`.
fn child_group(parent: i64, name: &str) -> Option<i64> {
    MARKET.groups.get(&parent)?.children.iter().copied().find(|c| MARKET.groups.get(c).is_some_and(|g| g.name.eq_ignore_ascii_case(name)))
}

/// A starting point to edit: buy orders at Jita, more for raw materials, nothing for blueprints and cosmetics.
pub fn example() -> Program {
    let mut rules = Vec::new();
    if let Some(m) = root_group("Manufacture & Research").and_then(|g| child_group(g, "Materials")) {
        rules.push(Rule { target: Target::Group(m), treatment: Treatment::Rate(0.95) });
    }
    for name in ["Blueprints & Reactions", "Ship SKINs", "Apparel", "Personalization"] {
        if let Some(g) = root_group(name) {
            rules.push(Rule { target: Target::Group(g), treatment: Treatment::Refuse });
        }
    }
    Program {
        name: "Example buyback".into(),
        rules,
        location: TRADE_HUBS[0].0.to_string(),
        notes: "An example to start from: change the rates, rules and receiver to match the buyback you use or run.".into(),
        ..Default::default()
    }
}

// ---------- appraisal ----------

/// The rule that applies to a type: its own rule first, else the rule of its closest market group.
/// `groups` is the type's market group path from the top group down.
pub fn rule_for(rules: &[Rule], type_id: i64, groups: &[i64]) -> Option<Rule> {
    if let Some(r) = rules.iter().find(|r| r.target == Target::Type(type_id)) {
        return Some(*r);
    }
    groups.iter().rev().find_map(|g| rules.iter().find(|r| r.target == Target::Group(*g)).copied())
}

/// What's known about a type for appraising it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TypeFacts {
    pub name: String,
    /// Packaged m³ of one unit.
    pub volume: f64,
    /// Market group path, top group first (empty when it isn't on the market).
    pub groups: Vec<i64>,
}

/// Facts from the static data; None for types not on the market.
pub fn facts(type_id: i64) -> Option<TypeFacts> {
    let t = MARKET.get(type_id)?;
    Some(TypeFacts { name: t.name.clone(), volume: t.volume, groups: MARKET.path(t.group).iter().map(|g| g.id).collect() })
}

/// An item to appraise.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    pub type_id: i64,
    pub quantity: i64,
    /// Blueprint copies have no market price (the original's would be far off), so they're never paid for.
    pub copy: bool,
}

impl Item {
    pub fn new(type_id: i64, quantity: i64) -> Self {
        Item { type_id, quantity, copy: false }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineStatus {
    Accepted,
    /// A rule refuses it.
    Refused,
    /// No orders on the program's basis at the hub (or prices didn't load).
    Unpriced,
    BlueprintCopy,
    /// Not a market type: nothing to price it with.
    NotOnMarket,
}

impl LineStatus {
    pub fn label(self) -> &'static str {
        match self {
            LineStatus::Accepted => "Accepted",
            LineStatus::Refused => "Not accepted",
            LineStatus::Unpriced => "No price",
            LineStatus::BlueprintCopy => "Blueprint copy",
            LineStatus::NotOnMarket => "Not on market",
        }
    }
    pub fn accepted(self) -> bool {
        self == LineStatus::Accepted
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OfferLine {
    pub type_id: i64,
    pub name: String,
    pub quantity: i64,
    /// Total packaged m³.
    pub volume: f64,
    /// Unit prices at the hub.
    pub buy: Option<f64>,
    pub sell: Option<f64>,
    /// Unit price on the program's basis.
    pub unit: Option<f64>,
    /// Share paid (0 when not accepted).
    pub rate: f64,
    /// Whether a rule set the rate (rather than the program's default).
    pub by_rule: bool,
    pub status: LineStatus,
    /// Quantity × unit price on the basis.
    pub value: f64,
    /// Hauling taken off.
    pub haul: f64,
    /// What the program pays for the line: value × rate − hauling, never below zero.
    pub offer: f64,
}

impl OfferLine {
    pub fn buy_total(&self) -> f64 {
        self.buy.unwrap_or(0.0) * self.quantity as f64
    }
    pub fn sell_total(&self) -> f64 {
        self.sell.unwrap_or(0.0) * self.quantity as f64
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Offer {
    pub lines: Vec<OfferLine>,
    /// What the program pays, rounded down to whole ISK.
    pub total: f64,
    /// Accepted lines at the basis price, before the rate and hauling.
    pub value: f64,
    /// Every priced line (accepted or not) at the hub's buy and sell orders.
    pub hub_buy: f64,
    pub hub_sell: f64,
    pub haul: f64,
    pub volume: f64,
    /// Lines refused by a rule, without a price, blueprint copies, or not on the market.
    pub refused: usize,
    pub unpriced: usize,
    pub copies: usize,
}

impl Offer {
    /// The offer as a share of the accepted items' basis value.
    pub fn effective_rate(&self) -> Option<f64> {
        (self.value > 0.0).then(|| self.total / self.value)
    }
    /// Every line is bought at a known price.
    pub fn is_clean(&self) -> bool {
        self.refused == 0 && self.unpriced == 0 && self.copies == 0
    }
    pub fn units(&self) -> i64 {
        self.lines.iter().map(|l| l.quantity).sum()
    }
}

/// Appraises items against a program at these hub prices, with type facts from `facts_of`.
pub fn appraise_with(p: &Program, items: &[Item], prices: &HashMap<i64, BestPrices>, facts_of: impl Fn(i64) -> Option<TypeFacts>) -> Offer {
    let mut o = Offer::default();
    for it in items {
        let f = facts_of(it.type_id);
        let price = prices.get(&it.type_id).copied().unwrap_or_default();
        let q = it.quantity as f64;
        let volume = f.as_ref().map(|f| f.volume).unwrap_or(0.0) * q;
        let rule = f.as_ref().and_then(|f| rule_for(&p.rules, it.type_id, &f.groups));
        let unit = p.basis.unit(&price);
        let (status, rate) = if it.copy {
            (LineStatus::BlueprintCopy, 0.0)
        } else if f.is_none() {
            (LineStatus::NotOnMarket, 0.0)
        } else if let Some(Rule { treatment: Treatment::Refuse, .. }) = rule {
            (LineStatus::Refused, 0.0)
        } else if unit.is_none() {
            (LineStatus::Unpriced, 0.0)
        } else {
            let r = match rule {
                Some(Rule { treatment: Treatment::Rate(r), .. }) => r,
                _ => p.rate,
            };
            (LineStatus::Accepted, r.max(0.0))
        };
        let priced = !it.copy;
        let value = if status.accepted() { unit.unwrap_or(0.0) * q } else { 0.0 };
        let haul = if status.accepted() { p.haul_per_m3.max(0.0) * volume } else { 0.0 };
        let offer = (value * rate - haul).max(0.0);
        o.value += value;
        o.haul += haul.min(value * rate);
        o.volume += volume;
        if priced {
            o.hub_buy += price.buy.unwrap_or(0.0) * q;
            o.hub_sell += price.sell.unwrap_or(0.0) * q;
        }
        match status {
            LineStatus::Accepted => {}
            LineStatus::Refused | LineStatus::NotOnMarket => o.refused += 1,
            LineStatus::Unpriced => o.unpriced += 1,
            LineStatus::BlueprintCopy => o.copies += 1,
        }
        o.total += offer;
        o.lines.push(OfferLine {
            type_id: it.type_id,
            name: f.map(|f| f.name).unwrap_or_else(|| format!("Type #{}", it.type_id)),
            quantity: it.quantity,
            volume,
            buy: price.buy.filter(|_| priced),
            sell: price.sell.filter(|_| priced),
            unit: unit.filter(|_| priced),
            rate,
            by_rule: matches!(rule, Some(Rule { treatment: Treatment::Rate(_), .. })),
            status,
            value,
            haul,
            offer,
        });
    }
    o.total = o.total.floor();
    o
}

/// Appraises with the static data's types.
pub fn appraise(p: &Program, items: &[Item], prices: &HashMap<i64, BestPrices>) -> Offer {
    appraise_with(p, items, prices, facts)
}

/// Every type to price for these items.
pub fn types_of(items: &[Item]) -> Vec<i64> {
    let mut t: Vec<i64> = items.iter().filter(|i| !i.copy).map(|i| i.type_id).collect();
    t.sort_unstable();
    t.dedup();
    t
}

/// A whole ISK amount as EVE's contract price field takes it: digits only.
pub fn price_field(isk: f64) -> String {
    format!("{:.0}", isk.max(0.0).floor())
}

/// What to do in game to sell these items to the program.
pub fn instructions(p: &Program, o: &Offer) -> String {
    let receiver = p.receiver_label();
    let mut s = String::from("Create an Item Exchange contract\n");
    s.push_str(&format!(
        "  Availability: Private, to {}\n",
        if receiver.is_empty() { "the buyback (no receiver set in the program)".to_string() } else { receiver }
    ));
    if !p.location.trim().is_empty() {
        s.push_str(&format!("  Location: {}\n", p.location.trim()));
    }
    s.push_str(&format!("  I will receive: {} ISK\n", crate::fmt::decimal(o.total, 0)));
    s.push_str(&format!("  Description: {}\n", p.name.trim()));
    let accepted = o.lines.iter().filter(|l| l.status.accepted()).count();
    s.push_str(&format!("  Items: {} line{} · {} m³ packaged", accepted, if accepted == 1 { "" } else { "s" }, crate::fmt::decimal(o.volume, 2)));
    if o.lines.len() > accepted {
        s.push_str(&format!("\nLeave out: {}", o.lines.iter().filter(|l| !l.status.accepted()).map(|l| l.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    s
}

// ---------- checking contracts ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verdict {
    Accept,
    Review,
    OverAsking,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Accept => "Accept",
            Verdict::Review => "Review",
            Verdict::OverAsking => "Over-asking",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Check {
    pub verdict: Verdict,
    pub reasons: Vec<String>,
    pub asked: f64,
    pub offer: f64,
    /// Asked minus offer (positive: asks more than the program pays).
    pub over: f64,
    /// `over` as a share of the offer.
    pub over_share: Option<f64>,
    /// What the buyback makes selling everything at the hub's sell orders (before fees and hauling).
    pub profit_at_sell: f64,
}

/// An incoming contract against the program: `asked` is its price, `asks_items` whether it wants items back.
/// Asking up to the offer is accepted; up to `tolerance` over it, or with lines the program doesn't buy, it needs a
/// look; more than that is over-asking.
pub fn check(asked: f64, offer: &Offer, tolerance: f64, asks_items: bool) -> Check {
    let over = asked - offer.total;
    let over_share = (offer.total > 0.0).then(|| over / offer.total);
    let mut reasons = Vec::new();
    let mut verdict = if over <= 0.0 {
        Verdict::Accept
    } else if offer.total > 0.0 && over <= offer.total * tolerance.max(0.0) {
        reasons.push(format!("Asks {}% over the offer (within tolerance)", percent(over / offer.total)));
        Verdict::Review
    } else {
        reasons.push(if offer.total > 0.0 { format!("Asks {}% over the offer", percent(over / offer.total)) } else { "Nothing in it is bought".into() });
        Verdict::OverAsking
    };
    let mut flag = |cond: bool, why: String| {
        if cond {
            reasons.push(why);
            if verdict == Verdict::Accept {
                verdict = Verdict::Review;
            }
        }
    };
    let refused: Vec<&str> = offer.lines.iter().filter(|l| matches!(l.status, LineStatus::Refused | LineStatus::NotOnMarket)).map(|l| l.name.as_str()).collect();
    flag(!refused.is_empty(), format!("Not accepted: {}", refused.join(", ")));
    flag(offer.unpriced > 0, format!("{} without a hub price", count(offer.unpriced, "line")));
    flag(offer.copies > 0, format!("{} (not valued)", count(offer.copies, "blueprint copy")));
    flag(asks_items, "Asks for items in return".into());
    flag(offer.lines.is_empty(), "No items".into());
    Check { verdict, reasons, asked, offer: offer.total, over, over_share, profit_at_sell: offer.hub_sell - asked }
}

fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else if let Some(stem) = what.strip_suffix('y') {
        format!("{n} {stem}ies")
    } else {
        format!("{n} {what}s")
    }
}

// ---------- history ----------

/// Buyback trades from the finished-contract flows: item exchanges where the user's side got items for ISK
/// (bought, as a buyback) or gave items for ISK (sold, to a buyback).
#[derive(Clone, Debug, Default)]
pub struct Activity {
    pub bought: Vec<Flow>,
    pub sold: Vec<Flow>,
}

/// Whether the flow is a buyback purchase (`buying`) or sale.
pub fn is_buyback(f: &Flow, kind: &str, buying: bool) -> bool {
    if kind != "item_exchange" {
        return false;
    }
    if buying {
        !f.items_in.is_empty() && f.items_out.is_empty() && f.isk_out > f.isk_in
    } else {
        f.items_in.is_empty() && !f.items_out.is_empty() && f.isk_in > f.isk_out
    }
}

/// Buyback trades in `period`, of `receiver`'s buyback (or all), leaving out trades between the user's own
/// characters unless `internal`.
pub fn activity(flows: &[Flow], archive: &Archive, receiver: &Receiver, period: Period, now: DateTime<Utc>, internal: bool) -> Activity {
    let mut a = Activity::default();
    for f in flows {
        if (!internal && f.internal) || !period.contains(f.date, now) || !receiver.matches(f) {
            continue;
        }
        let Some(kind) = archive.get(f.contract_id).map(|e| e.contract.kind.as_str()) else { continue };
        if is_buyback(f, kind, true) {
            a.bought.push(f.clone());
        } else if is_buyback(f, kind, false) {
            a.sold.push(f.clone());
        }
    }
    a
}

/// ISK and item value of buyback trades.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    pub count: usize,
    /// ISK paid (buying) or received (selling).
    pub isk: f64,
    /// The items' valuation where there is one.
    pub value: f64,
    /// ISK of the trades that have a valuation (to compare with `value`).
    pub isk_valued: f64,
    pub unvalued: usize,
    pub volume: f64,
}

impl Summary {
    pub fn of<'a>(flows: impl IntoIterator<Item = &'a Flow>, buying: bool) -> Summary {
        let mut s = Summary::default();
        for f in flows {
            let isk = if buying { f.isk_out - f.isk_in } else { f.isk_in - f.isk_out };
            let value = if buying { f.value_in } else { f.value_out };
            s.count += 1;
            s.isk += isk;
            s.volume += f.volume;
            match value {
                Some(v) => {
                    s.value += v;
                    s.isk_valued += isk;
                }
                None => s.unvalued += 1,
            }
        }
        s
    }
    /// Buying: what the items were worth over what was paid. Selling: what was received under the items' worth
    /// (negative when the buyback paid less than the hub price).
    pub fn margin(&self, buying: bool) -> f64 {
        if buying {
            self.value - self.isk_valued
        } else {
            self.isk_valued - self.value
        }
    }
    /// ISK as a share of the items' value (the rate a seller got, or a buyback paid).
    pub fn rate(&self) -> Option<f64> {
        (self.value > 0.0).then(|| self.isk_valued / self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::contracts::{ArchivedContract, Side};
    use crate::domains::contracts::Contract;

    fn prices() -> HashMap<i64, BestPrices> {
        [
            (34, BestPrices { buy: Some(4.0), sell: Some(6.0) }),
            (587, BestPrices { buy: Some(400_000.0), sell: Some(500_000.0) }),
            (999, BestPrices { buy: None, sell: Some(100.0) }),
            (1000, BestPrices { buy: Some(10.0), sell: Some(12.0) }),
        ]
        .into()
    }

    fn facts_of(t: i64) -> Option<TypeFacts> {
        match t {
            34 => Some(TypeFacts { name: "Tritanium".into(), volume: 0.01, groups: vec![475, 533, 1857] }),
            587 => Some(TypeFacts { name: "Rifter".into(), volume: 2500.0, groups: vec![4, 1361, 64] }),
            999 => Some(TypeFacts { name: "Rare thing".into(), volume: 1.0, groups: vec![19] }),
            1000 => Some(TypeFacts { name: "Skin".into(), volume: 0.0, groups: vec![1954, 2000] }),
            _ => None,
        }
    }

    fn program() -> Program {
        Program {
            rate: 0.9,
            rules: vec![
                Rule { target: Target::Group(475), treatment: Treatment::Rate(0.95) },
                Rule { target: Target::Group(4), treatment: Treatment::Rate(0.8) },
                Rule { target: Target::Type(587), treatment: Treatment::Rate(0.7) },
                Rule { target: Target::Group(1954), treatment: Treatment::Refuse },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn rules_pick_the_closest() {
        let p = program();
        assert_eq!(rule_for(&p.rules, 34, &[475, 533]).map(|r| r.treatment), Some(Treatment::Rate(0.95)));
        // the type rule beats its group's
        assert_eq!(rule_for(&p.rules, 587, &[4, 1361]).map(|r| r.treatment), Some(Treatment::Rate(0.7)));
        assert_eq!(rule_for(&p.rules, 588, &[4, 1361]).map(|r| r.treatment), Some(Treatment::Rate(0.8)));
        assert_eq!(rule_for(&p.rules, 1, &[19]), None);
        // a deeper group's rule beats a higher one's
        let rules = [Rule { target: Target::Group(4), treatment: Treatment::Refuse }, Rule { target: Target::Group(1361), treatment: Treatment::Rate(0.5) }];
        assert_eq!(rule_for(&rules, 588, &[4, 1361, 64]).map(|r| r.treatment), Some(Treatment::Rate(0.5)));
    }

    #[test]
    fn bases() {
        let p = BestPrices { buy: Some(4.0), sell: Some(6.0) };
        assert_eq!((Basis::Buy.unit(&p), Basis::Sell.unit(&p), Basis::Split.unit(&p)), (Some(4.0), Some(6.0), Some(5.0)));
        let only_sell = BestPrices { buy: None, sell: Some(6.0) };
        assert_eq!((Basis::Buy.unit(&only_sell), Basis::Split.unit(&only_sell)), (None, Some(6.0)));
    }

    #[test]
    fn offers() {
        let mut p = program();
        let items = [Item::new(34, 1000), Item::new(587, 1), Item::new(999, 3), Item::new(1000, 2), Item::new(42, 1), Item { type_id: 587, quantity: 1, copy: true }];
        let o = appraise_with(&p, &items, &prices(), facts_of);
        let by = |i: usize| &o.lines[i];
        assert_eq!((by(0).status, by(0).offer), (LineStatus::Accepted, 4000.0 * 0.95));
        assert_eq!((by(1).rate, by(1).offer), (0.7, 280_000.0));
        assert!(by(1).by_rule);
        assert_eq!(by(2).status, LineStatus::Unpriced, "no buy orders on a buy basis");
        assert_eq!(by(3).status, LineStatus::Refused);
        assert_eq!(by(4).status, LineStatus::NotOnMarket);
        assert_eq!(by(5).status, LineStatus::BlueprintCopy);
        assert_eq!(by(5).sell, None, "copies carry no price");
        assert_eq!(o.total, (3800.0f64 + 280_000.0).floor());
        assert_eq!((o.refused, o.unpriced, o.copies), (2, 1, 1));
        assert_eq!(o.value, 4000.0 + 400_000.0);
        assert_eq!(o.hub_sell, 6000.0 + 500_000.0 + 300.0 + 24.0);
        assert!(!o.is_clean());

        // hauling comes off each line, never below zero
        p.haul_per_m3 = 200.0;
        let o = appraise_with(&p, &[Item::new(34, 1000), Item::new(587, 1)], &prices(), facts_of);
        assert_eq!(o.lines[0].offer, 3800.0 - 2000.0);
        assert_eq!(o.lines[1].offer, 0.0, "a Rifter is worth less than hauling 2,500 m³");
        assert_eq!(o.haul, 2000.0 + 280_000.0);
        assert_eq!(o.total, 1800.0);
        assert_eq!(o.effective_rate(), Some(1800.0 / 404_000.0));
    }

    #[test]
    fn verdicts() {
        let p = Program { rate: 0.9, ..Default::default() };
        let o = appraise_with(&p, &[Item::new(34, 1000)], &prices(), facts_of);
        assert_eq!(o.total, 3600.0);
        assert_eq!(check(3500.0, &o, 0.02, false).verdict, Verdict::Accept);
        let c = check(3650.0, &o, 0.02, false);
        assert_eq!(c.verdict, Verdict::Review);
        assert_eq!(c.profit_at_sell, 6000.0 - 3650.0);
        assert_eq!(check(3800.0, &o, 0.02, false).verdict, Verdict::OverAsking);
        assert_eq!(check(3500.0, &o, 0.02, true).verdict, Verdict::Review);

        let o = appraise_with(&program(), &[Item::new(34, 1000), Item::new(1000, 1)], &prices(), facts_of);
        let c = check(1000.0, &o, 0.0, false);
        assert_eq!(c.verdict, Verdict::Review);
        assert!(c.reasons[0].contains("Skin"));
        let nothing = appraise_with(&program(), &[Item::new(1000, 1)], &prices(), facts_of);
        assert_eq!(check(10.0, &nothing, 0.5, false).verdict, Verdict::OverAsking);
    }

    #[test]
    fn instructions_text() {
        let mut p = Program { name: "Loot".into(), location: "Jita 4-4".into(), receiver: Receiver::Name("Buyback Corp".into()), ..Default::default() };
        let o = appraise_with(&p, &[Item::new(34, 1000), Item::new(42, 1)], &prices(), facts_of);
        let s = instructions(&p, &o);
        assert!(s.contains("Private, to Buyback Corp") && s.contains("Jita 4-4") && s.contains("3,600 ISK") && s.contains("Leave out: Type #42"));
        assert_eq!(price_field(1234.9), "1234");
        p.receiver = Receiver::Character(5);
        p.receiver_name = "Me".into();
        assert_eq!(p.receiver_label(), "Me");
    }

    #[test]
    fn programs_doc() {
        let mut d = BuybackDoc::default();
        let a = d.add(Program { name: "A".into(), ..Default::default() });
        let b = d.add(Program { name: "B".into(), ..Default::default() });
        let c = d.duplicate(a).unwrap();
        assert_eq!(d.programs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["A", "A (copy)", "B"]);
        assert!(a != b && b != c && a != c);
        let mut edited = d.get(b).unwrap().clone();
        edited.name = "B2".into();
        d.put(edited);
        assert_eq!(d.get(b).unwrap().name, "B2");
        d.remove(a);
        assert_eq!(d.programs.len(), 2);
        assert!(d.add(Program::default()) > c);
        // the example only uses groups the static data has
        assert!(example().rules.iter().all(|r| matches!(r.target, Target::Group(g) if MARKET.groups.contains_key(&g))));
        assert!(!example().rules.is_empty());
    }

    fn flow(id: i64, party: Party, items_in: bool, isk_in: f64, isk_out: f64, counterparty: &str) -> Flow {
        Flow {
            contract_id: id,
            date: Utc::now(),
            party,
            side: Side::Acceptor,
            category: super::super::contracts::Category::Bought,
            internal: false,
            counterparty: Some((77, counterparty.into())),
            counterparty_corp: None,
            availability: "personal".into(),
            isk_in,
            isk_out,
            items_in: if items_in { vec![(34, 10)] } else { vec![] },
            items_out: if items_in { vec![] } else { vec![(34, 10)] },
            value_in: items_in.then_some(120.0),
            value_out: (!items_in).then_some(100.0),
            volume: 0.1,
        }
    }

    #[test]
    fn history() {
        let mut archive = Archive::default();
        for (id, kind) in [(1, "item_exchange"), (2, "item_exchange"), (3, "auction")] {
            archive.contracts.insert(
                id,
                ArchivedContract { contract: Contract { contract_id: id, kind: kind.into(), ..Default::default() }, ..Default::default() },
            );
        }
        let me = Party::Corporation(500);
        let flows = vec![flow(1, me, true, 0.0, 100.0, "Seller"), flow(2, Party::Character(1), false, 90.0, 0.0, "Other Buyback"), flow(3, me, true, 0.0, 50.0, "Auctioneer")];
        let a = activity(&flows, &archive, &Receiver::Unset, Period::All, Utc::now(), false);
        assert_eq!((a.bought.len(), a.sold.len()), (1, 1), "auctions aren't buybacks");
        let s = Summary::of(&a.bought, true);
        assert_eq!((s.isk, s.value, s.margin(true)), (100.0, 120.0, 20.0));
        let s = Summary::of(&a.sold, false);
        assert_eq!((s.isk, s.margin(false), s.rate()), (90.0, -10.0, Some(0.9)));

        let a = activity(&flows, &archive, &Receiver::Corporation(500), Period::All, Utc::now(), false);
        assert_eq!((a.bought.len(), a.sold.len()), (1, 0));
        let a = activity(&flows, &archive, &Receiver::Name("other buyback".into()), Period::All, Utc::now(), false);
        assert_eq!((a.bought.len(), a.sold.len()), (0, 1));
    }
}
