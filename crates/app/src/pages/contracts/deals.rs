//! The Deals tab: public item exchanges and auctions in a region compared with the market.
//!
//! A region lists thousands of public contracts and their items need a request each, so the work goes in steps:
//! the listing's own filters (kind, price) narrow it first; items are loaded for that shortlist (newest first, up
//! to a limit, with a progress bar; items already cached cost nothing); the item filters (name or market category,
//! single item) apply then; the remaining contracts' items are priced at the contracts price hub; issuers and
//! locations are resolved last. What was loaded is kept for the session, so changing a filter or coming back to the
//! tab doesn't start over.

use super::common::{self, open_contract};
use super::Tab;
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::{labelled, parse_amount};
use crate::pages::market_orders::{set_child, type_cell};
use crate::rt;
use crate::widgets::{self as w, Btn, Status, Table};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use gtk::prelude::*;
use podstack_core::domains::contracts::{Bid, Contract, ContractItem};
use podstack_core::features::contracts as ct;
use podstack_core::features::deals::{self as dl, Kind, Prefilter, Value};
use podstack_core::fmt;
use podstack_core::market::BestPrices;
use podstack_core::public_contracts::{PublicBid, PublicContract, PublicItem};
use podstack_core::sde::{MAP, MARKET};
use podstack_core::universe::Place;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const CSS: &str = "
.dl-filters { padding: 4px 0; }
.dl-progress { padding: 8px 0; }
";

const REGION_KEY: &str = "contracts.deals.region";
const KIND_KEY: &str = "contracts.deals.kind";
const MIN_KEY: &str = "contracts.deals.min";
const MAX_KEY: &str = "contracts.deals.max";
const SEARCH_KEY: &str = "contracts.deals.search";
const SINGLE_KEY: &str = "contracts.deals.single";
const LIMIT_KEY: &str = "contracts.deals.limit";

const THE_FORGE: i64 = podstack_core::market::THE_FORGE;
/// How many contracts' items one search loads at most (each is a request; cached ones don't count).
const LIMITS: [usize; 5] = [250, 500, 1000, 2500, 5000];
/// Types priced per step (the progress bar moves between steps).
const PRICE_CHUNK: usize = 48;

/// What's been loaded this session.
#[derive(Default)]
struct Loaded {
    region: i64,
    listing: Arc<Vec<PublicContract>>,
    fetched: Option<DateTime<Utc>>,
    items: HashMap<i64, Arc<Vec<PublicItem>>>,
    /// Hub station the prices are from.
    station: i64,
    prices: HashMap<i64, BestPrices>,
    /// Types asked for (some have no orders, so `prices` alone can't say).
    priced: HashSet<i64>,
    names: HashMap<i64, String>,
    places: HashMap<i64, Place>,
    /// Bids of auctions looked at, highest first.
    bids: HashMap<i64, Vec<PublicBid>>,
}

thread_local! {
    static STATE: RefCell<Option<Rc<RefCell<Loaded>>>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

fn state() -> Option<Rc<RefCell<Loaded>>> {
    STATE.with(|s| s.borrow().clone())
}

fn regions() -> Vec<(i64, String)> {
    MAP.market_regions().cloned().collect()
}

struct Filters {
    pre: Prefilter,
    search: String,
    single: bool,
    limit: usize,
}

fn filters(ctx: &Ctx) -> Filters {
    let kind = ctx.recall(KIND_KEY).and_then(|v| v.parse::<usize>().ok()).and_then(|i| Kind::ALL.get(i).copied()).unwrap_or_default();
    let amount = |k: &str| ctx.recall(k).and_then(|v| parse_amount(&v)).filter(|v| *v >= 0.0);
    Filters {
        pre: Prefilter { kind, min_price: amount(MIN_KEY), max_price: amount(MAX_KEY) },
        search: ctx.recall(SEARCH_KEY).unwrap_or_default(),
        single: ctx.recall(SINGLE_KEY).as_deref() == Some("yes"),
        limit: ctx.recall(LIMIT_KEY).and_then(|v| v.parse::<usize>().ok()).and_then(|i| LIMITS.get(i).copied()).unwrap_or(LIMITS[1]),
    }
}

fn region_of(ctx: &Ctx) -> i64 {
    ctx.recall(REGION_KEY).and_then(|v| v.parse().ok()).unwrap_or(THE_FORGE)
}

/// The page's widgets that loading and filtering update.
struct Ui {
    ctx: Ctx,
    status: gtk::Box,
    results: gtk::Box,
    find: gtk::Button,
}

impl Ui {
    fn alive(&self) -> bool {
        self.results.root().is_some()
    }
    fn set_busy(&self, busy: bool) {
        BUSY.with(|b| b.set(busy));
        self.find.set_sensitive(!busy);
    }
    fn status(&self, child: Option<&gtk::Widget>) {
        while let Some(c) = self.status.first_child() {
            self.status.remove(&c);
        }
        if let Some(c) = child {
            self.status.append(c);
        }
    }
    /// A progress bar with a line of text; returns both to update.
    fn progress(&self, text: &str, fraction: Option<f64>) -> (gtk::ProgressBar, gtk::Label) {
        let b = w::vbox(6);
        b.add_css_class("dl-progress");
        let l = w::line(text, &["body-medium", "dim"]);
        let bar = w::progress(fraction.unwrap_or(0.0));
        if fraction.is_none() {
            bar.pulse();
            let weak = bar.downgrade();
            // pulses while shown; replacing the status removes it, which stops this
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(120), move || match weak.upgrade() {
                Some(b) if b.root().is_some() => {
                    b.pulse();
                    gtk::glib::ControlFlow::Continue
                }
                _ => gtk::glib::ControlFlow::Break,
            });
        }
        b.append(&l);
        b.append(&bar);
        self.status(Some(b.upcast_ref()));
        (bar, l)
    }
    fn error(&self, text: &str) {
        self.status(Some(w::banner("error", text, w::BannerKind::Error).upcast_ref()));
    }
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&super::tab_bar(ctx, Tab::Deals));

    let f = filters(ctx);
    let regs = regions();
    let names: Vec<&str> = regs.iter().map(|r| r.1.as_str()).collect();
    let region_dd = w::dropdown(&names, regs.iter().position(|r| r.0 == region_of(ctx)).unwrap_or(0) as u32);
    region_dd.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<gtk::Expression>, "string")));
    region_dd.set_enable_search(true);
    let kinds: Vec<&str> = Kind::ALL.iter().map(|k| k.label()).collect();
    let kind_dd = w::dropdown(&kinds, Kind::ALL.iter().position(|k| *k == f.pre.kind).unwrap_or(0) as u32);
    let min = gtk::Entry::new();
    min.set_placeholder_text(Some("Any"));
    min.set_width_chars(9);
    min.set_text(&ctx.recall(MIN_KEY).unwrap_or_default());
    let max = gtk::Entry::new();
    max.set_placeholder_text(Some("Any"));
    max.set_width_chars(9);
    max.set_text(&ctx.recall(MAX_KEY).unwrap_or_default());
    let search = w::search_entry("Item or category, e.g. Ships");
    search.set_hexpand(false);
    search.set_width_chars(24);
    search.set_text(&f.search);
    let single = gtk::Switch::new();
    single.set_active(f.single);
    single.set_valign(gtk::Align::Center);
    single.set_halign(gtk::Align::Start);
    single.set_tooltip_text(Some("One item and nothing asked in return: a ship, a module, a single blueprint"));
    let limit_labels: Vec<String> = LIMITS.iter().map(|l| fmt::int(*l as i64)).collect();
    let limit_refs: Vec<&str> = limit_labels.iter().map(|s| s.as_str()).collect();
    let limit_dd = w::dropdown(&limit_refs, LIMITS.iter().position(|l| *l == f.limit).unwrap_or(1) as u32);
    limit_dd.set_tooltip_text(Some("Most contracts to load items for per search (newest first). Items already loaded don't count."));
    let find = w::button_icon("search", "Find deals", Btn::Filled);
    let refresh = w::icon_button("refresh", "Reload the region's contracts");

    let controls = gtk::FlowBox::new();
    controls.add_css_class("dl-filters");
    controls.set_selection_mode(gtk::SelectionMode::None);
    controls.set_column_spacing(12);
    controls.set_row_spacing(12);
    controls.set_max_children_per_line(10);
    let actions = w::hbox(4);
    actions.append(&find);
    actions.append(&refresh);
    for c in [
        labelled("Region", &region_dd).upcast::<gtk::Widget>(),
        labelled("Kind", &kind_dd).upcast(),
        labelled("Min price (ISK)", &min).upcast(),
        labelled("Max price (ISK)", &max).upcast(),
        labelled("Search", &search).upcast(),
        labelled("Single item only", &single).upcast(),
        labelled("Load up to", &limit_dd).upcast(),
        actions.upcast(),
    ] {
        c.set_valign(gtk::Align::End);
        controls.insert(&c, -1);
    }
    body.append(&controls);

    let status = w::vbox(8);
    body.append(&status);
    let results = w::vbox(12);
    results.set_vexpand(true);
    body.append(&results);
    let ui = Rc::new(Ui { ctx: ctx.clone(), status, results, find: find.clone() });
    if BUSY.with(|b| b.get()) {
        // a search started before the page was rebuilt: wait for it, then show what it found
        find.set_sensitive(false);
        ui.status(Some(w::line("Still loading from the last search…", &["body-medium", "dim"]).upcast_ref()));
        let ui = ui.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
            if !ui.alive() {
                return gtk::glib::ControlFlow::Break;
            }
            if BUSY.with(|b| b.get()) {
                return gtk::glib::ControlFlow::Continue;
            }
            ui.find.set_sensitive(true);
            ui.status(None);
            render(&ui);
            gtk::glib::ControlFlow::Break
        });
    }

    // filters: remember, then show the loaded contracts through them again
    {
        let (ctx, ui) = (ctx.clone(), ui.clone());
        region_dd.connect_selected_notify(move |dd| {
            if let Some(r) = regions().get(dd.selected() as usize) {
                ctx.remember(REGION_KEY, r.0.to_string());
            }
            render(&ui);
        });
    }
    {
        let (ctx, ui) = (ctx.clone(), ui.clone());
        kind_dd.connect_selected_notify(move |dd| {
            ctx.remember(KIND_KEY, dd.selected().to_string());
            render(&ui);
        });
    }
    for (e, key) in [(&min, MIN_KEY), (&max, MAX_KEY)] {
        let (ctx, ui) = (ctx.clone(), ui.clone());
        e.connect_changed(move |e| {
            ctx.remember(key, e.text().as_str());
            render(&ui);
        });
    }
    {
        let (ctx, ui) = (ctx.clone(), ui.clone());
        search.connect_search_changed(move |s| {
            ctx.remember(SEARCH_KEY, s.text().as_str());
            render(&ui);
        });
    }
    {
        let (ctx, ui) = (ctx.clone(), ui.clone());
        single.connect_active_notify(move |s| {
            ctx.remember(SINGLE_KEY, if s.is_active() { "yes" } else { "no" });
            render(&ui);
        });
    }
    {
        let ctx = ctx.clone();
        limit_dd.connect_selected_notify(move |dd| ctx.remember(LIMIT_KEY, dd.selected().to_string()));
    }
    {
        let ui = ui.clone();
        find.connect_clicked(move |_| start(&ui, false));
    }
    {
        let ui = ui.clone();
        refresh.connect_clicked(move |_| start(&ui, true));
    }
    {
        let ui = ui.clone();
        search.connect_activate(move |_| start(&ui, false));
    }
    render(&ui);
    Page::new("Contracts", body).static_page().no_scroll()
}

// ---------- loading ----------

/// The contracts the listing's filters let through, newest first.
fn candidates(st: &Loaded, f: &Filters) -> Vec<PublicContract> {
    let now = Utc::now();
    let mut v: Vec<PublicContract> = st.listing.iter().filter(|c| f.pre.admits(c, now)).cloned().collect();
    v.sort_by_key(|c| std::cmp::Reverse(c.date_issued));
    v
}

/// Whether the loaded items let the contract through the item filters.
fn passes(items: &[PublicItem], f: &Filters) -> bool {
    items.iter().any(|i| i.is_included) && dl::matches(items, &f.search) && (!f.single || dl::is_single(items))
}

/// Find deals: the region's listing, then items for the shortlist, then prices and names.
fn start(ui: &Rc<Ui>, force: bool) {
    if BUSY.with(|b| b.get()) {
        return;
    }
    let ctx = ui.ctx.clone();
    let region = region_of(&ctx);
    let region_name = MAP.region_name(region).unwrap_or("the region").to_string();
    ui.set_busy(true);
    ui.progress(&format!("Loading public contracts in {region_name}…"), None);
    let core = ctx.core().clone();
    let ui = ui.clone();
    rt::run(async move { core.public_contracts.region(&core.esi, region, force).await }, move |r| match r {
        Err(e) => {
            ui.set_busy(false);
            ui.error(&format!("Couldn't load the public contracts in {region_name}: {e}"));
        }
        Ok(list) => {
            let st = match state().filter(|s| s.borrow().region == region) {
                Some(s) => s,
                None => {
                    let s = Rc::new(RefCell::new(Loaded { region, ..Default::default() }));
                    STATE.with(|x| *x.borrow_mut() = Some(s.clone()));
                    s
                }
            };
            {
                let mut s = st.borrow_mut();
                s.listing = list;
                // the listing may come from the last half hour's copy
                let age = ui.ctx.core().public_contracts.fetched(region).map(|at| at.elapsed()).unwrap_or_default();
                s.fetched = Some(Utc::now() - chrono::Duration::from_std(age).unwrap_or_default());
            }
            load_items(ui, st);
        }
    });
}

fn load_items(ui: Rc<Ui>, st: Rc<RefCell<Loaded>>) {
    let ctx = ui.ctx.clone();
    let f = filters(&ctx);
    let core = ctx.core().clone();
    let todo: Vec<i64> = {
        let mut s = st.borrow_mut();
        let cands = candidates(&s, &f);
        // items cached from earlier sessions come free
        for c in &cands {
            if let std::collections::hash_map::Entry::Vacant(e) = s.items.entry(c.contract_id) {
                if let Some(i) = core.public_contracts.cached_items(c.contract_id) {
                    e.insert(i);
                }
            }
        }
        cands.iter().filter(|c| !s.items.contains_key(&c.contract_id)).take(f.limit).map(|c| c.contract_id).collect()
    };
    if todo.is_empty() {
        return price(ui, st);
    }
    let total = todo.len();
    let (bar, label) = ui.progress(&format!("Loading the items of {} contracts…", fmt::int(total as i64)), Some(0.0));
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<(usize, usize)>();
    gtk::glib::spawn_future_local(async move {
        while let Some((done, total)) = rx.next().await {
            bar.set_fraction(done as f64 / total.max(1) as f64);
            label.set_text(&format!("Loading the items of {} contracts… {} done", fmt::int(total as i64), fmt::int(done as i64)));
        }
    });
    rt::run(
        async move {
            core.public_contracts
                .items(&core.esi, &todo, move |done, total| {
                    let _ = tx.unbounded_send((done, total));
                })
                .await
        },
        move |items| {
            {
                let mut s = st.borrow_mut();
                s.items.extend(items);
            }
            price(ui, st);
        },
    );
}

/// Contracts through every filter, with their items.
fn matching(st: &Loaded, f: &Filters) -> Vec<(PublicContract, Arc<Vec<PublicItem>>)> {
    candidates(st, f)
        .into_iter()
        .filter_map(|c| {
            let items = st.items.get(&c.contract_id)?.clone();
            passes(&items, f).then_some((c, items))
        })
        .collect()
}

/// Prices the matching contracts' types not priced yet, a chunk at a time.
fn price(ui: Rc<Ui>, st: Rc<RefCell<Loaded>>) {
    let ctx = ui.ctx.clone();
    let f = filters(&ctx);
    let s = common::settings();
    let (station, region) = ct::hub(&s);
    let hub = common::hub_name(&s).to_string();
    let types: Vec<i64> = {
        let mut st = st.borrow_mut();
        // a different hub since the last search: its prices start over
        if st.station != station {
            st.station = station;
            st.prices.clear();
            st.priced.clear();
        }
        let mut t: Vec<i64> = matching(&st, &f)
            .iter()
            .flat_map(|(_, items)| items.iter().filter(|i| !i.is_blueprint_copy).map(|i| i.type_id).collect::<Vec<_>>())
            .filter(|t| !st.priced.contains(t) && MARKET.get(*t).is_some())
            .collect();
        t.sort_unstable();
        t.dedup();
        t
    };
    if types.is_empty() {
        return resolve(ui, st);
    }
    let total = types.len();
    let (bar, label) = ui.progress(&format!("Pricing {} items at {hub}…", fmt::int(total as i64)), Some(0.0));
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<usize>();
    {
        let hub = hub.clone();
        gtk::glib::spawn_future_local(async move {
            while let Some(done) = rx.next().await {
                bar.set_fraction(done as f64 / total.max(1) as f64);
                label.set_text(&format!("Pricing {} items at {hub}… {} done", fmt::int(total as i64), fmt::int(done as i64)));
            }
        });
    }
    let core = ctx.core().clone();
    rt::run(
        async move {
            let mut out = HashMap::new();
            let mut done = 0;
            for chunk in types.chunks(PRICE_CHUNK) {
                out.extend(core.market.best_prices(&core.esi, region, Some(station), chunk, false).await);
                done += chunk.len();
                let _ = tx.unbounded_send(done);
            }
            (types, out)
        },
        move |(types, prices)| {
            {
                let mut s = st.borrow_mut();
                // a type whose orders failed to load stays unpriced, to be asked again next time
                s.priced.extend(types.iter().filter(|t| prices.contains_key(t)));
                s.prices.extend(prices);
            }
            resolve(ui, st);
        },
    );
}

/// Names of issuers and locations of the matching contracts.
fn resolve(ui: Rc<Ui>, st: Rc<RefCell<Loaded>>) {
    let ctx = ui.ctx.clone();
    let f = filters(&ctx);
    let (names, places): (Vec<i64>, Vec<i64>) = {
        let s = st.borrow();
        let m = matching(&s, &f);
        let mut n: Vec<i64> = m.iter().flat_map(|(c, _)| [c.issuer_id, c.issuer_corporation_id]).filter(|id| !s.names.contains_key(id)).collect();
        n.sort_unstable();
        n.dedup();
        let mut p: Vec<i64> = m.iter().filter_map(|(c, _)| c.start_location_id).filter(|id| !s.places.contains_key(id)).collect();
        p.sort_unstable();
        p.dedup();
        (n, p)
    };
    if names.is_empty() && places.is_empty() {
        ui.set_busy(false);
        ui.status(None);
        return render(&ui);
    }
    ui.progress("Looking up issuers and locations…", None);
    let core = ctx.core().clone();
    // structures can only be named with a character's login that may dock there; the first character's is tried
    let who = core.characters().first().map(|c| c.id);
    rt::run(
        async move {
            let n = core.universe.names(&core.esi, names).await;
            let auth = match who {
                Some(id) => core.auth.auth(&core.esi.http, id, 60).await.ok(),
                None => None,
            };
            let p = core.universe.places(&core.esi, places, auth.as_ref()).await;
            (n, p)
        },
        move |(n, p)| {
            {
                let mut s = st.borrow_mut();
                s.names.extend(n);
                s.places.extend(p);
            }
            ui.set_busy(false);
            ui.status(None);
            render(&ui);
        },
    );
}

// ---------- results ----------

#[derive(Clone)]
struct Deal {
    c: PublicContract,
    items: Arc<Vec<PublicItem>>,
    v: Value,
    /// What it costs now: the price, or an auction's top bid when loaded.
    price: f64,
    bid: Option<f64>,
    bids_loaded: bool,
    /// Some types weren't priced yet.
    pending: bool,
    headline: String,
    first: i64,
    place: Option<Place>,
    issuer: String,
}

impl Deal {
    fn vs_sell(&self) -> Option<f64> {
        if self.pending {
            None
        } else {
            dl::discount(self.price, self.v.net_sell())
        }
    }
    fn vs_buy(&self) -> Option<f64> {
        if self.pending {
            None
        } else {
            dl::discount(self.price, self.v.net_buy())
        }
    }
}

fn type_name(t: i64, names: &HashMap<i64, String>) -> String {
    MARKET.get(t).map(|t| t.name.clone()).or_else(|| names.get(&t).cloned()).unwrap_or_else(|| format!("Type #{t}"))
}

fn deals(st: &Loaded, f: &Filters) -> Vec<Deal> {
    matching(st, f)
        .into_iter()
        .map(|(c, items)| {
            let v = dl::value(&items, &st.prices);
            let pending = items.iter().any(|i| !i.is_blueprint_copy && MARKET.get(i.type_id).is_some() && !st.priced.contains(&i.type_id));
            let bids = st.bids.get(&c.contract_id);
            let bid = bids.and_then(|b| b.first().map(|b| b.amount));
            let issuer_id = if c.for_corporation { c.issuer_corporation_id } else { c.issuer_id };
            Deal {
                price: dl::price_now(&c, bid),
                bid,
                bids_loaded: bids.is_some(),
                pending,
                headline: dl::headline(&items, |t| type_name(t, &st.names)),
                first: items.iter().find(|i| i.is_included).map(|i| i.type_id).unwrap_or(0),
                place: c.start_location_id.and_then(|l| st.places.get(&l).cloned()),
                issuer: st.names.get(&issuer_id).cloned().unwrap_or_default(),
                c,
                items,
                v,
            }
        })
        .collect()
}

/// Shows the loaded contracts through the current filters.
fn render(ui: &Rc<Ui>) {
    let ctx = &ui.ctx;
    let region = region_of(ctx);
    let region_name = MAP.region_name(region).unwrap_or("this region").to_string();
    let Some(st) = state().filter(|s| s.borrow().region == region) else {
        set_child(
            &ui.results,
            &w::empty_state(
                "local_offer",
                "Find deals on contracts",
                &format!(
                    "Look through the public item exchanges and auctions in {region_name} for items under their market price. Narrow it by price first: each contract's items take a request to load, so a tighter price range gets there faster."
                ),
            ),
        );
        return;
    };
    let f = filters(ctx);
    let s = st.borrow();
    let cands = candidates(&s, &f);
    let with_items = cands.iter().filter(|c| s.items.contains_key(&c.contract_id)).count();
    let missing = cands.len() - with_items;
    let rows = deals(&s, &f);
    let pending = rows.iter().filter(|d| d.pending).count();
    drop(s);

    let v = w::vbox(12);
    v.set_vexpand(true);
    let settings = common::settings();
    let hub = common::hub_name(&settings);
    let fetched = st.borrow().fetched.map(|t| format!(" · listing from {}", fmt::relative(t))).unwrap_or_default();
    let summary = w::line(
        &format!(
            "{} deals · items loaded for {} of {} contracts matching kind and price · valued at {hub} (Contracts settings){fetched}",
            fmt::int(rows.len() as i64),
            fmt::int(with_items as i64),
            fmt::int(cands.len() as i64)
        ),
        &["body-small", "dim"],
    );
    summary.set_wrap(true);
    v.append(&summary);
    let busy = BUSY.with(|b| b.get());
    if missing > 0 && !busy {
        let b = w::banner(
            "downloading",
            &format!(
                "{} more contracts match the kind and price but their items aren't loaded. Narrow the price range, or load the next {}.",
                fmt::int(missing as i64),
                fmt::int(missing.min(f.limit) as i64)
            ),
            w::BannerKind::Info,
        );
        let more = w::button("Load items", Btn::Text);
        let ui2 = ui.clone();
        more.connect_clicked(move |_| start(&ui2, false));
        b.append(&more);
        v.append(&b);
    }
    if pending > 0 && !busy {
        let b = w::banner("sell", &format!("{} contracts aren't priced yet.", fmt::int(pending as i64)), w::BannerKind::Notice);
        let go = w::button("Price them", Btn::Text);
        let (ui2, st2) = (ui.clone(), st.clone());
        go.connect_clicked(move |_| {
            if !BUSY.with(|b| b.get()) {
                ui2.set_busy(true);
                price(ui2.clone(), st2.clone());
            }
        });
        b.append(&go);
        v.append(&b);
    }
    v.append(&table(ui, &st, rows));
    set_child(&ui.results, &v);
}

fn deal_status(d: &Deal) -> (String, Status, Option<String>) {
    if d.pending {
        return ("Not priced".into(), Status::Neutral, None);
    }
    let mut notes = Vec::new();
    if d.v.copies > 0 {
        notes.push(format!("{} blueprint cop{} (not valued)", d.v.copies, if d.v.copies == 1 { "y" } else { "ies" }));
    }
    if !d.v.unpriced.is_empty() {
        let names: Vec<String> = d.v.unpriced.iter().take(5).map(|t| MARKET.get(*t).map(|t| t.name.clone()).unwrap_or_else(|| format!("Type #{t}"))).collect();
        notes.push(format!("No hub price: {}", names.join(", ")));
    }
    if d.v.asks_items {
        notes.push("Asks for items in return (their value is taken off)".into());
    }
    let tip = (!notes.is_empty()).then(|| notes.join("\n"));
    if d.v.lines == 0 {
        return ("Blueprint copies".into(), Status::Neutral, tip);
    }
    if !d.v.complete() {
        return ("Partly valued".into(), Status::Warn, tip);
    }
    if d.price < d.v.net_buy() {
        ("Under buy orders".into(), Status::Good, tip)
    } else if d.price < d.v.net_sell() {
        ("Under sell orders".into(), Status::Info, tip)
    } else {
        ("Over market".into(), Status::Bad, tip)
    }
}

fn discount_cell(x: Option<f64>) -> gtk::Widget {
    let (t, class) = match x {
        Some(d) if d > 0.0 => (format!("{:.1}% under", d * 100.0), "success-text"),
        Some(d) if d < 0.0 => (format!("{:.1}% over", -d * 100.0), "error-text"),
        Some(_) => ("At market".into(), "dim"),
        None => ("—".into(), "dim"),
    };
    let l = w::line(&t, &["numeric", class]);
    l.set_xalign(1.0);
    l.upcast()
}

fn place_text(p: &Option<Place>, id: Option<i64>) -> (String, Option<String>) {
    match p {
        Some(p) if p.kind == "structure" && p.system_id.is_none() => ("Structure (no access)".into(), Some(p.name.clone())),
        Some(p) => {
            let tip = [p.system_name.clone(), p.security.map(fmt::security), p.region_name.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
            (p.name.clone(), Some(tip).filter(|t| !t.is_empty()))
        }
        None => (if id.is_some() { "Unknown".into() } else { String::new() }, None),
    }
}

/// The contract as the shared contract dialog shows it.
fn as_contract(d: &Deal, st: &Loaded) -> Contract {
    let c = &d.c;
    Contract {
        contract_id: c.contract_id,
        kind: c.kind.clone(),
        status: "outstanding".into(),
        title: c.title.clone(),
        for_corporation: c.for_corporation,
        availability: "public".into(),
        issuer_id: c.issuer_id,
        issuer_corporation_id: c.issuer_corporation_id,
        issuer_name: st.names.get(&c.issuer_id).cloned().unwrap_or_else(|| format!("#{}", c.issuer_id)),
        issuer_corporation_name: st.names.get(&c.issuer_corporation_id).cloned().unwrap_or_else(|| format!("Corporation #{}", c.issuer_corporation_id)),
        date_issued: c.date_issued,
        date_expired: c.date_expired,
        days_to_complete: c.days_to_complete,
        price: c.price,
        reward: c.reward,
        collateral: c.collateral,
        buyout: c.buyout,
        volume: c.volume,
        start_location: d.place.clone(),
        items: d
            .items
            .iter()
            .map(|i| ContractItem {
                record_id: i.record_id,
                type_id: i.type_id,
                name: type_name(i.type_id, &st.names),
                quantity: i.quantity,
                is_included: i.is_included,
                is_singleton: false,
                raw_quantity: i.is_blueprint_copy.then_some(-2),
            })
            .collect(),
        items_loaded: true,
        bids: st.bids.get(&c.contract_id).map(|b| b.iter().map(|b| Bid { bid_id: b.bid_id, bidder_id: 0, amount: b.amount, date_bid: b.date_bid }).collect()).unwrap_or_default(),
        ..Default::default()
    }
}

/// Loads an auction's bids, then shows the results again with its current price.
fn load_bids(ui: &Rc<Ui>, st: &Rc<RefCell<Loaded>>, id: i64) {
    let core = ui.ctx.core().clone();
    let (ui, st) = (ui.clone(), st.clone());
    rt::run(async move { core.public_contracts.bids(&core.esi, id).await }, move |r| match r {
        Ok(b) => {
            let msg = match b.first() {
                Some(top) => format!("Top bid {} ({} bids)", fmt::isk(top.amount), b.len()),
                None => "No bids yet".into(),
            };
            st.borrow_mut().bids.insert(id, b);
            ui.ctx.toast(&msg);
            render(&ui);
        }
        Err(e) => ui.ctx.toast(&format!("Couldn't load the bids: {e}")),
    });
}

fn table(ui: &Rc<Ui>, st: &Rc<RefCell<Loaded>>, mut rows: Vec<Deal>) -> gtk::Widget {
    let key = |x: Option<f64>| x.unwrap_or(-1e18);
    // already in the default order so the table's first sort doesn't move rows
    rows.sort_by(|a, b| key(b.vs_sell()).total_cmp(&key(a.vs_sell())));
    let now = Utc::now();
    let (ui2, st2) = (ui.clone(), st.clone());
    let (ui3, st3) = (ui.clone(), st.clone());
    let price_text = |d: &Deal| {
        if d.c.kind == "auction" {
            match d.bid {
                Some(b) => format!("{} bid", fmt::isk_short(b)),
                None if d.bids_loaded => format!("{} (no bids)", fmt::isk_short(d.price)),
                None => format!("{} opening", fmt::isk_short(d.price)),
            }
        } else {
            fmt::isk_short(d.price)
        }
    };
    Table::new(rows)
        .widget_sorted(
            "Deal",
            |d: &Deal| {
                let (t, s, tip) = deal_status(d);
                let l = w::status(&t, s);
                l.set_tooltip_text(tip.as_deref());
                l.upcast()
            },
            move |a, b| key(a.vs_sell()).total_cmp(&key(b.vs_sell())),
        )
        .widget_sorted(
            "Items",
            |d| {
                let row = w::hbox(8);
                let cell = type_cell(d.first, &d.headline);
                cell.set_hexpand(true);
                row.append(&cell);
                if d.v.copies > 0 {
                    row.append(&w::status(if d.v.copies == 1 { "Copy" } else { "Copies" }, Status::Neutral));
                }
                if d.v.asks_items {
                    row.append(&w::status("Wants items", Status::Warn));
                }
                if !d.c.title.trim().is_empty() {
                    row.set_tooltip_text(Some(d.c.title.trim()));
                }
                row.upcast()
            },
            |a, b| a.headline.to_lowercase().cmp(&b.headline.to_lowercase()),
        )
        .expand()
        .width(280)
        .text("Kind", |d| if d.c.kind == "auction" { "Auction".into() } else { "Exchange".into() })
        .num("Price", price_text, |d| d.price)
        .num("Buyout", |d| d.c.buyout.filter(|b| *b > 0.0).map(fmt::isk_short).unwrap_or_default(), |d| d.c.buyout.unwrap_or(0.0))
        .num("Sell value", |d| if d.pending { "—".into() } else { fmt::isk_short(d.v.net_sell()) }, |d| d.v.net_sell())
        .num("Buy value", |d| if d.pending { "—".into() } else { fmt::isk_short(d.v.net_buy()) }, |d| d.v.net_buy())
        .widget_sorted("vs sell", |d| discount_cell(d.vs_sell()), move |a, b| key(a.vs_sell()).total_cmp(&key(b.vs_sell())))
        .widget_sorted("vs buy", |d| discount_cell(d.vs_buy()), move |a, b| key(a.vs_buy()).total_cmp(&key(b.vs_buy())))
        .widget_sorted(
            "Expires",
            move |d| {
                let l = w::line(&fmt::duration(d.c.date_expired - now, 2), &["dim"]);
                l.set_tooltip_text(Some(&fmt::datetime(d.c.date_expired)));
                l.upcast()
            },
            |a, b| a.c.date_expired.cmp(&b.c.date_expired),
        )
        .widget_sorted(
            "Location",
            |d| {
                let (t, tip) = place_text(&d.place, d.c.start_location_id);
                let l = w::line(&t, &[]);
                l.set_tooltip_text(tip.as_deref());
                l.upcast()
            },
            |a, b| place_text(&a.place, None).0.cmp(&place_text(&b.place, None).0),
        )
        .width(200)
        .text("Issuer", |d| d.issuer.clone())
        .widget("", move |d| {
            let row = w::hbox(0);
            if d.c.kind == "auction" {
                let b = w::icon_button("gavel", if d.bids_loaded { "Reload bids" } else { "Load the current bid" });
                let (ui, st, id) = (ui2.clone(), st2.clone(), d.c.contract_id);
                b.connect_clicked(move |_| load_bids(&ui, &st, id));
                row.append(&b);
            }
            let b = w::icon_button("chevron_right", "Contract details");
            let (ctx, st, d) = (ui2.ctx.clone(), st2.clone(), d.clone());
            b.connect_clicked(move |_| open_contract(&ctx, &as_contract(&d, &st.borrow())));
            row.append(&b);
            row.upcast()
        })
        .on_activate(move |d| open_contract(&ui3.ctx, &as_contract(d, &st3.borrow())))
        .sort_by(7, true)
        .empty_text("No contracts match. Loosen the filters, or load more items.")
        .build()
}
