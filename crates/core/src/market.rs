//! Market prices: EVE's universe-wide average prices and lowest sell orders in a region or at Jita 4-4 (cached for
//! 6 hours in `$XDG_CACHE_HOME/podstack/market.json`), and for the market browser, full order books and daily price
//! history (kept in memory for as long as ESI caches them).

use crate::esi::{Esi, EsiError};
use crate::store;
use chrono::{DateTime, Duration, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration as StdDuration, Instant};

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
    books: Mutex<Fetched<BookOrder>>,
    histories: Mutex<Fetched<HistoryDay>>,
}

/// (region, type) → when it was fetched, and what.
type Fetched<T> = HashMap<(i64, i64), (Instant, Arc<Vec<T>>)>;

/// ESI refreshes order books every 5 minutes.
const BOOK_MAX_AGE: StdDuration = StdDuration::from_secs(300);
/// Market history changes once a day (at downtime); an hour keeps a long session reasonably current.
const HISTORY_MAX_AGE: StdDuration = StdDuration::from_secs(3600);

/// An open order in a region's order book.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct BookOrder {
    pub order_id: i64,
    pub type_id: i64,
    pub is_buy_order: bool,
    pub price: f64,
    pub volume_remain: i64,
    pub volume_total: i64,
    #[serde(default)]
    pub min_volume: i64,
    pub location_id: i64,
    pub system_id: i64,
    /// "station", "solarsystem", "region" or a jump count
    pub range: String,
    pub issued: DateTime<Utc>,
    /// Days.
    pub duration: i64,
    /// Filled in by `Market::orders`; ESI's order book doesn't say.
    #[serde(default)]
    pub region_id: i64,
}

impl BookOrder {
    pub fn expires(&self) -> DateTime<Utc> {
        self.issued + Duration::days(self.duration)
    }
    /// NPC stations; everything else is a player structure.
    pub fn in_npc_station(&self) -> bool {
        (60_000_000..64_000_000).contains(&self.location_id)
    }
}

/// One day of a region's trading in a type.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct HistoryDay {
    pub date: chrono::NaiveDate,
    pub average: f64,
    pub highest: f64,
    pub lowest: f64,
    pub order_count: i64,
    pub volume: i64,
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

/// Highest buy and lowest sell among some orders.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BestPrices {
    pub buy: Option<f64>,
    pub sell: Option<f64>,
}

impl BestPrices {
    pub fn of<'a>(orders: impl Iterator<Item = &'a BookOrder>) -> Self {
        let mut b = BestPrices::default();
        for o in orders {
            if o.is_buy_order {
                b.buy = Some(b.buy.map_or(o.price, |p| p.max(o.price)));
            } else {
                b.sell = Some(b.sell.map_or(o.price, |p| p.min(o.price)));
            }
        }
        b
    }
}

#[derive(Deserialize)]
struct RawOrder {
    price: f64,
    location_id: i64,
}

impl Market {
    pub fn load() -> Self {
        Market {
            cache: Mutex::new(store::read_json(&path()).unwrap_or_default()),
            books: Mutex::new(HashMap::new()),
            histories: Mutex::new(HashMap::new()),
        }
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

    /// Every open order for a type in a region (both sides).
    pub async fn orders(&self, esi: &Esi, region: i64, type_id: i64, force: bool) -> Result<Arc<Vec<BookOrder>>, EsiError> {
        if !force {
            if let Some((at, book)) = self.books.lock().get(&(region, type_id)) {
                if at.elapsed() < BOOK_MAX_AGE {
                    return Ok(book.clone());
                }
            }
        }
        let mut orders: Vec<BookOrder> = esi
            .get(format!("markets/{region}/orders"))
            .query("order_type", "all")
            .query("type_id", type_id)
            .all_pages()
            .await?;
        for o in &mut orders {
            o.region_id = region;
        }
        let book = Arc::new(orders);
        self.books.lock().insert((region, type_id), (Instant::now(), book.clone()));
        Ok(book)
    }

    /// Open orders for a type across several regions, fetched a few regions at a time. Regions that fail are left out
    /// and counted in the second value.
    pub async fn orders_in(&self, esi: &Esi, regions: &[i64], type_id: i64, force: bool) -> (Vec<BookOrder>, usize) {
        let mut all = Vec::new();
        let mut failed = 0;
        for chunk in regions.chunks(CONCURRENCY) {
            let futs = chunk.iter().map(|&r| self.orders(esi, r, type_id, force));
            for (r, result) in chunk.iter().zip(futures::future::join_all(futs).await) {
                match result {
                    Ok(book) => all.extend(book.iter().cloned()),
                    Err(err) => {
                        tracing::warn!("orders for {type_id} in {r}: {err}");
                        failed += 1;
                    }
                }
            }
        }
        (all, failed)
    }

    /// Daily trading in a type over the last year or so, oldest first.
    pub async fn history(&self, esi: &Esi, region: i64, type_id: i64) -> Result<Arc<Vec<HistoryDay>>, EsiError> {
        if let Some((at, h)) = self.histories.lock().get(&(region, type_id)) {
            if at.elapsed() < HISTORY_MAX_AGE {
                return Ok(h.clone());
            }
        }
        let mut days: Vec<HistoryDay> = esi.get(format!("markets/{region}/history")).query("type_id", type_id).json().await?;
        days.sort_by_key(|d| d.date);
        let h = Arc::new(days);
        self.histories.lock().insert((region, type_id), (Instant::now(), h.clone()));
        Ok(h)
    }

    /// Best buy and sell price per type in a region, optionally only orders at one station or structure. Uses the
    /// same cached order books as the browser. Types that failed to load are left out. PLEX is priced on New Eden's
    /// single PLEX market wherever it's asked for.
    pub async fn best_prices(&self, esi: &Esi, region: i64, location: Option<i64>, type_ids: &[i64], force: bool) -> HashMap<i64, BestPrices> {
        let mut out = HashMap::new();
        let market_of = |t: i64| if t == PLEX_TYPE_ID { (GLOBAL_PLEX_MARKET, None) } else { (region, location) };
        for chunk in type_ids.chunks(CONCURRENCY) {
            let futs = chunk.iter().map(|&t| self.orders(esi, market_of(t).0, t, force));
            for (&t, result) in chunk.iter().zip(futures::future::join_all(futs).await) {
                let location = market_of(t).1;
                match result {
                    Ok(book) => {
                        out.insert(t, BestPrices::of(book.iter().filter(|o| location.is_none_or(|l| o.location_id == l))));
                    }
                    Err(err) => tracing::warn!("orders for {t} in {region}: {err}"),
                }
            }
        }
        out
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
