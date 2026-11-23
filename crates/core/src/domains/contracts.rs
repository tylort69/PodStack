//! Contracts (with items for item exchanges and auctions, and bids for auctions), parties and locations resolved:
//! the character's own, and its corporation's. ESI only lists the last 30 days; every contract seen is also kept in
//! `features::contracts`' archive, so history outlives that.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use crate::features::comms;
use crate::universe::Place;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const SCOPE: &str = "esi-contracts.read_character_contracts.v1";
pub const CORP_SCOPE: &str = "esi-contracts.read_corporation_contracts.v1";

pub const DOMAINS: &[DomainSpec] = &[
    domain!("contracts", "Contracts", 3600, refresh_contracts),
    domain!("corp_contracts", "Corporation contracts", 3600, refresh_corp_contracts),
];

/// A bid on an auction.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Bid {
    pub bid_id: i64,
    pub bidder_id: i64,
    pub amount: f64,
    pub date_bid: DateTime<Utc>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ContractItem {
    pub record_id: i64,
    pub type_id: i64,
    pub name: String,
    pub quantity: i64,
    /// true: comes with the contract; false: asked for in exchange.
    pub is_included: bool,
    #[serde(default)]
    pub is_singleton: bool,
    /// -1 singleton, -2 blueprint copy
    #[serde(default)]
    pub raw_quantity: Option<i64>,
}

impl ContractItem {
    pub fn is_blueprint_copy(&self) -> bool {
        self.raw_quantity == Some(-2)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Contract {
    pub contract_id: i64,
    /// "item_exchange", "auction", "courier", "loan" or "unknown"
    #[serde(rename = "type")]
    pub kind: String,
    /// "outstanding", "in_progress", "finished", "rejected", …
    pub status: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub for_corporation: bool,
    /// "public", "personal", "corporation" or "alliance"
    #[serde(default)]
    pub availability: String,
    pub issuer_id: i64,
    pub issuer_corporation_id: i64,
    #[serde(default)]
    pub assignee_id: Option<i64>,
    #[serde(default)]
    pub acceptor_id: Option<i64>,
    #[serde(default)]
    pub issuer_name: String,
    #[serde(default)]
    pub issuer_corporation_name: String,
    #[serde(default)]
    pub assignee_name: Option<String>,
    #[serde(default)]
    pub acceptor_name: Option<String>,
    pub date_issued: DateTime<Utc>,
    pub date_expired: DateTime<Utc>,
    #[serde(default)]
    pub date_accepted: Option<DateTime<Utc>>,
    #[serde(default)]
    pub date_completed: Option<DateTime<Utc>>,
    #[serde(default)]
    pub days_to_complete: Option<i64>,
    #[serde(default)]
    pub price: Option<f64>,
    #[serde(default)]
    pub reward: Option<f64>,
    #[serde(default)]
    pub collateral: Option<f64>,
    #[serde(default)]
    pub buyout: Option<f64>,
    #[serde(default)]
    pub volume: Option<f64>,
    #[serde(default)]
    pub start_location: Option<Place>,
    #[serde(default)]
    pub end_location: Option<Place>,
    /// Items of item exchanges and auctions.
    #[serde(default)]
    pub items: Vec<ContractItem>,
    /// Whether `items` has been fetched (it never changes after, so it's kept across refreshes).
    #[serde(default)]
    pub items_loaded: bool,
    /// Bids of auctions, highest first.
    #[serde(default)]
    pub bids: Vec<Bid>,
}

impl Contract {
    pub fn is_completed(&self) -> bool {
        comms::is_contract_completed(&self.status)
    }
    pub fn has_items(&self) -> bool {
        matches!(self.kind.as_str(), "item_exchange" | "auction")
    }
    /// Finished successfully (as opposed to expired, deleted, failed, …).
    pub fn is_finished(&self) -> bool {
        matches!(self.status.as_str(), "finished" | "finished_issuer" | "finished_contractor")
    }
    /// What an auction went for: its highest bid (the buyout is a bid too), else the opening price.
    pub fn auction_price(&self) -> Option<f64> {
        self.bids.iter().map(|b| b.amount).reduce(f64::max).or(self.price)
    }
    /// When it ended or, while open, when it was accepted or issued.
    pub fn last_date(&self) -> DateTime<Utc> {
        self.date_completed.or(self.date_accepted).unwrap_or(self.date_issued)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContractsData {
    pub contracts: Vec<Contract>,
    /// Contract slots in use: the character's own open personal contracts.
    #[serde(default)]
    pub slots_used: u32,
}

#[derive(Deserialize)]
struct RawContract {
    contract_id: i64,
    #[serde(rename = "type")]
    kind: String,
    status: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    for_corporation: bool,
    #[serde(default)]
    availability: String,
    issuer_id: i64,
    issuer_corporation_id: i64,
    #[serde(default)]
    assignee_id: Option<i64>,
    #[serde(default)]
    acceptor_id: Option<i64>,
    date_issued: DateTime<Utc>,
    date_expired: DateTime<Utc>,
    #[serde(default)]
    date_accepted: Option<DateTime<Utc>>,
    #[serde(default)]
    date_completed: Option<DateTime<Utc>>,
    #[serde(default)]
    days_to_complete: Option<i64>,
    #[serde(default)]
    price: Option<f64>,
    #[serde(default)]
    reward: Option<f64>,
    #[serde(default)]
    collateral: Option<f64>,
    #[serde(default)]
    buyout: Option<f64>,
    #[serde(default)]
    volume: Option<f64>,
    #[serde(default)]
    start_location_id: Option<i64>,
    #[serde(default)]
    end_location_id: Option<i64>,
}

#[derive(Deserialize)]
struct RawItem {
    record_id: i64,
    type_id: i64,
    quantity: i64,
    is_included: bool,
    #[serde(default)]
    is_singleton: bool,
    #[serde(default)]
    raw_quantity: Option<i64>,
}

/// ESI writes 0 for "nobody".
fn party(id: Option<i64>) -> Option<i64> {
    id.filter(|i| *i > 0)
}

/// Open personal contracts the character issued itself use one of its slots.
fn uses_slot(c: &RawContract, character_id: i64) -> bool {
    !comms::is_contract_completed(&c.status) && c.issuer_id == character_id && !c.for_corporation && c.availability != "corporation"
}

fn build(
    character_id: i64,
    raw: Vec<RawContract>,
    names: &HashMap<i64, String>,
    places: &HashMap<i64, Place>,
    items: &HashMap<i64, Vec<ContractItem>>,
    bids: &HashMap<i64, Vec<Bid>>,
) -> ContractsData {
    let slots_used = raw.iter().filter(|c| uses_slot(c, character_id)).count() as u32;
    let name = |id: i64| names.get(&id).cloned();
    let mut contracts: Vec<Contract> = raw
        .into_iter()
        .map(|c| {
            let assignee_id = party(c.assignee_id);
            let acceptor_id = party(c.acceptor_id);
            Contract {
                contract_id: c.contract_id,
                status: c.status,
                title: c.title.unwrap_or_default(),
                for_corporation: c.for_corporation,
                availability: c.availability,
                issuer_name: name(c.issuer_id).unwrap_or_else(|| format!("#{}", c.issuer_id)),
                issuer_corporation_name: name(c.issuer_corporation_id).unwrap_or_else(|| format!("#{}", c.issuer_corporation_id)),
                issuer_id: c.issuer_id,
                issuer_corporation_id: c.issuer_corporation_id,
                assignee_name: assignee_id.map(|i| name(i).unwrap_or_else(|| format!("#{i}"))),
                acceptor_name: acceptor_id.map(|i| name(i).unwrap_or_else(|| format!("#{i}"))),
                assignee_id,
                acceptor_id,
                date_issued: c.date_issued,
                date_expired: c.date_expired,
                date_accepted: c.date_accepted,
                date_completed: c.date_completed,
                days_to_complete: c.days_to_complete,
                price: c.price,
                reward: c.reward,
                collateral: c.collateral,
                buyout: c.buyout,
                volume: c.volume,
                start_location: c.start_location_id.and_then(|i| places.get(&i).cloned()),
                end_location: c.end_location_id.and_then(|i| places.get(&i).cloned()),
                items_loaded: items.contains_key(&c.contract_id),
                items: items.get(&c.contract_id).cloned().unwrap_or_default(),
                bids: bids.get(&c.contract_id).cloned().unwrap_or_default(),
                kind: c.kind,
            }
        })
        .collect();
    contracts.sort_by_key(|c| std::cmp::Reverse(c.date_issued));
    ContractsData { contracts, slots_used }
}

async fn refresh_contracts(cx: &Cx) -> DomainResult {
    let data = fetch(cx, &cx.char_path(""), SCOPE, cx.prev.contracts.as_ref()).await?;
    crate::features::contracts::archive(&cx.core, cx.id, None, &data.contracts);
    crate::features::contracts::value_new(cx).await;
    Ok(Box::new(move |c| c.contracts = Some(data)))
}

/// The corporation's contracts: what it issued, accepted or was assigned (a buyback or courier corporation's work).
async fn refresh_corp_contracts(cx: &Cx) -> DomainResult {
    let Some(corp) = cx.prev.info.as_ref().map(|i| i.corporation_id).filter(|c| *c > 0) else {
        return Ok(Box::new(|_| {}));
    };
    // NPC corporations have no contracts of their own
    if (1_000_000..2_000_000).contains(&corp) {
        return Ok(Box::new(|c| c.corp_contracts = Some(ContractsData::default())));
    }
    let base = format!("corporations/{corp}/");
    let mut data = fetch(cx, &base, CORP_SCOPE, cx.prev.corp_contracts.as_ref()).await?;
    // slots are the character's own business
    data.slots_used = 0;
    crate::features::contracts::archive(&cx.core, cx.id, Some(corp), &data.contracts);
    crate::features::contracts::value_new(cx).await;
    Ok(Box::new(move |c| c.corp_contracts = Some(data)))
}

/// Contracts under `base` ("characters/<id>/" or "corporations/<id>/"), with items and bids; what `prev` already
/// has of either is kept rather than asked for again.
async fn fetch(cx: &Cx, base: &str, scope: &'static str, prev: Option<&ContractsData>) -> Result<ContractsData, crate::esi::EsiError> {
    let raw: Vec<RawContract> = cx.get(format!("{base}contracts")).scope(scope).all_pages().await?;

    let ids = raw.iter().flat_map(|c| [Some(c.issuer_id), Some(c.issuer_corporation_id), party(c.assignee_id), party(c.acceptor_id)]).flatten().collect::<Vec<i64>>();
    let names = cx.universe().names(cx.esi(), ids).await;
    let location_ids: Vec<i64> = raw.iter().flat_map(|c| [c.start_location_id, c.end_location_id]).flatten().collect();
    let places = cx.universe().places(cx.esi(), location_ids, Some(&cx.auth)).await;

    // items never change, so only contracts not seen before are asked for theirs; the archive may have them too
    // (a contract seen by another of the user's characters)
    let mut items: HashMap<i64, Vec<ContractItem>> = HashMap::new();
    let mut bids: HashMap<i64, Vec<Bid>> = HashMap::new();
    if let Some(prev) = prev {
        for c in &prev.contracts {
            if c.items_loaded {
                items.insert(c.contract_id, c.items.clone());
            }
            // bids stop changing once the auction is over
            if c.kind == "auction" && c.is_completed() {
                bids.insert(c.contract_id, c.bids.clone());
            }
        }
    }
    let archived = crate::features::contracts::known_items(raw.iter().map(|c| c.contract_id));
    for (id, list) in archived {
        items.entry(id).or_insert(list);
    }
    let wanted: Vec<i64> = raw
        .iter()
        .filter(|c| matches!(c.kind.as_str(), "item_exchange" | "auction") && !items.contains_key(&c.contract_id))
        .map(|c| c.contract_id)
        .collect();
    let fetched = futures::future::join_all(wanted.iter().map(|id| async move {
        let r = cx.get(format!("{base}contracts/{id}/items")).scope(scope).json::<Vec<RawItem>>().await;
        (*id, r)
    }))
    .await;
    let mut raw_items: HashMap<i64, Vec<RawItem>> = HashMap::new();
    for (id, r) in fetched {
        match r {
            Ok(list) => {
                raw_items.insert(id, list);
            }
            // gone or not visible any more: there's nothing to show, so don't ask again
            Err(e) if e.is_not_found() || e.is_forbidden() => {
                raw_items.insert(id, vec![]);
            }
            Err(e) => tracing::debug!("contract {id} items: {e}"),
        }
    }
    let wanted_bids: Vec<i64> = raw.iter().filter(|c| c.kind == "auction" && !bids.contains_key(&c.contract_id)).map(|c| c.contract_id).collect();
    let fetched = futures::future::join_all(wanted_bids.iter().map(|id| async move {
        let r = cx.get(format!("{base}contracts/{id}/bids")).scope(scope).json::<Vec<Bid>>().await;
        (*id, r)
    }))
    .await;
    for (id, r) in fetched {
        match r {
            Ok(mut list) => {
                list.sort_by(|a, b| b.amount.total_cmp(&a.amount));
                bids.insert(id, list);
            }
            Err(e) => tracing::debug!("contract {id} bids: {e}"),
        }
    }
    let type_ids: Vec<i64> = raw_items.values().flatten().map(|i| i.type_id).collect();
    let type_names = cx.universe().names(cx.esi(), type_ids).await;
    for (id, list) in raw_items {
        let list = list
            .into_iter()
            .map(|i| ContractItem {
                record_id: i.record_id,
                name: type_names.get(&i.type_id).cloned().unwrap_or_else(|| format!("Type #{}", i.type_id)),
                type_id: i.type_id,
                quantity: i.quantity,
                is_included: i.is_included,
                is_singleton: i.is_singleton,
                raw_quantity: i.raw_quantity,
            })
            .collect();
        items.insert(id, list);
    }

    Ok(build(cx.id, raw, &names, &places, &items, &bids))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_contracts() {
        let raw: Vec<RawContract> = serde_json::from_str(
            r#"[
            {"acceptor_id":0,"assignee_id":0,"availability":"public","buyout":10000000,"contract_id":1,
             "date_expired":"2017-06-13T13:12:32Z","date_issued":"2017-06-06T13:12:32Z","days_to_complete":0,
             "end_location_id":60003760,"for_corporation":false,"issuer_corporation_id":98000001,"issuer_id":90000001,
             "price":1000000,"reward":0,"start_location_id":60003760,"status":"outstanding","type":"auction","volume":0.01},
            {"acceptor_id":90000002,"assignee_id":90000002,"availability":"personal","collateral":500000000,"contract_id":2,
             "date_accepted":"2017-06-07T13:12:32Z","date_expired":"2017-06-14T13:12:32Z","date_issued":"2017-06-07T10:00:00Z",
             "days_to_complete":3,"end_location_id":60008494,"for_corporation":false,"issuer_corporation_id":98000001,
             "issuer_id":90000001,"reward":20000000,"start_location_id":60003760,"status":"in_progress","type":"courier",
             "title":"Haul","volume":12000.5},
            {"availability":"corporation","contract_id":3,"date_expired":"2017-06-14T13:12:32Z","date_issued":"2017-06-01T10:00:00Z",
             "date_completed":"2017-06-02T10:00:00Z","for_corporation":true,"issuer_corporation_id":98000001,"issuer_id":90000001,
             "price":5,"status":"finished","type":"item_exchange"}
        ]"#,
        )
        .unwrap();
        let used = raw.iter().filter(|c| uses_slot(c, 90000001)).count();
        assert_eq!(used, 2);

        let names: HashMap<i64, String> =
            [(90000001, "Alice"), (90000002, "Bob"), (98000001, "Corp")].into_iter().map(|(k, v)| (k, v.to_string())).collect();
        let places: HashMap<i64, Place> = [
            (60003760, Place { id: 60003760, name: "Jita IV - Moon 4".into(), system_name: Some("Jita".into()), ..Default::default() }),
            (60008494, Place { id: 60008494, name: "Amarr VIII".into(), system_name: Some("Amarr".into()), ..Default::default() }),
        ]
        .into();
        let items_raw: Vec<RawItem> = serde_json::from_str(
            r#"[{"is_included":true,"is_singleton":false,"quantity":1,"raw_quantity":-2,"record_id":123456,"type_id":587}]"#,
        )
        .unwrap();
        let items: HashMap<i64, Vec<ContractItem>> = [(
            1,
            items_raw
                .into_iter()
                .map(|i| ContractItem {
                    record_id: i.record_id,
                    type_id: i.type_id,
                    name: "Rifter".into(),
                    quantity: i.quantity,
                    is_included: i.is_included,
                    is_singleton: i.is_singleton,
                    raw_quantity: i.raw_quantity,
                })
                .collect(),
        )]
        .into();
        let d = build(90000001, raw, &names, &places, &items, &HashMap::new());
        assert_eq!(d.slots_used, 2);
        assert_eq!(d.contracts.iter().map(|c| c.contract_id).collect::<Vec<_>>(), vec![2, 1, 3]);
        let courier = &d.contracts[0];
        assert_eq!(courier.kind, "courier");
        assert_eq!(courier.acceptor_name.as_deref(), Some("Bob"));
        assert_eq!(courier.end_location.as_ref().and_then(|p| p.system_name.as_deref()), Some("Amarr"));
        let auction = &d.contracts[1];
        assert_eq!(auction.assignee_id, None, "0 means nobody");
        assert!(auction.items_loaded && auction.items[0].is_blueprint_copy());
        assert!(d.contracts[2].is_completed());
        assert!(!d.contracts[2].items_loaded);
    }
}
