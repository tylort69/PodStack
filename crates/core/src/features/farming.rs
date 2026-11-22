//! SP farms and their economics.
//!
//! A farm is a character the player trains to extract skill injectors from: everything above its base SP (at least
//! 5,000,000, what a character needs to extract at all) is harvestable, 500,000 SP per injector. Farms are stored in
//! the `farms` document, in the order they were added.
//!
//! Profit per 30 days at each farm's current training speed:
//!
//! ```text
//!   SP/min × 43,200 min (30 days) = SP per month
//!   SP per month ÷ 500,000        = Large Skill Injectors per month (one Skill Extractor each)
//!   injectors × injector price × (1 − sell fees)   income
//!   − injectors × extractor price                  extractors
//!   − subscription (Omega or MCT, in PLEX)         = profit (negative is a loss)
//! ```
//!
//! Each price is either the market's lowest sell order (PLEX on New Eden's single PLEX market, injectors and
//! extractors at Jita 4-4), or the player's own: PLEX in ISK, everything else in PLEX (as sold in the New Eden Store,
//! whose prices drop during sales) and converted to ISK at the PLEX price. The price settings are the app setting
//! `farm_profit`.

use crate::character::{DEFAULT_FARM_BASE_SP, INJECTOR_SP};
use crate::market::{self, GLOBAL_PLEX_MARKET, JITA_44, LARGE_SKILL_INJECTOR_TYPE_ID, PLEX_TYPE_ID, SKILL_EXTRACTOR_TYPE_ID, THE_FORGE};
use crate::store;
use crate::{Core, Event};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

pub const DOC: &str = "farms";
/// App setting holding [`ProfitSettings`].
pub const SETTINGS_KEY: &str = "farm_profit";
pub const MINUTES_PER_MONTH: f64 = 30.0 * 24.0 * 60.0;

/// What a farm's training time is paid with.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Subscription {
    /// 30 days of Omega, bought with PLEX.
    #[default]
    Omega,
    /// A Multiple Character Training certificate: the farm trains alongside another character on the account.
    Mct,
    /// Already paid for (e.g. a main's own Omega time), costs the farm nothing.
    None,
}

impl Subscription {
    pub const ALL: [Subscription; 3] = [Subscription::Omega, Subscription::Mct, Subscription::None];

    pub fn label(self) -> &'static str {
        match self {
            Subscription::Omega => "Omega",
            Subscription::Mct => "MCT",
            Subscription::None => "Already paid",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Farm {
    pub id: i64,
    pub base_sp: i64,
    #[serde(default)]
    pub subscription: Subscription,
}

/// The stored farms, in the order they were added.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct FarmsDoc {
    #[serde(default)]
    pub farms: Vec<Farm>,
}

impl FarmsDoc {
    pub fn get(&self, id: i64) -> Option<&Farm> {
        self.farms.iter().find(|f| f.id == id)
    }

    /// Adds a farm, or updates an existing farm's base SP (keeping its subscription). Base SP below 5,000,000 is
    /// raised to it, since a character can't extract below that.
    pub fn upsert(&mut self, id: i64, base_sp: i64) {
        let base_sp = base_sp.max(DEFAULT_FARM_BASE_SP);
        match self.farms.iter_mut().find(|f| f.id == id) {
            Some(f) => f.base_sp = base_sp,
            None => self.farms.push(Farm { id, base_sp, subscription: Subscription::default() }),
        }
    }

    pub fn set_subscription(&mut self, id: i64, subscription: Subscription) -> bool {
        match self.farms.iter_mut().find(|f| f.id == id) {
            Some(f) => {
                f.subscription = subscription;
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, id: i64) -> bool {
        let before = self.farms.len();
        self.farms.retain(|f| f.id != id);
        self.farms.len() != before
    }
}

static FARMS: LazyLock<Mutex<FarmsDoc>> = LazyLock::new(|| Mutex::new(store::load_doc(DOC)));

fn save_and_emit(core: &Core, doc: &FarmsDoc) {
    store::save_doc(DOC, doc);
    core.emit(Event::Doc(DOC.into()));
}

/// Every farm, in the order they were added (including farms of characters that were removed).
pub fn farms() -> Vec<Farm> {
    FARMS.lock().farms.clone()
}

/// The farm for a character, if it is one.
pub fn get(id: i64) -> Option<Farm> {
    FARMS.lock().get(id).cloned()
}

/// Adds a farm or changes its base SP (see [`FarmsDoc::upsert`]).
pub fn add_or_update(core: &Core, id: i64, base_sp: i64) {
    let mut doc = FARMS.lock();
    doc.upsert(id, base_sp);
    let snapshot = doc.clone();
    drop(doc);
    save_and_emit(core, &snapshot);
}

pub fn set_subscription(core: &Core, id: i64, subscription: Subscription) {
    let mut doc = FARMS.lock();
    if !doc.set_subscription(id, subscription) {
        return;
    }
    let snapshot = doc.clone();
    drop(doc);
    save_and_emit(core, &snapshot);
}

/// Stops tracking a farm (also call this when a character is removed).
pub fn remove(core: &Core, id: i64) {
    let mut doc = FARMS.lock();
    if !doc.remove(id) {
        return;
    }
    let snapshot = doc.clone();
    drop(doc);
    save_and_emit(core, &snapshot);
}

/// The whole document (backups).
pub fn export_all() -> FarmsDoc {
    FARMS.lock().clone()
}

/// Replaces the whole document (backup restore).
pub fn import_all(core: &Core, doc: FarmsDoc) {
    *FARMS.lock() = doc.clone();
    save_and_emit(core, &doc);
}

// ---------- profit ----------

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PriceSource {
    #[default]
    Market,
    Custom,
}

/// PLEX price setting: the market's, or the player's own in ISK.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct PlexPrice {
    #[serde(default)]
    pub source: PriceSource,
    #[serde(default)]
    pub isk: Option<f64>,
}

/// Injector/extractor price setting: the market's, or the player's own in PLEX.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ItemPrice {
    #[serde(default)]
    pub source: PriceSource,
    #[serde(default)]
    pub plex: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ProfitSettings {
    /// PLEX for 30 days of Omega in the New Eden Store.
    #[serde(default = "default_omega_plex")]
    pub omega_plex: f64,
    /// Sales tax + broker fee when selling injectors, in percent (0-50).
    #[serde(default = "default_fees")]
    pub fees_percent: f64,
    #[serde(default)]
    pub plex: PlexPrice,
    #[serde(default)]
    pub injector: ItemPrice,
    #[serde(default)]
    pub extractor: ItemPrice,
    /// MCT is only sold in the New Eden Store (the certificate can't be traded any more), so its price is in PLEX.
    #[serde(default)]
    pub mct_plex: Option<f64>,
}

fn default_omega_plex() -> f64 {
    500.0
}

fn default_fees() -> f64 {
    5.0
}

impl Default for ProfitSettings {
    fn default() -> Self {
        ProfitSettings {
            omega_plex: default_omega_plex(),
            fees_percent: default_fees(),
            plex: PlexPrice::default(),
            injector: ItemPrice::default(),
            extractor: ItemPrice::default(),
            mct_plex: None,
        }
    }
}

/// The two items whose price can be the market's or the player's own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PricedItem {
    Injector,
    Extractor,
}

impl PricedItem {
    pub const ALL: [PricedItem; 2] = [PricedItem::Injector, PricedItem::Extractor];

    pub fn label(self) -> &'static str {
        match self {
            PricedItem::Injector => "Large Skill Injector",
            PricedItem::Extractor => "Skill Extractor",
        }
    }

    pub fn note(self) -> &'static str {
        match self {
            PricedItem::Injector => "sold",
            PricedItem::Extractor => "one per injector",
        }
    }

    pub fn type_id(self) -> i64 {
        match self {
            PricedItem::Injector => LARGE_SKILL_INJECTOR_TYPE_ID,
            PricedItem::Extractor => SKILL_EXTRACTOR_TYPE_ID,
        }
    }
}

impl ProfitSettings {
    pub fn item(&self, item: PricedItem) -> &ItemPrice {
        match item {
            PricedItem::Injector => &self.injector,
            PricedItem::Extractor => &self.extractor,
        }
    }

    pub fn item_mut(&mut self, item: PricedItem) -> &mut ItemPrice {
        match item {
            PricedItem::Injector => &mut self.injector,
            PricedItem::Extractor => &mut self.extractor,
        }
    }

    /// Switches the PLEX price source; switching to "my price" starts from the market price.
    pub fn set_plex_source(&mut self, source: PriceSource, market: &MarketPrices) {
        self.plex.source = source;
        if source == PriceSource::Custom && self.plex.isk.is_none() {
            self.plex.isk = market.plex.map(f64::round);
        }
    }

    /// Switches an item's price source; switching to "my price" starts from the market price converted to PLEX.
    pub fn set_item_source(&mut self, item: PricedItem, source: PriceSource, market: &MarketPrices) {
        let seed = match (market.get(item), market.plex) {
            (Some(p), Some(plex)) if plex > 0.0 => Some((p / plex).round()),
            _ => None,
        };
        let own = self.item_mut(item);
        own.source = source;
        if source == PriceSource::Custom && own.plex.is_none() {
            own.plex = seed;
        }
    }
}

/// Lowest sell prices on the market, `None` where nobody sells or loading failed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MarketPrices {
    pub plex: Option<f64>,
    pub injector: Option<f64>,
    pub extractor: Option<f64>,
}

impl MarketPrices {
    pub fn get(&self, item: PricedItem) -> Option<f64> {
        match item {
            PricedItem::Injector => self.injector,
            PricedItem::Extractor => self.extractor,
        }
    }

    /// Nothing loaded at all (ESI unreachable).
    pub fn is_empty(&self) -> bool {
        self.plex.is_none() && self.injector.is_none() && self.extractor.is_none()
    }
}

/// The market prices: PLEX on the global PLEX market, injectors and extractors at Jita 4-4. Cached for 6 hours
/// unless `force`.
pub async fn market_prices(core: &Core, force: bool) -> MarketPrices {
    let items = [LARGE_SKILL_INJECTOR_TYPE_ID, SKILL_EXTRACTOR_TYPE_ID];
    let (jita, plex) = futures::join!(core.market.jita_sell(&core.esi, &items, force), core.market.plex_price(&core.esi, force));
    MarketPrices {
        plex,
        injector: jita.get(&LARGE_SKILL_INJECTOR_TYPE_ID).copied().flatten(),
        extractor: jita.get(&SKILL_EXTRACTOR_TYPE_ID).copied().flatten(),
    }
}

/// When the market prices in use were fetched (the oldest of them), if they're all cached.
pub fn market_price_date(m: &market::Market) -> Option<DateTime<Utc>> {
    let jita = m.price_date(THE_FORGE, Some(JITA_44), &[LARGE_SKILL_INJECTOR_TYPE_ID, SKILL_EXTRACTOR_TYPE_ID])?;
    let plex = m.price_date(GLOBAL_PLEX_MARKET, None, &[PLEX_TYPE_ID])?;
    Some(jita.min(plex))
}

/// The ISK prices to calculate with; `None` where a price is missing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Prices {
    pub plex: Option<f64>,
    pub injector: Option<f64>,
    pub extractor: Option<f64>,
    /// 30 days of Omega
    pub omega: Option<f64>,
    /// 30 days of MCT
    pub mct: Option<f64>,
}

impl Prices {
    pub fn get(&self, item: PricedItem) -> Option<f64> {
        match item {
            PricedItem::Injector => self.injector,
            PricedItem::Extractor => self.extractor,
        }
    }

    /// Whether every price the calculation needs is known; the MCT price only matters if a farm pays for MCT.
    pub fn is_complete(&self, needs_mct: bool) -> bool {
        self.plex.is_some() && self.injector.is_some() && self.extractor.is_some() && self.omega.is_some() && (!needs_mct || self.mct.is_some())
    }

    pub fn subscription_cost(&self, s: Subscription) -> Option<f64> {
        match s {
            Subscription::Omega => self.omega,
            Subscription::Mct => self.mct,
            Subscription::None => Some(0.0),
        }
    }

    /// ISK one extracted injector brings in: its sale after fees, minus the extractor.
    pub fn profit_per_injector(&self, settings: &ProfitSettings) -> Option<f64> {
        Some(self.injector? * (1.0 - settings.fees_percent / 100.0) - self.extractor?)
    }
}

fn usable(v: Option<f64>) -> Option<f64> {
    v.filter(|v| v.is_finite() && *v >= 0.0)
}

/// Combines the market prices with the player's own.
pub fn resolve_prices(market: &MarketPrices, s: &ProfitSettings) -> Prices {
    let plex = match (s.plex.source, usable(s.plex.isk)) {
        (PriceSource::Custom, Some(isk)) => Some(isk),
        _ => market.plex,
    };
    let item = |i: PricedItem| {
        let own = s.item(i);
        match (own.source, usable(own.plex)) {
            (PriceSource::Custom, Some(p)) => plex.map(|x| p * x),
            _ => market.get(i),
        }
    };
    Prices {
        plex,
        injector: item(PricedItem::Injector),
        extractor: item(PricedItem::Extractor),
        omega: plex.map(|x| s.omega_plex * x),
        mct: plex.zip(usable(s.mct_plex)).map(|(x, p)| p * x),
    }
}

pub fn needs_mct(farms: &[Farm]) -> bool {
    farms.iter().any(|f| f.subscription == Subscription::Mct)
}

/// One farm's month at its current training speed; ISK amounts per 30 days.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FarmResult {
    pub sp_per_minute: f64,
    pub sp_per_month: f64,
    pub injectors_per_month: f64,
    pub income: f64,
    pub extractors: f64,
    pub subscription: f64,
    pub profit: f64,
    /// What the injectors ready now are worth after fees and extractors.
    pub ready_value: f64,
}

impl std::ops::Add for FarmResult {
    type Output = FarmResult;
    fn add(self, o: FarmResult) -> FarmResult {
        FarmResult {
            sp_per_minute: self.sp_per_minute + o.sp_per_minute,
            sp_per_month: self.sp_per_month + o.sp_per_month,
            injectors_per_month: self.injectors_per_month + o.injectors_per_month,
            income: self.income + o.income,
            extractors: self.extractors + o.extractors,
            subscription: self.subscription + o.subscription,
            profit: self.profit + o.profit,
            ready_value: self.ready_value + o.ready_value,
        }
    }
}

/// A farm's month from its training speed (`Character::sp_per_hour`) and injectors ready now. `None` when a price
/// it needs is missing.
pub fn for_farm(sp_per_hour: i64, injectors_ready: i64, subscription: Subscription, prices: &Prices, s: &ProfitSettings) -> Option<FarmResult> {
    let sp_per_minute = sp_per_hour as f64 / 60.0;
    let sp_per_month = sp_per_minute * MINUTES_PER_MONTH;
    let injectors_per_month = sp_per_month / INJECTOR_SP as f64;
    let net_injector = prices.injector? * (1.0 - s.fees_percent / 100.0);
    let extractor = prices.extractor?;
    let income = injectors_per_month * net_injector;
    let extractors = injectors_per_month * extractor;
    let subscription = prices.subscription_cost(subscription)?;
    Some(FarmResult {
        sp_per_minute,
        sp_per_month,
        injectors_per_month,
        income,
        extractors,
        subscription,
        profit: income - extractors - subscription,
        ready_value: injectors_ready as f64 * (net_injector - extractor),
    })
}

/// Progress (0-1) toward the next injector.
pub fn injector_progress(total_sp: i64, base_sp: i64) -> f64 {
    if total_sp <= base_sp {
        return 0.0;
    }
    ((total_sp - base_sp) % INJECTOR_SP) as f64 / INJECTOR_SP as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn market() -> MarketPrices {
        MarketPrices { plex: Some(5_000_000.0), injector: Some(800_000_000.0), extractor: Some(300_000_000.0) }
    }

    #[test]
    fn upsert_clamps_and_keeps_subscription() {
        let mut d = FarmsDoc::default();
        d.upsert(1, 1_000);
        assert_eq!(d.get(1).unwrap().base_sp, 5_000_000);
        assert!(d.set_subscription(1, Subscription::Mct));
        d.upsert(1, 6_000_000);
        assert_eq!(d.get(1).unwrap(), &Farm { id: 1, base_sp: 6_000_000, subscription: Subscription::Mct });
        d.upsert(2, 7_000_000);
        assert_eq!(d.farms.iter().map(|f| f.id).collect::<Vec<_>>(), vec![1, 2]);
        assert!(d.remove(1));
        assert!(!d.remove(1));
        assert!(!d.set_subscription(1, Subscription::None));
    }

    #[test]
    fn subscription_serializes_like_legacy() {
        assert_eq!(serde_json::to_string(&Subscription::None).unwrap(), "\"none\"");
        let f: Farm = serde_json::from_str(r#"{"id":5,"base_sp":5000000}"#).unwrap();
        assert_eq!(f.subscription, Subscription::Omega);
    }

    #[test]
    fn settings_defaults_fill_missing_fields() {
        let s: ProfitSettings = serde_json::from_str(r#"{"fees_percent":3}"#).unwrap();
        assert_eq!(s.omega_plex, 500.0);
        assert_eq!(s.fees_percent, 3.0);
        assert_eq!(s.plex.source, PriceSource::Market);
    }

    #[test]
    fn resolves_market_and_own_prices() {
        let mut s = ProfitSettings::default();
        let p = resolve_prices(&market(), &s);
        assert_eq!(p.plex, Some(5_000_000.0));
        assert_eq!(p.omega, Some(2_500_000_000.0));
        assert_eq!(p.mct, None);
        assert!(p.is_complete(false));
        assert!(!p.is_complete(true));

        s.plex = PlexPrice { source: PriceSource::Custom, isk: Some(4_000_000.0) };
        s.injector = ItemPrice { source: PriceSource::Custom, plex: Some(150.0) };
        s.mct_plex = Some(400.0);
        let p = resolve_prices(&market(), &s);
        assert_eq!(p.plex, Some(4_000_000.0));
        assert_eq!(p.injector, Some(600_000_000.0));
        assert_eq!(p.extractor, Some(300_000_000.0));
        assert_eq!(p.mct, Some(1_600_000_000.0));
        assert!(p.is_complete(true));

        // own prices in PLEX need a PLEX price
        let none = MarketPrices::default();
        s.plex.source = PriceSource::Market;
        let p = resolve_prices(&none, &s);
        assert_eq!(p.plex, None);
        assert_eq!(p.injector, None);
        assert!(!p.is_complete(false));
    }

    #[test]
    fn custom_source_seeds_from_market() {
        let mut s = ProfitSettings::default();
        s.set_item_source(PricedItem::Extractor, PriceSource::Custom, &market());
        assert_eq!(s.extractor.plex, Some(60.0));
        s.set_plex_source(PriceSource::Custom, &market());
        assert_eq!(s.plex.isk, Some(5_000_000.0));
        // an own price already set is kept
        s.extractor.plex = Some(70.0);
        s.set_item_source(PricedItem::Extractor, PriceSource::Market, &market());
        s.set_item_source(PricedItem::Extractor, PriceSource::Custom, &market());
        assert_eq!(s.extractor.plex, Some(70.0));
    }

    #[test]
    fn farm_month() {
        let s = ProfitSettings::default();
        let p = resolve_prices(&market(), &s);
        // 2,700 SP/h = 45 SP/min = 1,944,000 SP / 30 days = 3.888 injectors
        let r = for_farm(2_700, 2, Subscription::Omega, &p, &s).unwrap();
        assert!((r.sp_per_minute - 45.0).abs() < 1e-9);
        assert!((r.injectors_per_month - 3.888).abs() < 1e-9);
        let net = 800_000_000.0 * 0.95;
        assert!((r.income - 3.888 * net).abs() < 1e-3);
        assert!((r.extractors - 3.888 * 300_000_000.0).abs() < 1e-3);
        assert_eq!(r.subscription, 2_500_000_000.0);
        assert!((r.profit - (r.income - r.extractors - r.subscription)).abs() < 1e-6);
        assert!((r.ready_value - 2.0 * (net - 300_000_000.0)).abs() < 1e-6);
        assert_eq!(p.profit_per_injector(&s), Some(net - 300_000_000.0));

        let paid = for_farm(2_700, 0, Subscription::None, &p, &s).unwrap();
        assert_eq!(paid.subscription, 0.0);
        assert!(for_farm(2_700, 0, Subscription::Mct, &p, &s).is_none());
        let total = r + paid;
        assert!((total.injectors_per_month - 7.776).abs() < 1e-9);
    }

    #[test]
    fn progress_to_next_injector() {
        assert_eq!(injector_progress(4_000_000, 5_000_000), 0.0);
        assert_eq!(injector_progress(5_250_000, 5_000_000), 0.5);
        assert_eq!(injector_progress(6_000_000, 5_000_000), 0.0);
    }
}
