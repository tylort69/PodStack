//! Wallet journal, wallet transactions and market orders.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use crate::universe::Place;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const WALLET_SCOPE: &str = "esi-wallet.read_character_wallet.v1";
const ORDERS_SCOPE: &str = "esi-markets.read_character_orders.v1";

pub const DOMAINS: &[DomainSpec] = &[
    domain!("wallet_journal", "Wallet Journal", 3600, refresh_journal),
    domain!("wallet_transactions", "Wallet Transactions", 3600, refresh_transactions),
    // ESI caches orders for 20 minutes
    domain!("market_orders", "Market Orders", 1200, refresh_market_orders),
];

/// One ISK movement. ESI keeps the last 30 days.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct JournalEntry {
    pub id: i64,
    pub date: DateTime<Utc>,
    /// e.g. "player_donation", "market_transaction"
    pub ref_type: String,
    #[serde(default)]
    pub amount: f64,
    /// Balance after this entry.
    #[serde(default)]
    pub balance: Option<f64>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub first_party_id: Option<i64>,
    #[serde(default)]
    pub second_party_id: Option<i64>,
    #[serde(default)]
    pub first_party: Option<String>,
    #[serde(default)]
    pub second_party: Option<String>,
    #[serde(default)]
    pub tax: Option<f64>,
}

/// The wallet journal, newest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JournalData {
    pub entries: Vec<JournalEntry>,
}

/// A market buy or sell.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Transaction {
    pub transaction_id: i64,
    pub date: DateTime<Utc>,
    pub type_id: i64,
    pub name: String,
    pub quantity: i64,
    pub unit_price: f64,
    pub is_buy: bool,
    pub client_id: i64,
    #[serde(default)]
    pub client: Option<String>,
    pub location_id: i64,
    #[serde(default)]
    pub location: Place,
    #[serde(default)]
    pub journal_ref_id: Option<i64>,
}

impl Transaction {
    /// ISK in (sells) or out (buys, negative).
    pub fn total(&self) -> f64 {
        let t = self.quantity as f64 * self.unit_price;
        if self.is_buy {
            -t
        } else {
            t
        }
    }
}

/// Market transactions, newest first (ESI returns the latest 2500).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TransactionsData {
    pub transactions: Vec<Transaction>,
}

/// A buy or sell order: open, or ended in the last 90 days.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct MarketOrder {
    pub order_id: i64,
    pub type_id: i64,
    pub name: String,
    #[serde(default)]
    pub is_buy_order: bool,
    pub price: f64,
    pub volume_total: i64,
    pub volume_remain: i64,
    #[serde(default)]
    pub min_volume: Option<i64>,
    /// "station", "solarsystem", "region" or a jump count ("1" … "40")
    #[serde(default)]
    pub range: String,
    pub issued: DateTime<Utc>,
    /// Days.
    pub duration: i64,
    #[serde(default)]
    pub escrow: Option<f64>,
    /// "active", "filled", "expired" or "cancelled"
    pub state: String,
    pub location_id: i64,
    #[serde(default)]
    pub location: Place,
    pub region_id: i64,
    #[serde(default)]
    pub region: String,
}

impl MarketOrder {
    pub fn is_active(&self) -> bool {
        self.state == "active"
    }
    /// ISK value of what's left: listed (sell) or still to buy (buy).
    pub fn remaining_value(&self) -> f64 {
        self.price * self.volume_remain as f64
    }
}

/// Market orders, newest first.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MarketOrdersData {
    pub orders: Vec<MarketOrder>,
}

impl MarketOrdersData {
    pub fn active(&self) -> impl Iterator<Item = &MarketOrder> {
        self.orders.iter().filter(|o| o.is_active())
    }
    pub fn ended(&self) -> impl Iterator<Item = &MarketOrder> {
        self.orders.iter().filter(|o| !o.is_active())
    }
}

#[derive(Deserialize)]
struct RawJournal {
    id: i64,
    date: DateTime<Utc>,
    ref_type: String,
    #[serde(default)]
    amount: Option<f64>,
    #[serde(default)]
    balance: Option<f64>,
    #[serde(default)]
    description: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    first_party_id: Option<i64>,
    #[serde(default)]
    second_party_id: Option<i64>,
    #[serde(default)]
    tax: Option<f64>,
}

fn build_journal(raw: Vec<RawJournal>, names: &HashMap<i64, String>) -> JournalData {
    let name = |id: Option<i64>| id.and_then(|i| names.get(&i).cloned());
    let mut entries: Vec<JournalEntry> = raw
        .into_iter()
        .map(|e| JournalEntry {
            id: e.id,
            date: e.date,
            ref_type: e.ref_type,
            amount: e.amount.unwrap_or(0.0),
            balance: e.balance,
            description: e.description,
            reason: e.reason.filter(|r| !r.trim().is_empty()),
            first_party: name(e.first_party_id),
            second_party: name(e.second_party_id),
            first_party_id: e.first_party_id,
            second_party_id: e.second_party_id,
            tax: e.tax,
        })
        .collect();
    entries.sort_by(|a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id)));
    JournalData { entries }
}

async fn refresh_journal(cx: &Cx) -> DomainResult {
    let raw: Vec<RawJournal> = cx.get(cx.char_path("wallet/journal")).scope(WALLET_SCOPE).all_pages().await?;
    // collected first: a borrowing iterator held across the await isn't Send
    let ids: Vec<i64> = raw.iter().flat_map(|e| [e.first_party_id, e.second_party_id]).flatten().collect();
    let names = cx.universe().names(cx.esi(), ids).await;
    let data = build_journal(raw, &names);
    Ok(Box::new(move |c| {
        let old = c.wallet_journal.take().map(|d| d.entries).unwrap_or_default();
        c.wallet_journal = Some(JournalData { entries: merge_history(old, data.entries, |e| e.id, |a, b| b.date.cmp(&a.date).then(b.id.cmp(&a.id))) });
    }))
}

/// Most entries kept per character. ESI only serves the last 30 days (2500 transactions), so PodStack keeps what
/// it has seen to track trade profits further back.
const HISTORY_LIMIT: usize = 50_000;

/// Adds newly fetched entries to the ones kept from earlier refreshes (the new copy wins), newest first.
fn merge_history<T, K: std::hash::Hash + Eq>(
    old: Vec<T>,
    new: Vec<T>,
    key: impl Fn(&T) -> K,
    order: impl Fn(&T, &T) -> std::cmp::Ordering,
) -> Vec<T> {
    let fresh: std::collections::HashSet<K> = new.iter().map(&key).collect();
    let mut all = new;
    all.extend(old.into_iter().filter(|t| !fresh.contains(&key(t))));
    all.sort_by(order);
    all.truncate(HISTORY_LIMIT);
    all
}

#[derive(Deserialize)]
struct RawTransaction {
    transaction_id: i64,
    date: DateTime<Utc>,
    type_id: i64,
    quantity: i64,
    unit_price: f64,
    #[serde(default)]
    is_buy: bool,
    client_id: i64,
    location_id: i64,
    #[serde(default)]
    journal_ref_id: Option<i64>,
}

fn build_transactions(raw: Vec<RawTransaction>, names: &HashMap<i64, String>, places: &HashMap<i64, Place>) -> TransactionsData {
    let mut transactions: Vec<Transaction> = raw
        .into_iter()
        .map(|t| Transaction {
            transaction_id: t.transaction_id,
            date: t.date,
            type_id: t.type_id,
            name: names.get(&t.type_id).cloned().unwrap_or_else(|| format!("Type #{}", t.type_id)),
            quantity: t.quantity,
            unit_price: t.unit_price,
            is_buy: t.is_buy,
            client_id: t.client_id,
            client: names.get(&t.client_id).cloned(),
            location_id: t.location_id,
            location: places.get(&t.location_id).cloned().unwrap_or_default(),
            journal_ref_id: t.journal_ref_id,
        })
        .collect();
    transactions.sort_by(|a, b| b.date.cmp(&a.date).then(b.transaction_id.cmp(&a.transaction_id)));
    TransactionsData { transactions }
}

async fn refresh_transactions(cx: &Cx) -> DomainResult {
    let raw: Vec<RawTransaction> = cx.get(cx.char_path("wallet/transactions")).scope(WALLET_SCOPE).json().await?;
    let ids: Vec<i64> = raw.iter().flat_map(|t| [t.type_id, t.client_id]).collect();
    let names = cx.universe().names(cx.esi(), ids).await;
    let locations: Vec<i64> = raw.iter().map(|t| t.location_id).collect();
    let places = cx.universe().places(cx.esi(), locations, Some(&cx.auth)).await;
    let data = build_transactions(raw, &names, &places);
    Ok(Box::new(move |c| {
        let old = c.wallet_transactions.take().map(|d| d.transactions).unwrap_or_default();
        let transactions = merge_history(old, data.transactions, |t| t.transaction_id, |a, b| {
            b.date.cmp(&a.date).then(b.transaction_id.cmp(&a.transaction_id))
        });
        c.wallet_transactions = Some(TransactionsData { transactions });
    }))
}

#[derive(Deserialize)]
struct RawOrder {
    order_id: i64,
    type_id: i64,
    #[serde(default)]
    is_buy_order: Option<bool>,
    #[serde(default)]
    is_corporation: bool,
    price: f64,
    volume_total: i64,
    volume_remain: i64,
    #[serde(default)]
    min_volume: Option<i64>,
    #[serde(default)]
    range: String,
    issued: DateTime<Utc>,
    #[serde(default)]
    duration: i64,
    #[serde(default)]
    escrow: Option<f64>,
    /// Only history orders have one; open orders are "active".
    #[serde(default)]
    state: Option<String>,
    location_id: i64,
    region_id: i64,
}

fn build_orders(raw: Vec<RawOrder>, names: &HashMap<i64, String>, places: &HashMap<i64, Place>) -> MarketOrdersData {
    let mut orders: Vec<MarketOrder> = raw
        .into_iter()
        .filter(|o| !o.is_corporation)
        .map(|o| {
            let state = o.state.clone().unwrap_or_else(|| "active".into());
            MarketOrder {
                order_id: o.order_id,
                type_id: o.type_id,
                name: names.get(&o.type_id).cloned().unwrap_or_else(|| format!("Type #{}", o.type_id)),
                is_buy_order: o.is_buy_order.unwrap_or(false),
                price: o.price,
                volume_total: o.volume_total,
                volume_remain: o.volume_remain,
                min_volume: o.min_volume,
                range: o.range,
                issued: o.issued,
                duration: o.duration,
                escrow: o.escrow,
                // ESI reports filled orders as expired with nothing left
                state: if state == "expired" && o.volume_remain == 0 { "filled".into() } else { state },
                location_id: o.location_id,
                location: places.get(&o.location_id).cloned().unwrap_or_default(),
                region_id: o.region_id,
                region: names.get(&o.region_id).cloned().unwrap_or_default(),
            }
        })
        .collect();
    // an order can be in both lists for a moment as it ends; the open one wins
    let mut seen = std::collections::HashSet::new();
    orders.sort_by_key(|o| !o.is_active());
    orders.retain(|o| seen.insert(o.order_id));
    orders.sort_by_key(|o| std::cmp::Reverse(o.issued));
    MarketOrdersData { orders }
}

async fn refresh_market_orders(cx: &Cx) -> DomainResult {
    let mut raw: Vec<RawOrder> = cx.get(cx.char_path("orders")).scope(ORDERS_SCOPE).json().await?;
    raw.extend(cx.get(cx.char_path("orders/history")).scope(ORDERS_SCOPE).all_pages::<RawOrder>().await?);
    let ids: Vec<i64> = raw.iter().flat_map(|o| [o.type_id, o.region_id]).collect();
    let names = cx.universe().names(cx.esi(), ids).await;
    let locations: Vec<i64> = raw.iter().map(|o| o.location_id).collect();
    let places = cx.universe().places(cx.esi(), locations, Some(&cx.auth)).await;
    let data = build_orders(raw, &names, &places);
    Ok(Box::new(move |c| c.market_orders = Some(data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> HashMap<i64, String> {
        [(2112625428, "Pilot"), (1000132, "Secure Commerce Commission"), (587, "Rifter"), (10000002, "The Forge"), (54321, "Buyer")]
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect()
    }

    #[test]
    fn journal() {
        let raw: Vec<RawJournal> = serde_json::from_str(
            r#"[
            {"amount":-100000,"balance":500000.43,"context_id":4,"context_id_type":"contract_id","date":"2018-02-23T14:31:32Z",
             "description":"Contract Deposit","first_party_id":2112625428,"id":89,"ref_type":"contract_deposit","second_party_id":1000132},
            {"date":"2018-02-24T10:00:00Z","description":"Bounty","id":90,"ref_type":"bounty_prizes","reason":" ","balance":600000.43,"amount":100000}
        ]"#,
        )
        .unwrap();
        let j = build_journal(raw, &names());
        assert_eq!(j.entries[0].id, 90, "newest first");
        assert_eq!(j.entries[0].reason, None, "blank reasons dropped");
        assert_eq!(j.entries[1].amount, -100000.0);
        assert_eq!(j.entries[1].first_party.as_deref(), Some("Pilot"));
        assert_eq!(j.entries[1].second_party.as_deref(), Some("Secure Commerce Commission"));
    }

    #[test]
    fn transactions() {
        let raw: Vec<RawTransaction> = serde_json::from_str(
            r#"[{"client_id":54321,"date":"2016-10-24T09:00:00Z","is_buy":true,"is_personal":true,"journal_ref_id":67890,
                 "location_id":60003760,"quantity":3,"transaction_id":1234567890,"type_id":587,"unit_price":100.5}]"#,
        )
        .unwrap();
        let places: HashMap<i64, Place> = [(60003760, Place { id: 60003760, name: "Jita IV - Moon 4".into(), kind: "station".into(), ..Default::default() })].into();
        let t = build_transactions(raw, &names(), &places);
        let t = &t.transactions[0];
        assert_eq!(t.name, "Rifter");
        assert_eq!(t.client.as_deref(), Some("Buyer"));
        assert_eq!(t.location.name, "Jita IV - Moon 4");
        assert!((t.total() + 301.5).abs() < 1e-9);
    }

    #[test]
    fn orders() {
        let mut raw: Vec<RawOrder> = serde_json::from_str(
            r#"[{"duration":30,"escrow":45.6,"is_buy_order":true,"issued":"2016-09-03T05:12:25Z","location_id":60003760,
                 "min_volume":1,"order_id":123,"price":33.3,"range":"station","region_id":10000002,"type_id":587,
                 "volume_remain":4422,"volume_total":123456},
                {"duration":90,"issued":"2016-09-04T05:12:25Z","location_id":60003760,"order_id":124,"price":10,
                 "range":"region","region_id":10000002,"type_id":587,"volume_remain":10,"volume_total":10,"is_corporation":true}]"#,
        )
        .unwrap();
        raw.extend(
            serde_json::from_str::<Vec<RawOrder>>(
                r#"[{"duration":30,"issued":"2016-08-01T00:00:00Z","location_id":60003760,"order_id":100,"price":5,
                     "range":"region","region_id":10000002,"type_id":587,"volume_remain":0,"volume_total":5,"state":"expired",
                     "is_corporation":false},
                    {"duration":30,"issued":"2016-08-02T00:00:00Z","location_id":60003760,"order_id":101,"price":5,
                     "range":"region","region_id":10000002,"type_id":587,"volume_remain":2,"volume_total":5,"state":"cancelled",
                     "is_corporation":false}]"#,
            )
            .unwrap(),
        );
        let o = build_orders(raw, &names(), &HashMap::new());
        assert_eq!(o.orders.len(), 3, "corporation orders left out");
        assert_eq!(o.orders[0].order_id, 123);
        assert_eq!(o.orders[0].state, "active");
        assert!(o.orders[0].is_buy_order);
        assert_eq!(o.orders[0].region, "The Forge");
        let filled = o.orders.iter().find(|x| x.order_id == 100).unwrap();
        assert_eq!(filled.state, "filled");
        let cancelled = o.orders.iter().find(|x| x.order_id == 101).unwrap();
        assert_eq!(cancelled.state, "cancelled");
        assert!(!cancelled.is_buy_order);
        assert_eq!(o.active().count(), 1);
    }
}
