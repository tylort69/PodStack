//! Public contracts of a region (the in-game contract browser's data), for the courier finder and the deal finder.
//!
//! ESI gives a region's open public contracts in pages (refreshed every 30 minutes) and each contract's items with
//! a separate request. Items never change, so they're cached on disk for as long as the contract is listed.

use crate::esi::{Esi, EsiError};
use crate::store;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PublicContract {
    pub contract_id: i64,
    /// "item_exchange", "auction", "courier", "loan" or "unknown"
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub title: String,
    pub issuer_id: i64,
    pub issuer_corporation_id: i64,
    #[serde(default)]
    pub for_corporation: bool,
    pub date_issued: DateTime<Utc>,
    pub date_expired: DateTime<Utc>,
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
    pub start_location_id: Option<i64>,
    #[serde(default)]
    pub end_location_id: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PublicItem {
    pub record_id: i64,
    pub type_id: i64,
    pub quantity: i64,
    pub is_included: bool,
    #[serde(default)]
    pub is_blueprint_copy: bool,
    #[serde(default)]
    pub material_efficiency: Option<i64>,
    #[serde(default)]
    pub time_efficiency: Option<i64>,
    #[serde(default)]
    pub runs: Option<i64>,
}

/// How long a region's listing is kept before asking again (ESI's own cache time).
const REGION_TTL: Duration = Duration::from_secs(30 * 60);
/// Requests in flight at once when loading items.
const ITEM_CONCURRENCY: usize = 16;

fn items_path() -> std::path::PathBuf {
    store::cache_dir().join("public_contract_items.json")
}

pub struct PublicContracts {
    regions: Mutex<HashMap<i64, (Instant, Arc<Vec<PublicContract>>)>>,
    items: Mutex<HashMap<i64, Arc<Vec<PublicItem>>>>,
}

impl Default for PublicContracts {
    fn default() -> Self {
        Self::load()
    }
}

impl PublicContracts {
    pub fn load() -> Self {
        let items: HashMap<i64, Vec<PublicItem>> = store::read_json(&items_path()).unwrap_or_default();
        PublicContracts {
            regions: Mutex::new(HashMap::new()),
            items: Mutex::new(items.into_iter().map(|(k, v)| (k, Arc::new(v))).collect()),
        }
    }

    /// Writes the item cache, keeping only contracts still listed in a region loaded this session (the rest have
    /// ended, and their items are no use any more).
    fn save(&self) {
        let listed: HashSet<i64> = self.regions.lock().values().flat_map(|(_, l)| l.iter().map(|c| c.contract_id)).collect();
        let items = self.items.lock();
        let keep: HashMap<i64, &Vec<PublicItem>> =
            items.iter().filter(|(id, _)| listed.is_empty() || listed.contains(id)).map(|(k, v)| (*k, v.as_ref())).collect();
        let _ = store::write_json(&items_path(), &keep, false);
    }

    /// The region's open public contracts (every page), from memory when fetched in the last 30 minutes.
    pub async fn region(&self, esi: &Esi, region: i64, force: bool) -> Result<Arc<Vec<PublicContract>>, EsiError> {
        if !force {
            if let Some((at, list)) = self.regions.lock().get(&region) {
                if at.elapsed() < REGION_TTL {
                    return Ok(list.clone());
                }
            }
        }
        let list: Vec<PublicContract> = esi.get(format!("contracts/public/{region}/")).all_pages().await?;
        let list = Arc::new(list);
        self.regions.lock().insert(region, (Instant::now(), list.clone()));
        Ok(list)
    }

    /// When the region was last fetched, if this session has it.
    pub fn fetched(&self, region: i64) -> Option<Instant> {
        self.regions.lock().get(&region).map(|r| r.0)
    }

    pub fn cached_items(&self, contract_id: i64) -> Option<Arc<Vec<PublicItem>>> {
        self.items.lock().get(&contract_id).cloned()
    }

    /// Items of these contracts: cached ones at once, the rest fetched a few at a time. `progress(done, total)`
    /// is called as they arrive (from the async task: send it on to the UI). A contract that's gone gets no items.
    pub async fn items(&self, esi: &Esi, ids: &[i64], progress: impl Fn(usize, usize)) -> HashMap<i64, Arc<Vec<PublicItem>>> {
        use futures::stream::StreamExt;
        let mut out: HashMap<i64, Arc<Vec<PublicItem>>> = HashMap::new();
        let mut todo = Vec::new();
        {
            let cache = self.items.lock();
            for id in ids {
                match cache.get(id) {
                    Some(i) => {
                        out.insert(*id, i.clone());
                    }
                    None => todo.push(*id),
                }
            }
        }
        let total = todo.len();
        if total == 0 {
            return out;
        }
        let mut stream = futures::stream::iter(todo.into_iter().map(|id| async move {
            let r: Result<Vec<PublicItem>, EsiError> = esi.get(format!("contracts/public/items/{id}/")).all_pages().await;
            (id, r)
        }))
        .buffer_unordered(ITEM_CONCURRENCY);
        let mut done = 0;
        while let Some((id, r)) = stream.next().await {
            done += 1;
            match r {
                Ok(list) => {
                    let list = Arc::new(list);
                    self.items.lock().insert(id, list.clone());
                    out.insert(id, list);
                }
                // expired or taken since the listing: remember it as empty so it isn't asked for again
                Err(e) if e.is_not_found() || e.is_forbidden() || e.status() == Some(204) => {
                    self.items.lock().insert(id, Arc::new(vec![]));
                }
                Err(e) => tracing::debug!("public contract {id} items: {e}"),
            }
            if done % 25 == 0 || done == total {
                progress(done, total);
            }
        }
        self.save();
        out
    }

    /// The bids on a public auction, highest first.
    pub async fn bids(&self, esi: &Esi, contract_id: i64) -> Result<Vec<PublicBid>, EsiError> {
        let mut b: Vec<PublicBid> = esi.get(format!("contracts/public/bids/{contract_id}/")).all_pages().await?;
        b.sort_by(|a, b| b.amount.total_cmp(&a.amount));
        Ok(b)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PublicBid {
    pub bid_id: i64,
    pub amount: f64,
    pub date_bid: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_esi_listing() {
        let l: Vec<PublicContract> = serde_json::from_str(
            r#"[{"buyout":10000000000.01,"collateral":0,"contract_id":1,"date_expired":"2017-06-13T13:12:32Z",
                 "date_issued":"2017-06-06T13:12:32Z","days_to_complete":0,"end_location_id":60014719,"for_corporation":true,
                 "issuer_corporation_id":456,"issuer_id":123,"price":1000000.01,"reward":0,"start_location_id":60014719,
                 "title":"Ship","type":"auction","volume":0.01}]"#,
        )
        .unwrap();
        assert_eq!(l[0].kind, "auction");
        let i: Vec<PublicItem> = serde_json::from_str(
            r#"[{"is_blueprint_copy":true,"is_included":true,"item_id":123456,"material_efficiency":5,"quantity":1,
                 "record_id":123456,"runs":-1,"time_efficiency":0,"type_id":587}]"#,
        )
        .unwrap();
        assert!(i[0].is_blueprint_copy && i[0].is_included);
    }
}
