//! Where ISK comes from and goes to: every wallet journal entry put in a category by its `ref_type` (market lines
//! by what was traded, through the matching wallet transaction), and totals per category, activity, character and
//! day over a chosen period. Behind the Wallet page.

use crate::character::Character;
use crate::domains::wallet::{JournalEntry, Transaction};
use crate::sde::MARKET;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use std::collections::HashMap;
use std::sync::Arc;

/// How far back the analysis looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Period {
    Days(i64),
    /// Everything PodStack has kept (ESI serves 30 days; older entries are the ones seen on earlier refreshes).
    All,
}

impl Period {
    pub const ALL: [Period; 4] = [Period::Days(7), Period::Days(30), Period::Days(90), Period::All];

    pub fn label(self) -> String {
        match self {
            Period::Days(d) => format!("{d} days"),
            Period::All => "All".into(),
        }
    }

    /// For `ctx.remember`: "7", "30", "90" or "all".
    pub fn key(self) -> String {
        match self {
            Period::Days(d) => d.to_string(),
            Period::All => "all".into(),
        }
    }

    pub fn from_key(k: &str) -> Option<Period> {
        match k {
            "all" => Some(Period::All),
            _ => k.parse().ok().filter(|d| *d > 0).map(Period::Days),
        }
    }

    pub fn since(self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Period::Days(d) => Some(now - Duration::days(d)),
            Period::All => None,
        }
    }
}

/// Which side of the ledger a category's ISK counts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Income; the odd negative entry (a collateral paid for a mission) is netted against it.
    Income,
    /// Spending; refunds (an escrow returned) are netted against it.
    Spending,
    /// Money in is income and money out is spending (contracts, donations, insurance).
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Ratting,
    Missions,
    Rewards,
    SalesMining,
    SalesPlanetary,
    SalesShips,
    SalesOther,
    SkillTrading,
    Plex,
    MarketPurchases,
    TaxesFees,
    Contracts,
    ContractEscrow,
    Industry,
    Planetary,
    Insurance,
    Donations,
    Corporation,
    SkillsClones,
    LoyaltyStore,
    Services,
    Other,
}

impl Category {
    pub const ALL: [Category; 22] = [
        Category::Ratting,
        Category::Missions,
        Category::Rewards,
        Category::SalesMining,
        Category::SalesPlanetary,
        Category::SalesShips,
        Category::SalesOther,
        Category::SkillTrading,
        Category::Plex,
        Category::MarketPurchases,
        Category::TaxesFees,
        Category::Contracts,
        Category::ContractEscrow,
        Category::Industry,
        Category::Planetary,
        Category::Insurance,
        Category::Donations,
        Category::Corporation,
        Category::SkillsClones,
        Category::LoyaltyStore,
        Category::Services,
        Category::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Category::Ratting => "Ratting (bounties & ESS)",
            Category::Missions => "Mission rewards",
            Category::Rewards => "Incursions & other rewards",
            Category::SalesMining => "Market sales: ore & minerals",
            Category::SalesPlanetary => "Market sales: planetary materials",
            Category::SalesShips => "Market sales: ships",
            Category::SalesOther => "Market sales: other goods",
            Category::SkillTrading => "Skill injectors & extractors",
            Category::Plex => "PLEX",
            Category::MarketPurchases => "Market purchases",
            Category::TaxesFees => "Taxes & broker fees",
            Category::Contracts => "Contracts",
            Category::ContractEscrow => "Contract deposits & collateral",
            Category::Industry => "Industry jobs",
            Category::Planetary => "PI import/export taxes",
            Category::Insurance => "Insurance",
            Category::Donations => "Player donations & trades",
            Category::Corporation => "Corporation payments",
            Category::SkillsClones => "Skills & clones",
            Category::LoyaltyStore => "Loyalty store",
            Category::Services => "Fees & services",
            Category::Other => "Other",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Category::Ratting => "gps_fixed",
            Category::Missions => "assignment_turned_in",
            Category::Rewards => "emoji_events",
            Category::SalesMining => "diamond",
            Category::SalesPlanetary => "public",
            Category::SalesShips => "rocket_launch",
            Category::SalesOther => "sell",
            Category::SkillTrading => "vaccines",
            Category::Plex => "toll",
            Category::MarketPurchases => "shopping_cart",
            Category::TaxesFees => "account_balance",
            Category::Contracts => "assignment",
            Category::ContractEscrow => "lock",
            Category::Industry => "precision_manufacturing",
            Category::Planetary => "public",
            Category::Insurance => "health_and_safety",
            Category::Donations => "volunteer_activism",
            Category::Corporation => "corporate_fare",
            Category::SkillsClones => "school",
            Category::LoyaltyStore => "storefront",
            Category::Services => "receipt_long",
            Category::Other => "more_horiz",
        }
    }

    /// Stable name for `ctx.remember`.
    pub fn key(self) -> String {
        format!("{self:?}")
    }

    pub fn from_key(k: &str) -> Option<Category> {
        Category::ALL.into_iter().find(|c| c.key() == k)
    }

    pub fn side(self) -> Side {
        match self {
            Category::Ratting
            | Category::Missions
            | Category::Rewards
            | Category::SalesMining
            | Category::SalesPlanetary
            | Category::SalesShips
            | Category::SalesOther => Side::Income,
            Category::MarketPurchases
            | Category::TaxesFees
            | Category::ContractEscrow
            | Category::Industry
            | Category::Planetary
            | Category::SkillsClones
            | Category::LoyaltyStore
            | Category::Services => Side::Spending,
            Category::SkillTrading
            | Category::Plex
            | Category::Contracts
            | Category::Insurance
            | Category::Donations
            | Category::Corporation
            | Category::Other => Side::Both,
        }
    }

    pub fn activity(self) -> Activity {
        match self {
            Category::Ratting => Activity::Ratting,
            Category::Missions | Category::LoyaltyStore => Activity::Missions,
            Category::Rewards => Activity::Rewards,
            Category::SalesMining => Activity::Mining,
            Category::SalesPlanetary | Category::Planetary => Activity::Planetary,
            Category::SalesShips | Category::SalesOther | Category::MarketPurchases | Category::TaxesFees => Activity::Trading,
            Category::SkillTrading => Activity::SpFarming,
            Category::Plex => Activity::Plex,
            Category::Contracts | Category::ContractEscrow => Activity::Contracts,
            Category::Industry => Activity::Industry,
            Category::Insurance => Activity::Insurance,
            Category::Donations | Category::Corporation => Activity::Transfers,
            Category::SkillsClones | Category::Services | Category::Other => Activity::Upkeep,
        }
    }
}

/// What a character does for ISK: categories grouped so their costs net against their income ("Trading" is market
/// sales less purchases, broker fees and sales tax).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Activity {
    Ratting,
    Missions,
    Rewards,
    Mining,
    Planetary,
    Trading,
    SpFarming,
    Plex,
    Contracts,
    Industry,
    Insurance,
    Transfers,
    Upkeep,
}

impl Activity {
    pub fn label(self) -> &'static str {
        match self {
            Activity::Ratting => "Ratting",
            Activity::Missions => "Missions",
            Activity::Rewards => "Incursions & events",
            Activity::Mining => "Mining",
            Activity::Planetary => "Planetary industry",
            Activity::Trading => "Trading",
            Activity::SpFarming => "SP farming",
            Activity::Plex => "PLEX",
            Activity::Contracts => "Contracts",
            Activity::Industry => "Industry",
            Activity::Insurance => "Insurance",
            Activity::Transfers => "Transfers",
            Activity::Upkeep => "Skills, clones & fees",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Activity::Ratting => "gps_fixed",
            Activity::Missions => "assignment_turned_in",
            Activity::Rewards => "emoji_events",
            Activity::Mining => "diamond",
            Activity::Planetary => "public",
            Activity::Trading => "storefront",
            Activity::SpFarming => "opacity",
            Activity::Plex => "toll",
            Activity::Contracts => "assignment",
            Activity::Industry => "precision_manufacturing",
            Activity::Insurance => "health_and_safety",
            Activity::Transfers => "swap_horiz",
            Activity::Upkeep => "school",
        }
    }

    /// What its net covers, for a tooltip.
    pub fn covers(self) -> String {
        let cats: Vec<&str> = Category::ALL.into_iter().filter(|c| c.activity() == self).map(|c| c.label()).collect();
        cats.join(", ")
    }
}

/// What kind of goods a market line traded, from the item's market group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Goods {
    /// Ores, ice, moon ores, gas, minerals and ice products: what mining makes.
    Mining,
    Planetary,
    Ships,
    SkillTrading,
    Plex,
    Other,
}

// Market group ids from CCP's static data. They have been stable for years; a type outside them counts as other
// goods.
const MINING_GROUPS: [i64; 8] = [54, 1855, 2395, 3784, 1857, 1033, 1032, 501];
const PLANETARY_GROUP: i64 = 1332;
const SHIPS_GROUP: i64 = 4;
const SKILL_TRADING_GROUP: i64 = 2358;
const PLEX_GROUP: i64 = 1923;

pub fn goods_of(type_id: i64) -> Goods {
    let Some(t) = MARKET.get(type_id) else { return Goods::Other };
    let path: Vec<i64> = MARKET.path(t.group).iter().map(|g| g.id).collect();
    if path.contains(&PLEX_GROUP) {
        Goods::Plex
    } else if path.contains(&SKILL_TRADING_GROUP) {
        Goods::SkillTrading
    } else if path.iter().any(|g| MINING_GROUPS.contains(g)) {
        Goods::Mining
    } else if path.contains(&PLANETARY_GROUP) {
        Goods::Planetary
    } else if path.contains(&SHIPS_GROUP) {
        Goods::Ships
    } else {
        Goods::Other
    }
}

/// The category of a journal entry. `goods` is what its market transaction traded (None when the entry isn't a
/// market line or the transaction wasn't found).
pub fn categorize(ref_type: &str, amount: f64, goods: Option<(Goods, bool)>) -> Category {
    match ref_type {
        "market_transaction" => match goods {
            Some((Goods::Plex, _)) => Category::Plex,
            Some((Goods::SkillTrading, _)) => Category::SkillTrading,
            Some((_, true)) => Category::MarketPurchases,
            Some((Goods::Mining, false)) => Category::SalesMining,
            Some((Goods::Planetary, false)) => Category::SalesPlanetary,
            Some((Goods::Ships, false)) => Category::SalesShips,
            Some((Goods::Other, false)) => Category::SalesOther,
            None if amount < 0.0 => Category::MarketPurchases,
            None => Category::SalesOther,
        },
        "market_escrow" => Category::MarketPurchases,
        "brokers_fee" | "transaction_tax" | "contract_brokers_fee" | "contract_brokers_fee_corp" | "contract_sales_tax"
        | "contract_deposit_sales_tax" | "market_provider_tax" | "freelance_jobs_broadcasting_fee" | "freelance_jobs_duration_fee" => {
            Category::TaxesFees
        }
        "bounty_prizes" | "bounty_prize" | "ess_escrow_transfer" => Category::Ratting,
        "agent_mission_reward"
        | "agent_mission_time_bonus_reward"
        | "agent_mission_collateral_paid"
        | "agent_mission_collateral_refunded"
        | "agent_mission_reward_corporation_tax"
        | "agent_mission_time_bonus_reward_corporation_tax"
        | "agent_services_rendered"
        | "agent_donation" => Category::Missions,
        "corporate_reward_payout" | "corporate_reward_tax" | "project_discovery_reward" | "daily_goal_payouts" | "daily_goal_payouts_tax"
        | "milestone_reward_payment" | "daily_challenge_reward" | "resource_wars_reward" | "redeemed_isk_token" | "freelance_jobs_reward"
        | "freelance_jobs_reward_corporation_tax" | "agent_security_reward" => Category::Rewards,
        r if r.starts_with("insurgency_") && r.contains("reward") => Category::Rewards,
        "contract_deposit"
        | "contract_deposit_refund"
        | "contract_collateral"
        | "contract_collateral_refund"
        | "contract_collateral_deposited_corp"
        | "contract_collateral_payout"
        | "contract_reward_deposited"
        | "contract_reward_deposited_corp"
        | "contract_reward_refund"
        | "contract_auction_bid"
        | "contract_auction_bid_refund"
        | "contract_auction_bid_corp"
        | "freelance_jobs_reward_escrow"
        | "freelance_jobs_escrow_refund" => Category::ContractEscrow,
        r if r.starts_with("contract_") => Category::Contracts,
        "industry_job_tax" | "manufacturing" | "copying" | "reverse_engineering" | "reaction" | "reprocessing_tax" | "industry_facility_tax" => {
            Category::Industry
        }
        r if r.starts_with("researching_") => Category::Industry,
        "planetary_import_tax" | "planetary_export_tax" | "planetary_construction" => Category::Planetary,
        "insurance" => Category::Insurance,
        "player_donation" | "player_trading" => Category::Donations,
        "corporation_account_withdrawal" | "corporation_dividend_payment" | "corporation_payment" | "corp_account_withdrawal" | "union_payment" => {
            Category::Corporation
        }
        "skill_purchase" | "jump_clone_activation_fee" | "jump_clone_installation_fee" | "clone_activation" | "clone_transfer" | "clone_upgrade" => {
            Category::SkillsClones
        }
        "lp_store" => Category::LoyaltyStore,
        "docking_fee" | "office_rental_fee" | "structure_gate_jump" | "asset_safety_recovery_tax" | "war_fee" | "war_fee_surrender"
        | "war_ally_contract" | "alliance_maintainance_fee" | "alliance_registration_fee" | "infrastructure_hub_maintenance"
        | "sovereignity_bill" | "cspa" | "kill_right_fee" | "bounty" | "medal_creation" | "medal_issued" | "repair_bill"
        | "factory_slot_rental_fee" | "jump_clone_upkeep" | "upkeep_adjustment_fee" | "contraband_fine" | "copying_fee"
        | "skyhook_claim_fee" | "bounty_surcharge" | "cosmetic_market_component_item_purchase" | "freelance_jobs_reward_corporation_tax_refund" => {
            Category::Services
        }
        _ => Category::Other,
    }
}

/// The (income, spending) an amount adds to a category; spending is positive.
pub fn split(category: Category, amount: f64) -> (f64, f64) {
    match category.side() {
        Side::Income => (amount, 0.0),
        Side::Spending => (0.0, -amount),
        Side::Both if amount >= 0.0 => (amount, 0.0),
        Side::Both => (0.0, -amount),
    }
}

/// The item a market line traded.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub type_id: i64,
    pub name: String,
    pub quantity: i64,
}

/// One journal entry, its character and its category.
#[derive(Clone, Debug)]
pub struct Line {
    pub char_id: i64,
    pub entry: JournalEntry,
    pub category: Category,
    pub item: Option<Item>,
    /// What the line counts for: its amount, or nothing for a buy order filled from escrow (the escrow entry
    /// already counted that ISK).
    pub amount: f64,
}

/// Every journal entry of `chars` since `since` (None: all kept), newest first.
pub fn lines(chars: &[Arc<Character>], since: Option<DateTime<Utc>>) -> Vec<Line> {
    lines_with(chars, since, goods_of)
}

fn lines_with(chars: &[Arc<Character>], since: Option<DateTime<Utc>>, goods: impl Fn(i64) -> Goods) -> Vec<Line> {
    let mut out = Vec::new();
    let mut cache: HashMap<i64, Goods> = HashMap::new();
    for c in chars {
        let Some(journal) = &c.wallet_journal else { continue };
        let by_ref: HashMap<i64, &Transaction> = c
            .wallet_transactions
            .iter()
            .flat_map(|t| t.transactions.iter())
            .filter_map(|t| t.journal_ref_id.map(|r| (r, t)))
            .collect();
        // newest first, so the entry before each one in time is the next one
        for (i, e) in journal.entries.iter().enumerate().filter(|(_, e)| since.is_none_or(|s| e.date >= s)) {
            let tx = (e.ref_type == "market_transaction").then(|| by_ref.get(&e.id)).flatten();
            let traded = tx.map(|t| (*cache.entry(t.type_id).or_insert_with(|| goods(t.type_id)), t.is_buy));
            let older = journal.entries.get(i + 1).and_then(|o| o.balance);
            out.push(Line {
                char_id: c.id,
                category: categorize(&e.ref_type, e.amount, traded),
                item: tx.map(|t| Item { type_id: t.type_id, name: t.name.clone(), quantity: t.quantity }),
                amount: if paid_from_escrow(e, older) { 0.0 } else { e.amount },
                entry: e.clone(),
            });
        }
    }
    out.sort_by(|a, b| b.entry.date.cmp(&a.entry.date).then(b.entry.id.cmp(&a.entry.id)));
    out
}

/// A market purchase that left the balance as it was: a buy order filled with ISK set aside by an earlier
/// `market_escrow` entry, which is where the analysis counts it.
fn paid_from_escrow(e: &JournalEntry, older_balance: Option<f64>) -> bool {
    e.ref_type == "market_transaction" && e.amount < 0.0 && matches!((e.balance, older_balance), (Some(b), Some(o)) if (b - o).abs() < 0.5)
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    pub income: f64,
    /// Positive.
    pub spending: f64,
    pub count: usize,
}

impl Totals {
    pub fn net(&self) -> f64 {
        self.income - self.spending
    }
    fn add(&mut self, category: Category, amount: f64) {
        let (i, s) = split(category, amount);
        self.income += i;
        self.spending += s;
        self.count += 1;
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CategoryTotal {
    pub category: Category,
    pub totals: Totals,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActivityTotal {
    pub activity: Activity,
    pub totals: Totals,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CharacterTotal {
    pub char_id: i64,
    pub totals: Totals,
    /// The activity with the best net, if any made money.
    pub top: Option<(Activity, f64)>,
}

/// Income and spending of one day (or week, or month, when the period is long).
#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    pub start: NaiveDate,
    pub totals: Totals,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketSize {
    Day,
    Week,
    Month,
}

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub totals: Totals,
    /// Every category with entries, biggest flow first.
    pub categories: Vec<CategoryTotal>,
    /// Every activity with entries, best net first.
    pub activities: Vec<ActivityTotal>,
    /// Per character, best net first.
    pub characters: Vec<CharacterTotal>,
    pub buckets: Vec<Bucket>,
    pub bucket_size: Option<BucketSize>,
    /// The oldest entry counted.
    pub first: Option<DateTime<Utc>>,
}

impl Analysis {
    /// Income categories, biggest first.
    pub fn income(&self) -> Vec<(Category, f64)> {
        let mut v: Vec<(Category, f64)> = self.categories.iter().filter(|c| c.totals.income > 0.0).map(|c| (c.category, c.totals.income)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v
    }

    /// Spending categories, biggest first.
    pub fn spending(&self) -> Vec<(Category, f64)> {
        let mut v: Vec<(Category, f64)> =
            self.categories.iter().filter(|c| c.totals.spending > 0.0).map(|c| (c.category, c.totals.spending)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v
    }

    /// Activities that made ISK, best net first.
    pub fn earners(&self) -> Vec<&ActivityTotal> {
        self.activities.iter().filter(|a| a.totals.net() > 0.0).collect()
    }
}

/// Totals of `lines` (from [`lines`]) for the period from `since` (None: from the oldest line) to `now`.
/// `chars` lists the characters to compare, so ones without entries still get a row.
pub fn analyze(lines: &[Line], chars: &[i64], since: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Analysis {
    let mut totals = Totals::default();
    let mut categories: HashMap<Category, Totals> = HashMap::new();
    let mut activities: HashMap<Activity, Totals> = HashMap::new();
    let mut per_char: HashMap<i64, (Totals, HashMap<Activity, Totals>)> = chars.iter().map(|id| (*id, Default::default())).collect();
    for l in lines {
        let a = l.amount;
        totals.add(l.category, a);
        categories.entry(l.category).or_default().add(l.category, a);
        activities.entry(l.category.activity()).or_default().add(l.category, a);
        let (t, acts) = per_char.entry(l.char_id).or_default();
        t.add(l.category, a);
        acts.entry(l.category.activity()).or_default().add(l.category, a);
    }

    let mut categories: Vec<CategoryTotal> = categories.into_iter().map(|(category, totals)| CategoryTotal { category, totals }).collect();
    categories.sort_by(|a, b| (b.totals.income + b.totals.spending).total_cmp(&(a.totals.income + a.totals.spending)).then(a.category.cmp(&b.category)));
    let mut activities: Vec<ActivityTotal> = activities.into_iter().map(|(activity, totals)| ActivityTotal { activity, totals }).collect();
    activities.sort_by(|a, b| b.totals.net().total_cmp(&a.totals.net()).then(a.activity.cmp(&b.activity)));
    let mut characters: Vec<CharacterTotal> = per_char
        .into_iter()
        .map(|(char_id, (totals, acts))| {
            let top = acts
                .into_iter()
                .map(|(a, t)| (a, t.net()))
                .filter(|(_, n)| *n > 0.0)
                .max_by(|a, b| a.1.total_cmp(&b.1).then(b.0.cmp(&a.0)));
            CharacterTotal { char_id, totals, top }
        })
        .collect();
    characters.sort_by(|a, b| b.totals.net().total_cmp(&a.totals.net()).then(a.char_id.cmp(&b.char_id)));

    let first = lines.iter().map(|l| l.entry.date).min();
    let start = since.or(first);
    let (buckets, bucket_size) = match start {
        Some(start) if start <= now => {
            let size = bucket_size(start, now);
            (buckets(lines, start, now, size), Some(size))
        }
        _ => (Vec::new(), None),
    };
    Analysis { totals, categories, activities, characters, buckets, bucket_size, first }
}

/// Days for up to three months, then weeks for up to a year, then months: about 100 bars at most.
pub(crate) fn bucket_size(start: DateTime<Utc>, now: DateTime<Utc>) -> BucketSize {
    let days = (now - start).num_days();
    if days <= 92 {
        BucketSize::Day
    } else if days <= 366 {
        BucketSize::Week
    } else {
        BucketSize::Month
    }
}

pub(crate) fn bucket_start(d: NaiveDate, size: BucketSize) -> NaiveDate {
    match size {
        BucketSize::Day => d,
        BucketSize::Week => d - Duration::days(d.weekday().num_days_from_monday() as i64),
        BucketSize::Month => d.with_day(1).unwrap_or(d),
    }
}

pub(crate) fn next_bucket(d: NaiveDate, size: BucketSize) -> NaiveDate {
    match size {
        BucketSize::Day => d + Duration::days(1),
        BucketSize::Week => d + Duration::days(7),
        BucketSize::Month => d.checked_add_months(chrono::Months::new(1)).unwrap_or(d + Duration::days(31)),
    }
}

/// Every bucket from `start` to `now` in EVE time (UTC), empty ones included so the chart's time axis is even.
fn buckets(lines: &[Line], start: DateTime<Utc>, now: DateTime<Utc>, size: BucketSize) -> Vec<Bucket> {
    let mut out = Vec::new();
    let mut index: HashMap<NaiveDate, usize> = HashMap::new();
    let mut d = bucket_start(start.date_naive(), size);
    let last = now.date_naive();
    while d <= last {
        index.insert(d, out.len());
        out.push(Bucket { start: d, totals: Totals::default() });
        d = next_bucket(d, size);
    }
    for l in lines {
        if let Some(&i) = index.get(&bucket_start(l.entry.date.date_naive(), size)) {
            out[i].totals.add(l.category, l.amount);
        }
    }
    out
}

/// Characters whose journal can't load for lack of the wallet scope.
pub fn missing_scope(chars: &[Arc<Character>]) -> Vec<Arc<Character>> {
    chars.iter().filter(|c| c.refreshes.get("wallet_journal").and_then(|r| r.error.as_deref()) == Some("scope")).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::wallet::{JournalData, TransactionsData};
    use chrono::TimeZone;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, day, hour, 0, 0).unwrap()
    }

    fn entry(id: i64, day: u32, ref_type: &str, amount: f64) -> JournalEntry {
        JournalEntry { id, date: at(day, 12), ref_type: ref_type.into(), amount, ..Default::default() }
    }

    fn tx(journal: i64, type_id: i64, is_buy: bool) -> Transaction {
        Transaction { transaction_id: journal * 10, journal_ref_id: Some(journal), type_id, is_buy, quantity: 1, name: format!("#{type_id}"), ..Default::default() }
    }

    fn character(id: i64, entries: Vec<JournalEntry>, transactions: Vec<Transaction>) -> Arc<Character> {
        let mut c = Character::new(id);
        c.wallet_journal = Some(JournalData { entries });
        c.wallet_transactions = Some(TransactionsData { transactions });
        Arc::new(c)
    }

    /// Test goods by type id, so the tests don't depend on the static data.
    fn goods(type_id: i64) -> Goods {
        match type_id {
            34 => Goods::Mining,
            2393 => Goods::Planetary,
            587 => Goods::Ships,
            44992 => Goods::Plex,
            40520 => Goods::SkillTrading,
            _ => Goods::Other,
        }
    }

    #[test]
    fn ref_types() {
        assert_eq!(categorize("bounty_prizes", 1.0, None), Category::Ratting);
        assert_eq!(categorize("ess_escrow_transfer", 1.0, None), Category::Ratting);
        assert_eq!(categorize("agent_mission_time_bonus_reward", 1.0, None), Category::Missions);
        assert_eq!(categorize("insurgency_suppression_contribution_reward", 1.0, None), Category::Rewards);
        assert_eq!(categorize("brokers_fee", -1.0, None), Category::TaxesFees);
        assert_eq!(categorize("contract_sales_tax", -1.0, None), Category::TaxesFees);
        assert_eq!(categorize("contract_collateral", -1.0, None), Category::ContractEscrow);
        assert_eq!(categorize("contract_reward", 1.0, None), Category::Contracts);
        assert_eq!(categorize("contract_price", -1.0, None), Category::Contracts);
        assert_eq!(categorize("researching_time_productivity", -1.0, None), Category::Industry);
        assert_eq!(categorize("planetary_export_tax", -1.0, None), Category::Planetary);
        assert_eq!(categorize("jump_clone_activation_fee", -1.0, None), Category::SkillsClones);
        assert_eq!(categorize("something_new_from_ccp", 1.0, None), Category::Other);
        // market lines by what was traded, or by sign when the transaction is unknown
        assert_eq!(categorize("market_transaction", 1.0, Some((Goods::Mining, false))), Category::SalesMining);
        assert_eq!(categorize("market_transaction", -1.0, Some((Goods::Mining, true))), Category::MarketPurchases);
        assert_eq!(categorize("market_transaction", -1.0, Some((Goods::Plex, true))), Category::Plex);
        assert_eq!(categorize("market_transaction", 1.0, Some((Goods::SkillTrading, false))), Category::SkillTrading);
        assert_eq!(categorize("market_transaction", 1.0, None), Category::SalesOther);
        assert_eq!(categorize("market_transaction", -1.0, None), Category::MarketPurchases);
        for c in Category::ALL {
            assert_eq!(Category::from_key(&c.key()), Some(c));
        }
    }

    #[test]
    fn sides_net_refunds() {
        // an escrow returned lowers spending instead of counting as income
        assert_eq!(split(Category::MarketPurchases, 50.0), (0.0, -50.0));
        assert_eq!(split(Category::MarketPurchases, -80.0), (0.0, 80.0));
        assert_eq!(split(Category::Contracts, 10.0), (10.0, 0.0));
        assert_eq!(split(Category::Contracts, -10.0), (0.0, 10.0));
        assert_eq!(split(Category::Missions, -5.0), (-5.0, 0.0));
    }

    #[test]
    fn market_goods_from_static_data() {
        assert_eq!(goods_of(34), Goods::Mining, "Tritanium");
        assert_eq!(goods_of(1230), Goods::Mining, "Veldspar");
        assert_eq!(goods_of(16634), Goods::Mining, "Atmospheric Gases");
        assert_eq!(goods_of(2393), Goods::Planetary, "Bacteria");
        assert_eq!(goods_of(587), Goods::Ships, "Rifter");
        assert_eq!(goods_of(44992), Goods::Plex);
        assert_eq!(goods_of(40520), Goods::SkillTrading, "Large Skill Injector");
        assert_eq!(goods_of(2048), Goods::Other, "Damage Control II");
        assert_eq!(goods_of(-1), Goods::Other);
    }

    fn sample() -> Vec<Arc<Character>> {
        vec![
            character(
                1,
                vec![
                    entry(1, 20, "bounty_prizes", 100.0),
                    entry(2, 20, "ess_escrow_transfer", 50.0),
                    entry(3, 21, "market_escrow", -300.0),
                    entry(4, 22, "market_escrow", 100.0),
                    entry(5, 22, "market_transaction", 400.0),
                    entry(6, 22, "transaction_tax", -20.0),
                    entry(7, 1, "bounty_prizes", 999.0),
                ],
                vec![tx(5, 587, false)],
            ),
            character(
                2,
                vec![
                    entry(10, 21, "market_transaction", 200.0),
                    entry(11, 21, "market_transaction", -60.0),
                    entry(12, 22, "player_donation", -40.0),
                    entry(13, 22, "planetary_export_tax", -10.0),
                ],
                vec![tx(10, 34, false), tx(11, 44992, true)],
            ),
            character(3, vec![], vec![]),
        ]
    }

    #[test]
    fn lines_join_transactions_and_filter_the_period() {
        let chars = sample();
        let since = Some(at(15, 0));
        let l = lines_with(&chars, since, goods);
        assert_eq!(l.len(), 10, "the entry on the 1st is before the period");
        assert!(l.windows(2).all(|w| w[0].entry.date >= w[1].entry.date), "newest first");
        let sale = l.iter().find(|l| l.entry.id == 5).unwrap();
        assert_eq!(sale.category, Category::SalesShips);
        assert_eq!(sale.item.as_ref().map(|i| i.type_id), Some(587));
        assert_eq!(l.iter().find(|l| l.entry.id == 10).unwrap().category, Category::SalesMining);
        assert_eq!(l.iter().find(|l| l.entry.id == 11).unwrap().category, Category::Plex);
        assert_eq!(lines_with(&chars, None, goods).len(), 11);
    }

    #[test]
    fn totals_by_category_activity_and_character() {
        let chars = sample();
        let since = Some(at(15, 0));
        let l = lines_with(&chars, since, goods);
        let a = analyze(&l, &[1, 2, 3], since, at(22, 18));
        // income: 100 + 50 + 400 + 200; spending: 200 (escrow net) + 20 + 60 + 40 + 10
        assert_eq!(a.totals.income, 750.0);
        assert_eq!(a.totals.spending, 330.0);
        assert_eq!(a.totals.net(), l.iter().map(|l| l.amount).sum::<f64>(), "net is the sum of all amounts");
        assert_eq!(a.income()[0], (Category::SalesShips, 400.0));
        assert!(a.income().contains(&(Category::Ratting, 150.0)));
        assert_eq!(a.spending()[0], (Category::MarketPurchases, 200.0));

        // trading nets sales against purchases and fees: 400 - 200 - 20
        let trading = a.activities.iter().find(|x| x.activity == Activity::Trading).unwrap();
        assert_eq!(trading.totals.net(), 180.0);
        assert_eq!(a.earners()[0].activity, Activity::Mining, "mining's 200 beats trading's 180");
        assert!(a.earners().iter().all(|x| x.totals.net() > 0.0));
        assert!(a.activities.iter().any(|x| x.activity == Activity::Transfers && x.totals.net() == -40.0));

        assert_eq!(a.characters.len(), 3, "characters without entries get a row");
        let c1 = a.characters.iter().find(|c| c.char_id == 1).unwrap();
        assert_eq!(c1.totals.net(), 330.0);
        assert_eq!(c1.top, Some((Activity::Trading, 180.0)));
        let c3 = a.characters.iter().find(|c| c.char_id == 3).unwrap();
        assert_eq!((c3.totals, c3.top), (Totals::default(), None));
        assert_eq!(a.characters[0].char_id, 1, "best net first");
    }

    #[test]
    fn buckets_cover_the_whole_period() {
        let chars = sample();
        let since = Some(at(15, 0));
        let l = lines_with(&chars, since, goods);
        let a = analyze(&l, &[1, 2], since, at(22, 18));
        assert_eq!(a.bucket_size, Some(BucketSize::Day));
        assert_eq!(a.buckets.len(), 8, "the 15th to the 22nd, empty days included");
        assert_eq!(a.buckets[0].totals, Totals::default());
        let d20 = a.buckets.iter().find(|b| b.start.day() == 20).unwrap();
        assert_eq!((d20.totals.income, d20.totals.count), (150.0, 2));
        assert_eq!(a.buckets.iter().map(|b| b.totals.net()).sum::<f64>(), a.totals.net());

        // "all" starts at the oldest entry; a long span uses weeks
        let all = lines_with(&chars, None, goods);
        let a = analyze(&all, &[1, 2], None, at(22, 18));
        assert_eq!(a.first, Some(at(1, 12)));
        assert_eq!(a.buckets.first().map(|b| b.start.day()), Some(1));
        let start = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        assert_eq!(bucket_size(start, at(22, 0)), BucketSize::Week);
        assert_eq!(bucket_size(start - Duration::days(400), at(22, 0)), BucketSize::Month);
        let b = buckets(&all, start - Duration::days(400), at(22, 0), BucketSize::Month);
        assert!(b.iter().all(|b| b.start.day() == 1));
        assert_eq!(bucket_start(NaiveDate::from_ymd_opt(2026, 11, 22).unwrap(), BucketSize::Week), NaiveDate::from_ymd_opt(2026, 11, 16).unwrap());
    }

    #[test]
    fn buy_orders_filled_from_escrow_count_once() {
        let with_balance = |mut e: JournalEntry, b: f64| {
            e.balance = Some(b);
            e
        };
        let c = character(
            1,
            vec![
                // newest first: a sell, a buy filled from escrow (balance unchanged), a direct buy, the escrow
                with_balance(entry(4, 22, "market_transaction", 50.0), 870.0),
                with_balance(entry(3, 22, "market_transaction", -100.0), 820.0),
                with_balance(entry(2, 21, "market_transaction", -80.0), 820.0),
                with_balance(entry(1, 20, "market_escrow", -100.0), 900.0),
            ],
            vec![tx(3, 34, true), tx(2, 34, true), tx(4, 34, false)],
        );
        let l = lines_with(&[c], None, goods);
        let amount = |id| l.iter().find(|l| l.entry.id == id).unwrap().amount;
        assert_eq!((amount(4), amount(3), amount(2), amount(1)), (50.0, 0.0, -80.0, -100.0));
        let a = analyze(&l, &[1], None, at(22, 18));
        assert_eq!(a.totals.spending, 180.0);
        assert_eq!(a.totals.net(), 870.0 - 1000.0, "net is the balance change");
    }

    #[test]
    fn periods() {
        for p in Period::ALL {
            assert_eq!(Period::from_key(&p.key()), Some(p));
        }
        assert_eq!(Period::from_key("0"), None);
        assert_eq!(Period::Days(7).since(at(22, 0)), Some(at(15, 0)));
        assert_eq!(Period::All.since(at(22, 0)), None);
    }
}
