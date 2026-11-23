//! Public item exchanges and auctions compared with the market: what a contract's items are worth at a hub, and
//! how far under (or over) that it asks.
//!
//! Blueprint copies have no market of their own (an original's price would be far off), so they're left out of
//! the value and counted; types with no orders at the hub are counted as unpriced, so a contract that looks cheap
//! only because part of it couldn't be valued is easy to tell apart.

use crate::market::BestPrices;
use crate::public_contracts::{PublicContract, PublicItem};
use crate::sde::MARKET;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Which public contracts to look at.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Kind {
    #[default]
    Both,
    ItemExchange,
    Auction,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Both, Kind::ItemExchange, Kind::Auction];
    pub fn label(self) -> &'static str {
        match self {
            Kind::Both => "Item exchanges and auctions",
            Kind::ItemExchange => "Item exchanges",
            Kind::Auction => "Auctions",
        }
    }
    pub fn admits(self, kind: &str) -> bool {
        match self {
            Kind::Both => matches!(kind, "item_exchange" | "auction"),
            Kind::ItemExchange => kind == "item_exchange",
            Kind::Auction => kind == "auction",
        }
    }
}

/// The filters that work before any items are loaded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Prefilter {
    pub kind: Kind,
    pub min_price: Option<f64>,
    pub max_price: Option<f64>,
}

impl Prefilter {
    /// Whether the listing alone lets the contract through: the right kind, still open, and its asking price (the
    /// opening bid of an auction) in range.
    pub fn admits(&self, c: &PublicContract, now: DateTime<Utc>) -> bool {
        if !self.kind.admits(&c.kind) || c.date_expired <= now {
            return false;
        }
        let price = c.price.unwrap_or(0.0);
        self.min_price.is_none_or(|m| price >= m) && self.max_price.is_none_or(|m| price <= m)
    }
}

/// A contract's items valued at the hub.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Value {
    /// Items that come with the contract, at the lowest sell and highest buy orders.
    pub sell: f64,
    pub buy: f64,
    /// Items the contract asks for in return, at the same prices (they cost the buyer too).
    pub asked_sell: f64,
    pub asked_buy: f64,
    /// Included lines (not counting blueprint copies).
    pub lines: usize,
    pub units: i64,
    pub copies: usize,
    /// Types with no orders at the hub, or not on the market.
    pub unpriced: Vec<i64>,
    pub asks_items: bool,
}

impl Value {
    /// Worth to the buyer: what comes with it less what it asks for back.
    pub fn net_sell(&self) -> f64 {
        self.sell - self.asked_sell
    }
    pub fn net_buy(&self) -> f64 {
        self.buy - self.asked_buy
    }
    /// Every line could be valued.
    pub fn complete(&self) -> bool {
        self.unpriced.is_empty() && self.copies == 0
    }
}

/// Values items at hub prices. `buy` uses the highest buy order (and falls back to nothing), `sell` the lowest sell
/// order.
pub fn value(items: &[PublicItem], prices: &HashMap<i64, BestPrices>) -> Value {
    let mut v = Value::default();
    for i in items {
        if i.is_blueprint_copy {
            if i.is_included {
                v.copies += 1;
            }
            continue;
        }
        let p = prices.get(&i.type_id).copied().unwrap_or_default();
        let q = i.quantity as f64;
        if p.buy.is_none() && p.sell.is_none() && !v.unpriced.contains(&i.type_id) {
            v.unpriced.push(i.type_id);
        }
        if i.is_included {
            v.lines += 1;
            v.units += i.quantity;
            v.sell += p.sell.unwrap_or(0.0) * q;
            v.buy += p.buy.unwrap_or(0.0) * q;
        } else {
            v.asks_items = true;
            v.asked_sell += p.sell.unwrap_or(0.0) * q;
            v.asked_buy += p.buy.unwrap_or(0.0) * q;
        }
    }
    v
}

/// How much under `worth` the price is, as a share of it (0.25: a quarter under the market; negative: over it).
pub fn discount(price: f64, worth: f64) -> Option<f64> {
    (worth > 0.0).then(|| (worth - price) / worth)
}

/// What taking the contract costs now: an item exchange's price, an auction's highest bid (or opening price).
pub fn price_now(c: &PublicContract, top_bid: Option<f64>) -> f64 {
    let p = c.price.unwrap_or(0.0);
    match (c.kind.as_str(), top_bid) {
        ("auction", Some(b)) => b.max(p),
        _ => p,
    }
}

/// Whether one of the included items matches `query`: by name, or by any of its market groups ("Ships",
/// "Frigates", "Mining Barges").
pub fn matches(items: &[PublicItem], query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return true;
    }
    items.iter().filter(|i| i.is_included).any(|i| {
        let Some(t) = MARKET.get(i.type_id) else { return false };
        t.name.to_lowercase().contains(&q) || MARKET.path(t.group).iter().any(|g| g.name.to_lowercase().contains(&q))
    })
}

/// A single item and nothing asked back: the ship (or module) buyer's case.
pub fn is_single(items: &[PublicItem]) -> bool {
    items.iter().filter(|i| i.is_included).count() == 1 && items.iter().all(|i| i.is_included) && items.iter().all(|i| i.quantity == 1)
}

/// The contract's headline: its first item's name (the largest stack of the most valuable kind is often unknown
/// before pricing, so the first is what the game shows too), with how many more lines it has.
pub fn headline(items: &[PublicItem], name: impl Fn(i64) -> String) -> String {
    let included: Vec<&PublicItem> = items.iter().filter(|i| i.is_included).collect();
    match included.split_first() {
        None => "No items".into(),
        Some((first, rest)) => {
            let mut s = name(first.type_id);
            if first.quantity > 1 {
                s = format!("{s} × {}", crate::fmt::int(first.quantity));
            }
            if !rest.is_empty() {
                s.push_str(&format!(" + {} more", rest.len()));
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(type_id: i64, quantity: i64, included: bool, copy: bool) -> PublicItem {
        PublicItem { record_id: type_id, type_id, quantity, is_included: included, is_blueprint_copy: copy, ..Default::default() }
    }

    #[test]
    fn values_and_discounts() {
        let prices: HashMap<i64, BestPrices> = [(587, BestPrices { buy: Some(400.0), sell: Some(500.0) }), (34, BestPrices { buy: Some(4.0), sell: Some(5.0) })].into();
        let items = [item(587, 2, true, false), item(689, 1, true, true), item(42, 1, true, false), item(34, 10, false, false)];
        let v = value(&items, &prices);
        assert_eq!((v.sell, v.buy, v.lines, v.units, v.copies), (1000.0, 800.0, 2, 3, 1));
        assert_eq!(v.unpriced, vec![42]);
        assert!(v.asks_items && !v.complete());
        assert_eq!((v.net_sell(), v.net_buy()), (950.0, 760.0));
        assert_eq!(discount(750.0, 1000.0), Some(0.25));
        assert_eq!(discount(1250.0, 1000.0), Some(-0.25));
        assert_eq!(discount(1.0, 0.0), None);
    }

    #[test]
    fn filters() {
        let t = "2026-06-01T00:00:00Z".parse().unwrap();
        let later = "2026-06-10T00:00:00Z".parse().unwrap();
        let c = PublicContract { kind: "auction".into(), price: Some(100.0), date_issued: t, date_expired: later, ..Default::default() };
        let f = Prefilter { kind: Kind::Both, min_price: Some(50.0), max_price: Some(200.0) };
        assert!(f.admits(&c, t));
        assert!(!f.admits(&c, later), "expired");
        assert!(!Prefilter { kind: Kind::ItemExchange, ..Default::default() }.admits(&c, t));
        assert!(!Prefilter { max_price: Some(99.0), ..Default::default() }.admits(&c, t));
        assert_eq!(price_now(&c, Some(150.0)), 150.0);
        assert_eq!(price_now(&c, None), 100.0);

        let rifter = [item(587, 1, true, false)];
        assert!(is_single(&rifter));
        assert!(!is_single(&[item(587, 1, true, false), item(34, 1, false, false)]));
        assert!(matches(&rifter, "rift") && matches(&rifter, "ships") && matches(&rifter, "frigate") && !matches(&rifter, "mineral"));
        assert_eq!(headline(&[item(34, 5000, true, false), item(35, 1, true, false)], |_| "Tritanium".into()), "Tritanium × 5,000 + 1 more");
    }
}
