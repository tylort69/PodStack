//! Market prices: EVE's universe-wide average prices and lowest sell orders in a region or at Jita 4-4.
//! Cached for 6 hours in `$XDG_CACHE_HOME/podstack/market.json`.

use crate::esi::Esi;
use crate::store;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const THE_FORGE: i64 = 10000002;
/// Jita IV - Moon 4 - Caldari Navy Assembly Plant
pub const JITA_44: i64 = 60003760;
/// PLEX trades on one market for all of New Eden, which ESI serves as this region.
pub const GLOBAL_PLEX_MARKET: i64 = 19000001;
pub const PLEX_TYPE_ID: i64 = 44992;
pub const SKILL_EXTRACTOR_TYPE_ID: i64 = 40519;
pub const LARGE_SKILL_INJECTOR_TYPE_ID: i64 = 40520;
pub const MCT_TYPE_ID: i64 = 34133;

const MAX_AGE_HOURS: i64 = 6;
const CONCURRENCY: usize = 8;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Entry {
    date: DateTime<Utc>,
    price: Option<f64>,
}

#[derive(Default, Serialize, Deserialize)]
struct Cache {
    averages: Option<(DateTime<Utc>, HashMap<i64, f64>)>,
    /// "region" or "region:location" -> type -> lowest sell
    sell: HashMap<String, HashMap<i64, Entry>>,
}

pub struct Market {
    cache: Mutex<Cache>,
}

fn fresh(d: DateTime<Utc>) -> bool {
    d > Utc::now() - Duration::hours(MAX_AGE_HOURS)
}

fn path() -> std::path::PathBuf {
    store::cache_dir().join("market.json")
}

#[derive(Deserialize)]
struct RawPrice {
    type_id: i64,
    #[serde(default)]
    average_price: Option<f64>,
}

#[derive(Deserialize)]
struct RawOrder {
    price: f64,
    location_id: i64,
}

impl Market {
    pub fn load() -> Self {
        Market { cache: Mutex::new(store::read_json(&path()).unwrap_or_default()) }
    }

    fn save(&self) {
        let c = self.cache.lock();
        let _ = store::write_json(&path(), &*c, false);
    }

    /// Average price of every type, in one request.
    pub async fn average_prices(&self, esi: &Esi) -> HashMap<i64, f64> {
        if let Some((d, p)) = &self.cache.lock().averages {
            if fresh(*d) {
                return p.clone();
            }
        }
        match esi.get("markets/prices").json::<Vec<RawPrice>>().await {
            Ok(list) => {
                let prices: HashMap<i64, f64> = list.into_iter().filter_map(|p| p.average_price.map(|a| (p.type_id, a))).collect();
                self.cache.lock().averages = Some((Utc::now(), prices.clone()));
                self.save();
                prices
            }
            Err(err) => {
                tracing::warn!("average prices: {err}");
                self.cache.lock().averages.as_ref().map(|(_, p)| p.clone()).unwrap_or_default()
            }
        }
    }

    /// Lowest sell order per type in a region (or only at `location`), `None` where nobody sells.
    /// Types that failed to load are left out.
    pub async fn lowest_sell(&self, esi: &Esi, region: i64, location: Option<i64>, type_ids: &[i64], force: bool) -> HashMap<i64, Option<f64>> {
        let key = match location {
            Some(l) => format!("{region}:{l}"),
            None => region.to_string(),
        };
        let missing: Vec<i64> = {
            let c = self.cache.lock();
            let m = c.sell.get(&key);
            type_ids.iter().copied().filter(|id| force || !m.and_then(|m| m.get(id)).map(|e| fresh(e.date)).unwrap_or(false)).collect()
        };
        for chunk in missing.chunks(CONCURRENCY) {
            let futs = chunk.iter().map(|&t| async move {
                let r = esi
                    .get(format!("markets/{region}/orders"))
                    .query("order_type", "sell")
                    .query("type_id", t)
                    .all_pages::<RawOrder>()
                    .await;
                (t, r)
            });
            for (t, r) in futures::future::join_all(futs).await {
                match r {
                    Ok(orders) => {
                        let lowest = orders
                            .iter()
                            .filter(|o| location.map(|l| o.location_id == l).unwrap_or(true))
                            .map(|o| o.price)
                            .fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.min(p))));
                        self.cache.lock().sell.entry(key.clone()).or_default().insert(t, Entry { date: Utc::now(), price: lowest });
                    }
                    Err(err) => tracing::warn!("orders for {t} in {region}: {err}"),
                }
            }
        }
        if !missing.is_empty() {
            self.save();
        }
        let c = self.cache.lock();
        let m = c.sell.get(&key);
        type_ids.iter().filter_map(|id| m.and_then(|m| m.get(id)).map(|e| (*id, e.price))).collect()
    }

    pub async fn jita_sell(&self, esi: &Esi, type_ids: &[i64], force: bool) -> HashMap<i64, Option<f64>> {
        self.lowest_sell(esi, THE_FORGE, Some(JITA_44), type_ids, force).await
    }

    pub async fn forge_sell(&self, esi: &Esi, type_ids: &[i64], force: bool) -> HashMap<i64, Option<f64>> {
        self.lowest_sell(esi, THE_FORGE, None, type_ids, force).await
    }

    pub async fn plex_price(&self, esi: &Esi, force: bool) -> Option<f64> {
        self.lowest_sell(esi, GLOBAL_PLEX_MARKET, None, &[PLEX_TYPE_ID], force).await.get(&PLEX_TYPE_ID).copied().flatten()
    }

    /// When the oldest of these cached prices was fetched (None if any isn't cached).
    pub fn price_date(&self, region: i64, location: Option<i64>, type_ids: &[i64]) -> Option<DateTime<Utc>> {
        let key = match location {
            Some(l) => format!("{region}:{l}"),
            None => region.to_string(),
        };
        let c = self.cache.lock();
        let m = c.sell.get(&key)?;
        let mut oldest: Option<DateTime<Utc>> = None;
        for id in type_ids {
            let d = m.get(id)?.date;
            oldest = Some(oldest.map_or(d, |o| o.min(d)));
        }
        oldest
    }
}
