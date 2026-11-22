//! Market → Browser: any item's order book across regions, its price history, and EVE Tycoon-style
//! statistics.
//!
//! The left pane finds items (a search, or the market's group tree); the right pane shows the selected item: its
//! order book in one region, the five trade hubs or every region, statistics over the orders that pass the filters,
//! the sellers and buyers, and the region's daily price history. The page updates itself (it is static), and keeps
//! what's selected in `market.browser.*` memory keys so coming back to the tab shows the same thing.

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Btn, Status, Table};
use chrono::{DateTime, Utc};
use gtk::{gio, glib, prelude::*};
use podstack_core::features::comms;
use podstack_core::features::trade::{self, BookFilter, BookStats, SideStats};
use podstack_core::market::{BookOrder, HistoryDay, GLOBAL_PLEX_MARKET, PLEX_TYPE_ID, THE_FORGE};
use podstack_core::sde::{SecurityClass, MAP, MARKET, TRADE_HUBS};
use podstack_core::{config, fmt, universe, Core};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Duration as StdDuration;

const CSS: &str = "
.mkt-browser-nav { background-color: @surface_container_low; border-radius: 16px; padding: 8px; }
.mkt-browser-nav listview, .mkt-browser-nav list, .mkt-browser-nav scrolledwindow { background-color: transparent; }
.mkt-browser-tree > row { border-radius: 8px; padding: 0 4px; min-height: 30px; }
.mkt-browser-tree > row:hover, .mkt-browser-results > row:hover { background-color: alpha(@on_surface, 0.08); }
.mkt-browser-results > row { border-radius: 8px; padding: 6px 8px; }
.mkt-browser-group { color: @on_surface; }
.mkt-browser-current { color: @primary; font-weight: 600; }
.mkt-browser-header image, .mkt-browser-header picture { border-radius: 8px; }
.mkt-browser-sec {
  border-radius: 6px; padding: 1px 6px; font-size: 12px; font-weight: 600; font-feature-settings: \"tnum\";
  min-width: 26px;
}
.mkt-browser-sec.high { background-color: alpha(@success, 0.16); color: @success; }
.mkt-browser-sec.low { background-color: alpha(@warning, 0.18); color: @warning; }
.mkt-browser-sec.null { background-color: alpha(@error, 0.16); color: @error; }
.mkt-browser-tiles .stat-tile { min-width: 160px; }
.mkt-browser-up .stat-value { color: @success; }
.mkt-browser-down .stat-value { color: @error; }
.mkt-browser-chart-tip {
  background-color: @surface_container_highest; color: @on_surface; border-radius: 12px; padding: 8px 12px;
  box-shadow: 0 2px 6px alpha(@shadow, 0.3);
}
.mkt-browser-dot { min-width: 10px; min-height: 10px; border-radius: 999px; }
.mkt-browser-dot.avg { background-color: @primary; }
.mkt-browser-dot.band { background-color: alpha(@primary, 0.25); }
.mkt-browser-dot.volume { background-color: alpha(@secondary, 0.55); }
.mkt-browser-filters entry { min-width: 110px; }
";

const KEY_TYPE: &str = "market.browser.type";
const KEY_MODE: &str = "market.browser.mode";
const KEY_REGION: &str = "market.browser.region";
const KEY_FILTERS: &str = "market.browser.filters";
const KEY_FILTERS_OPEN: &str = "market.browser.filters_open";
const KEY_RANGE: &str = "market.browser.range";
const KEY_SIDE: &str = "market.browser.side";
const KEY_SEARCH: &str = "market.browser.search";

const STRUCTURE_SCOPE: &str = "esi-universe.read_structures.v1";
/// Structure names cost one request each (and need docking access), so a huge book only names this many.
const MAX_STRUCTURE_LOOKUPS: usize = 150;
/// Regions fetched per step when loading several; each step advances the progress bar.
const REGION_STEP: usize = 8;
const SEARCH_LIMIT: usize = 100;
/// Price history ranges: (label, days).
const RANGES: [(&str, i64); 4] = [("1M", 30), ("3M", 90), ("6M", 180), ("1Y", 365)];

/// Shows an item in the market browser, from anywhere in the app.
#[allow(dead_code)] // for links into the market browser from other pages
pub fn show_item(ctx: &Ctx, type_id: i64) {
    ctx.remember(KEY_TYPE, type_id.to_string());
    super::open(ctx, Tab::Browser);
}

// ---------------------------------------------------------------------------------------------------------------
// State

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Region,
    Hubs,
    All,
}

impl Mode {
    const ALL: [Mode; 3] = [Mode::Region, Mode::Hubs, Mode::All];
    fn label(self) -> &'static str {
        match self {
            Mode::Region => "Region",
            Mode::Hubs => "Trade hubs",
            Mode::All => "All regions",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Mode::Region => "region",
            Mode::Hubs => "hubs",
            Mode::All => "all",
        }
    }
}

/// The filter panel's settings. Price and quantity bounds are kept as typed so they survive a rebuild unchanged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Filters {
    #[serde(default)]
    book: BookFilter,
    #[serde(default)]
    min_price: String,
    #[serde(default)]
    max_price: String,
    #[serde(default)]
    min_qty: String,
    #[serde(default)]
    max_qty: String,
    #[serde(default)]
    mine_only: bool,
}

/// `Filters` with the bounds parsed, for checking many orders.
struct Applied<'a> {
    book: &'a BookFilter,
    price: (Option<f64>, Option<f64>),
    qty: (Option<f64>, Option<f64>),
    mine_only: bool,
}

impl Filters {
    fn applied(&self) -> Applied<'_> {
        Applied {
            book: &self.book,
            price: (parse_amount(&self.min_price), parse_amount(&self.max_price)),
            qty: (parse_amount(&self.min_qty), parse_amount(&self.max_qty)),
            mine_only: self.mine_only,
        }
    }

    /// How many filters are on, for the panel's header.
    fn active(&self) -> usize {
        let b = &self.book;
        [b.exclude_high, b.exclude_low, b.exclude_null, b.exclude_npc, b.exclude_structures, b.hubs_only, self.mine_only]
            .into_iter()
            .filter(|on| *on)
            .count()
            + [&self.min_price, &self.max_price, &self.min_qty, &self.max_qty].into_iter().filter(|s| parse_amount(s).is_some()).count()
    }
}

impl Applied<'_> {
    fn keep(&self, o: &BookOrder, own: &HashSet<i64>) -> bool {
        let within = |v: f64, (lo, hi): (Option<f64>, Option<f64>)| lo.is_none_or(|l| v >= l) && hi.is_none_or(|h| v <= h);
        self.book.keep(o)
            && (!self.mine_only || own.contains(&o.order_id))
            && within(o.price, self.price)
            && within(o.volume_remain as f64, self.qty)
    }
}

/// A number as typed: "1,250", "1.5m", "20k", "2b".
fn parse_amount(s: &str) -> Option<f64> {
    let s: String = s.trim().to_lowercase().chars().filter(|c| !matches!(c, ',' | ' ' | '_')).collect();
    if s.is_empty() {
        return None;
    }
    let (num, mult) = match s.chars().last() {
        Some('k') => (&s[..s.len() - 1], 1e3),
        Some('m') => (&s[..s.len() - 1], 1e6),
        Some('b') => (&s[..s.len() - 1], 1e9),
        Some('t') => (&s[..s.len() - 1], 1e12),
        _ => (s.as_str(), 1.0),
    };
    num.parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v * mult)
}

/// A loaded order book, and what it was loaded for.
struct Book {
    type_id: i64,
    mode: Mode,
    region: i64,
    orders: Rc<Vec<BookOrder>>,
    /// Regions that failed to load, of how many.
    failed: usize,
    regions: usize,
    at: DateTime<Utc>,
}

enum History {
    Loading,
    Failed(String),
    Loaded(Rc<Vec<HistoryDay>>),
}

struct State {
    type_id: Option<i64>,
    mode: Mode,
    region: i64,
    filters: Filters,
    range: usize,
    book: Option<Book>,
    loading: bool,
    book_error: Option<String>,
    history: Option<(i64, i64, History)>,
    /// Station and structure names.
    names: HashMap<i64, String>,
}

/// The page's widgets that change, and its state. Signal handlers hold it weakly; the page's root keeps it alive
/// (see `build`), so it goes away with the page.
struct Ui {
    ctx: Ctx,
    st: RefCell<State>,
    /// Bumped by every order book load, so a slow load can't overwrite a newer one.
    book_gen: Cell<u64>,
    history_gen: Cell<u64>,
    filter_gen: Cell<u64>,
    detail: gtk::Stack,
    header: gtk::Box,
    status: gtk::Box,
    region_dd: gtk::DropDown,
    filter_count: gtk::Label,
    stats: gtk::Box,
    orders: gtk::Box,
    history: gtk::Box,
    tree: RefCell<Option<Tree>>,
}

/// Type rows currently bound in the tree, to mark the selected one.
type BoundRows = Rc<RefCell<Vec<(i64, glib::WeakRef<gtk::Box>)>>>;

struct Tree {
    view: gtk::ListView,
    model: gtk::TreeListModel,
    bound: BoundRows,
}

fn regions() -> Vec<(i64, String)> {
    MAP.market_regions().cloned().collect()
}

fn hub_regions() -> Vec<i64> {
    TRADE_HUBS.iter().map(|h| h.3).collect()
}

/// Replaces a container's children.
fn fill(holder: &gtk::Box, child: &impl IsA<gtk::Widget>) {
    while let Some(c) = holder.first_child() {
        holder.remove(&c);
    }
    holder.append(child);
}

/// ISK prices: full below a million, compact above.
fn price(v: f64) -> String {
    if v.abs() >= 1e6 {
        fmt::isk_short(v)
    } else {
        fmt::decimal(v, 2)
    }
}

fn opt_price(v: Option<f64>) -> String {
    v.map(price).unwrap_or_else(|| "–".into())
}

// ---------------------------------------------------------------------------------------------------------------
// Page

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&tab_bar(ctx, Tab::Browser));

    let all_regions = regions();
    let type_id = ctx
        .recall(KEY_TYPE)
        .or_else(|| std::env::var("PODSTACK_MARKET_TYPE").ok())
        .and_then(|s| s.trim().parse::<i64>().ok())
        .filter(|id| MARKET.get(*id).is_some());
    let mode = ctx.recall(KEY_MODE).and_then(|k| Mode::ALL.into_iter().find(|m| m.key() == k)).unwrap_or(Mode::Region);
    let region = ctx
        .recall(KEY_REGION)
        .and_then(|s| s.parse::<i64>().ok())
        .filter(|r| all_regions.iter().any(|x| x.0 == *r))
        .unwrap_or(THE_FORGE);
    let filters: Filters = ctx.recall(KEY_FILTERS).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let range = ctx.recall(KEY_RANGE).and_then(|s| s.parse().ok()).filter(|r| *r < RANGES.len()).unwrap_or(1);

    let names: Vec<&str> = all_regions.iter().map(|r| r.1.as_str()).collect();
    let region_dd = w::dropdown(&names, all_regions.iter().position(|r| r.0 == region).unwrap_or(0) as u32);
    region_dd.set_enable_search(true);
    region_dd.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<&gtk::Expression>, "string")));
    region_dd.set_sensitive(mode == Mode::Region);

    let ui = Rc::new(Ui {
        ctx: ctx.clone(),
        st: RefCell::new(State {
            type_id,
            mode,
            region,
            filters,
            range,
            book: None,
            loading: false,
            book_error: None,
            history: None,
            names: HashMap::new(),
        }),
        book_gen: Cell::new(0),
        history_gen: Cell::new(0),
        filter_gen: Cell::new(0),
        detail: gtk::Stack::new(),
        header: w::hbox(16),
        status: w::hbox(8),
        region_dd,
        filter_count: w::status("", Status::Info),
        stats: w::vbox(8),
        orders: w::vbox(0),
        history: w::vbox(0),
        tree: RefCell::new(None),
    });

    let split = w::hbox(16);
    split.set_vexpand(true);
    split.append(&left_pane(&ui));
    split.append(&right_pane(&ui));
    body.append(&split);

    // The root keeps the page state alive: GObject drops a widget's signal handlers when it is disposed, and
    // everything else holds the state weakly.
    {
        let keep = ui.clone();
        body.connect_destroy(move |_| {
            let _ = &keep;
        });
    }

    if let Some(id) = type_id {
        ui.show_type(id);
        let wk = Rc::downgrade(&ui);
        // once the tree is laid out, so it can scroll to the row
        glib::idle_add_local_once(move || {
            if let Some(ui) = wk.upgrade() {
                ui.reveal_in_tree(id);
            }
        });
    } else {
        ui.detail.set_visible_child_name("empty");
    }
    Page::new("Market", body).no_scroll().static_page()
}

// ---------------------------------------------------------------------------------------------------------------
// Left pane: search and the group tree

#[derive(Clone, Copy, Debug)]
enum Node {
    Group(i64),
    Type(i64),
}

fn node_of(obj: &glib::Object) -> Option<Node> {
    obj.downcast_ref::<glib::BoxedAnyObject>().map(|b| *b.borrow::<Node>())
}

fn node_store(groups: &[i64], types: &[i64]) -> gio::ListStore {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let items: Vec<glib::BoxedAnyObject> = groups
        .iter()
        .map(|g| glib::BoxedAnyObject::new(Node::Group(*g)))
        .chain(types.iter().map(|t| glib::BoxedAnyObject::new(Node::Type(*t))))
        .collect();
    store.extend_from_slice(&items);
    store
}

fn left_pane(ui: &Rc<Ui>) -> gtk::Box {
    let pane = w::vbox(8);
    pane.add_css_class("mkt-browser-nav");
    pane.set_size_request(300, -1);
    pane.set_hexpand(false);

    let search = w::search_entry("Search items");
    search.set_hexpand(false);
    pane.append(&search);

    let stack = gtk::Stack::new();
    stack.set_vexpand(true);
    stack.add_named(&w::scrolled(&group_tree(ui)), Some("tree"));

    let results = gtk::ListBox::new();
    results.add_css_class("mkt-browser-results");
    results.set_selection_mode(gtk::SelectionMode::None);
    results.set_activate_on_single_click(true);
    let result_ids: Rc<RefCell<Vec<i64>>> = Rc::default();
    {
        let (wk, ids) = (Rc::downgrade(ui), result_ids.clone());
        results.connect_row_activated(move |_, row| {
            let Some(ui) = wk.upgrade() else { return };
            if let Some(id) = usize::try_from(row.index()).ok().and_then(|i| ids.borrow().get(i).copied()) {
                ui.show_type(id);
                ui.reveal_in_tree(id);
            }
        });
    }
    stack.add_named(&w::scrolled(&results), Some("search"));
    pane.append(&stack);

    let show_results = {
        let (stack, results, ids) = (stack.clone(), results.clone(), result_ids.clone());
        move |q: &str| {
            if q.trim().is_empty() {
                stack.set_visible_child_name("tree");
                return;
            }
            results.remove_all();
            let hits = MARKET.search(q, SEARCH_LIMIT);
            *ids.borrow_mut() = hits.iter().map(|t| t.id).collect();
            for t in &hits {
                let row = w::hbox(10);
                row.append(&w::eve_image(&config::type_icon(t.id, 32), 28));
                let texts = w::vbox(0);
                texts.set_valign(gtk::Align::Center);
                texts.append(&w::line(&t.name, &["body-medium"]));
                if let Some(g) = MARKET.groups.get(&t.group) {
                    texts.append(&w::line(&g.name, &["body-small", "dim"]));
                }
                row.append(&texts);
                results.append(&row);
            }
            if hits.is_empty() {
                let l = w::text("No items match.", &["body-medium", "dim"]);
                l.set_margin_top(12);
                l.set_margin_start(8);
                let r = gtk::ListBoxRow::new();
                r.set_child(Some(&l));
                r.set_activatable(false);
                results.append(&r);
            }
            stack.set_visible_child_name("search");
        }
    };
    let initial = ui.ctx.recall(KEY_SEARCH).unwrap_or_default();
    search.set_text(&initial);
    show_results(&initial);
    {
        let ctx = ui.ctx.clone();
        search.connect_search_changed(move |s| {
            ctx.remember(KEY_SEARCH, s.text().as_str());
            show_results(s.text().as_str());
        });
    }
    pane
}

fn group_tree(ui: &Rc<Ui>) -> gtk::ListView {
    let model = gtk::TreeListModel::new(node_store(&MARKET.roots, &[]), false, false, |obj| match node_of(obj)? {
        Node::Group(g) => {
            let g = MARKET.groups.get(&g)?;
            Some(node_store(&g.children, &g.types).upcast())
        }
        Node::Type(_) => None,
    });
    let bound: BoundRows = Rc::default();

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let ex = gtk::TreeExpander::new();
        ex.set_indent_for_icon(true);
        item.set_child(Some(&ex));
    });
    {
        let (wk, bound) = (Rc::downgrade(ui), bound.clone());
        factory.connect_bind(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(ex) = item.child().and_downcast::<gtk::TreeExpander>() else { return };
            let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else { return };
            let Some(node) = row.item().as_ref().and_then(node_of) else { return };
            let b = w::hbox(8);
            b.set_hexpand(true);
            b.set_margin_top(3);
            b.set_margin_bottom(3);
            match node {
                Node::Group(g) => {
                    let name = MARKET.groups.get(&g).map(|g| g.name.as_str()).unwrap_or("?");
                    let l = w::line(name, &["body-medium", "mkt-browser-group"]);
                    l.set_hexpand(true);
                    l.set_tooltip_text(Some(name));
                    b.append(&l);
                }
                Node::Type(t) => {
                    b.append(&w::eve_image(&config::type_icon(t, 32), 20));
                    let name = MARKET.get(t).map(|t| t.name.as_str()).unwrap_or("?");
                    let l = w::line(name, &["body-medium"]);
                    l.set_hexpand(true);
                    l.set_tooltip_text(Some(name));
                    b.append(&l);
                    let current = wk.upgrade().and_then(|ui| ui.st.borrow().type_id);
                    if current == Some(t) {
                        b.add_css_class("mkt-browser-current");
                    }
                    let mut list = bound.borrow_mut();
                    list.retain(|(_, r)| r.upgrade().is_some_and(|b| b.parent().is_some()));
                    list.push((t, b.downgrade()));
                }
            }
            ex.set_child(Some(&b));
            ex.set_list_row(Some(&row));
        });
    }
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        if let Some(ex) = item.child().and_downcast::<gtk::TreeExpander>() {
            ex.set_list_row(None);
            ex.set_child(None::<&gtk::Widget>);
        }
    });

    let view = gtk::ListView::new(Some(gtk::NoSelection::new(Some(model.clone()))), Some(factory));
    view.add_css_class("mkt-browser-tree");
    view.set_single_click_activate(true);
    {
        let wk = Rc::downgrade(ui);
        view.connect_activate(move |view, pos| {
            let Some(row) = view.model().and_then(|m| m.item(pos)).and_downcast::<gtk::TreeListRow>() else { return };
            match row.item().as_ref().and_then(node_of) {
                Some(Node::Group(_)) => row.set_expanded(!row.is_expanded()),
                Some(Node::Type(t)) => {
                    if let Some(ui) = wk.upgrade() {
                        ui.show_type(t);
                    }
                }
                None => {}
            }
        });
    }
    *ui.tree.borrow_mut() = Some(Tree { view: view.clone(), model, bound });
    view
}

// ---------------------------------------------------------------------------------------------------------------
// Right pane: the selected item

fn right_pane(ui: &Rc<Ui>) -> gtk::Stack {
    let stack = ui.detail.clone();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.add_named(&w::empty_state("storefront", "Pick an item", "Search for an item, or browse the market groups on the left."), Some("empty"));

    let col = w::vbox(16);
    col.set_margin_end(8);
    col.set_margin_bottom(8);
    ui.header.add_css_class("mkt-browser-header");
    col.append(&ui.header);
    col.append(&scope_bar(ui));
    col.append(&filter_panel(ui));
    col.append(&ui.stats);
    col.append(&ui.orders);
    col.append(&ui.history);
    // scrolls sideways rather than forcing the window wider when the window is narrow
    let sw = w::scrolled(&col);
    sw.set_hscrollbar_policy(gtk::PolicyType::Automatic);
    stack.add_named(&sw, Some("item"));
    stack
}

fn scope_bar(ui: &Rc<Ui>) -> gtk::Box {
    let bar = w::hbox(12);
    let mode = ui.st.borrow().mode;
    let labels: Vec<&str> = Mode::ALL.iter().map(|m| m.label()).collect();
    {
        let wk = Rc::downgrade(ui);
        bar.append(&w::segmented(&labels, Mode::ALL.iter().position(|m| *m == mode).unwrap_or(0), move |i| {
            let Some(ui) = wk.upgrade() else { return };
            let mode = Mode::ALL[i];
            ui.ctx.remember(KEY_MODE, mode.key());
            ui.st.borrow_mut().mode = mode;
            ui.region_dd.set_sensitive(mode == Mode::Region);
            ui.load_book(false);
            ui.load_history();
        }));
    }
    {
        let wk = Rc::downgrade(ui);
        let all = regions();
        ui.region_dd.connect_selected_notify(move |dd| {
            let Some(ui) = wk.upgrade() else { return };
            let Some((id, _)) = all.get(dd.selected() as usize) else { return };
            ui.ctx.remember(KEY_REGION, id.to_string());
            ui.st.borrow_mut().region = *id;
            ui.load_book(false);
            ui.load_history();
        });
    }
    bar.append(&ui.region_dd);
    ui.status.set_valign(gtk::Align::Center);
    ui.status.set_hexpand(true);
    ui.status.set_halign(gtk::Align::End);
    bar.append(&ui.status);
    bar
}

/// A chip that toggles one filter.
fn filter_chip(ui: &Rc<Ui>, label: &str, tooltip: &str, get: fn(&Filters) -> bool, set: fn(&mut Filters, bool)) -> gtk::ToggleButton {
    let chip = w::chip(label, get(&ui.st.borrow().filters));
    chip.set_tooltip_text(Some(tooltip));
    let wk = Rc::downgrade(ui);
    chip.connect_toggled(move |c| {
        if let Some(ui) = wk.upgrade() {
            set(&mut ui.st.borrow_mut().filters, c.is_active());
            ui.filters_changed();
        }
    });
    chip
}

/// A price or quantity bound; applied shortly after typing stops.
fn filter_entry(ui: &Rc<Ui>, placeholder: &str, get: fn(&Filters) -> &String, set: fn(&mut Filters, String)) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_placeholder_text(Some(placeholder));
    e.set_width_chars(12);
    e.set_text(get(&ui.st.borrow().filters));
    e.set_tooltip_text(Some("Accepts 1,250 or shorthands like 20k, 1.5m, 2b"));
    let wk = Rc::downgrade(ui);
    e.connect_changed(move |e| {
        let Some(ui) = wk.upgrade() else { return };
        set(&mut ui.st.borrow_mut().filters, e.text().to_string());
        let gen = ui.filter_gen.get() + 1;
        ui.filter_gen.set(gen);
        let wk = Rc::downgrade(&ui);
        glib::timeout_add_local_once(StdDuration::from_millis(350), move || {
            if let Some(ui) = wk.upgrade() {
                if ui.filter_gen.get() == gen {
                    ui.filters_changed();
                }
            }
        });
    });
    e
}

fn filter_panel(ui: &Rc<Ui>) -> gtk::Box {
    let open = ui.ctx.recall(KEY_FILTERS_OPEN).as_deref() == Some("1");
    let panel = w::vbox(8);
    panel.add_css_class("card");
    panel.add_css_class("mkt-browser-filters");

    let head = w::hbox(8);
    let toggle = w::button_icon(if open { "expand_less" } else { "filter_list" }, "Filters", Btn::Text);
    head.append(&toggle);
    ui.filter_count.set_valign(gtk::Align::Center);
    head.append(&ui.filter_count);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    head.append(&spacer);
    let reset = w::button("Reset", Btn::Text);
    head.append(&reset);
    panel.append(&head);

    let revealer = gtk::Revealer::new();
    revealer.set_reveal_child(open);
    let content = w::vbox(12);
    content.set_margin_top(4);

    let min_price = filter_entry(ui, "Min price", |f| &f.min_price, |f, v| f.min_price = v);
    let max_price = filter_entry(ui, "Max price", |f| &f.max_price, |f, v| f.max_price = v);
    let min_qty = filter_entry(ui, "Min quantity", |f| &f.min_qty, |f, v| f.min_qty = v);
    let max_qty = filter_entry(ui, "Max quantity", |f| &f.max_qty, |f, v| f.max_qty = v);
    let bounds = gtk::FlowBox::new();
    bounds.set_selection_mode(gtk::SelectionMode::None);
    bounds.set_column_spacing(24);
    bounds.set_row_spacing(8);
    bounds.set_max_children_per_line(2);
    for (label, lo, hi) in [("Price", &min_price, &max_price), ("Quantity", &min_qty, &max_qty)] {
        let group = w::hbox(8);
        let l = w::line(label, &["body-medium", "dim"]);
        l.set_width_chars(8);
        group.append(&l);
        group.append(lo);
        group.append(&w::line("–", &["dim"]));
        group.append(hi);
        bounds.append(&group);
    }
    content.append(&bounds);

    let chips: Vec<gtk::Widget> = vec![
        filter_chip(ui, "Exclude high-sec", "Leave out orders in systems of 0.5 and above", |f| f.book.exclude_high, |f, v| f.book.exclude_high = v).upcast(),
        filter_chip(ui, "Exclude low-sec", "Leave out orders in systems from 0.1 to 0.4", |f| f.book.exclude_low, |f, v| f.book.exclude_low = v).upcast(),
        filter_chip(ui, "Exclude null-sec", "Leave out orders in systems of 0.0 and below", |f| f.book.exclude_null, |f, v| f.book.exclude_null = v).upcast(),
        filter_chip(ui, "Exclude NPC stations", "Only orders in player structures", |f| f.book.exclude_npc, |f, v| f.book.exclude_npc = v).upcast(),
        filter_chip(ui, "Exclude structures", "Only orders in NPC stations", |f| f.book.exclude_structures, |f, v| f.book.exclude_structures = v).upcast(),
        filter_chip(ui, "Market hubs only", "Only Jita 4-4, Amarr, Dodixie, Rens and Hek's hub stations", |f| f.book.hubs_only, |f, v| f.book.hubs_only = v).upcast(),
        filter_chip(ui, "My orders only", "Only orders placed by your characters", |f| f.mine_only, |f, v| f.mine_only = v).upcast(),
    ];
    let chip_row = gtk::FlowBox::new();
    chip_row.set_selection_mode(gtk::SelectionMode::None);
    chip_row.set_column_spacing(8);
    chip_row.set_row_spacing(8);
    chip_row.set_max_children_per_line(8);
    for c in &chips {
        chip_row.append(c);
    }
    content.append(&chip_row);
    revealer.set_child(Some(&content));
    panel.append(&revealer);

    {
        let (ctx, revealer) = (ui.ctx.clone(), revealer.clone());
        toggle.connect_clicked(move |b| {
            let open = !revealer.reveals_child();
            revealer.set_reveal_child(open);
            ctx.remember(KEY_FILTERS_OPEN, if open { "1" } else { "0" });
            if let Some(icon) = b.child().and_then(|c| c.first_child()).and_downcast::<gtk::Label>() {
                icon.set_text(if open { "expand_less" } else { "filter_list" });
            }
        });
    }
    {
        let wk = Rc::downgrade(ui);
        let entries = [min_price, max_price, min_qty, max_qty];
        reset.connect_clicked(move |_| {
            let Some(ui) = wk.upgrade() else { return };
            // the widgets' handlers write their (now default) values back, so clear them first
            for e in &entries {
                e.set_text("");
            }
            for c in &chips {
                if let Some(t) = c.downcast_ref::<gtk::ToggleButton>() {
                    t.set_active(false);
                }
            }
            ui.st.borrow_mut().filters = Filters::default();
            ui.filters_changed();
        });
    }
    ui.update_filter_count();
    panel
}

impl Ui {
    fn weak(self: &Rc<Self>) -> Weak<Ui> {
        Rc::downgrade(self)
    }

    fn filters_changed(self: &Rc<Self>) {
        let json = serde_json::to_string(&self.st.borrow().filters).unwrap_or_default();
        self.ctx.remember(KEY_FILTERS, json);
        self.update_filter_count();
        self.render_book();
    }

    fn update_filter_count(&self) {
        let n = self.st.borrow().filters.active();
        self.filter_count.set_text(&format!("{n} on"));
        self.filter_count.set_visible(n > 0);
    }

    /// Selects an item: its header now, then its order book and history.
    fn show_type(self: &Rc<Self>, id: i64) {
        let Some(t) = MARKET.get(id) else { return };
        self.ctx.remember(KEY_TYPE, id.to_string());
        {
            let mut st = self.st.borrow_mut();
            st.type_id = Some(id);
        }
        self.detail.set_visible_child_name("item");

        let header = w::hbox(16);
        header.append(&w::eve_image(&config::type_icon(id, 64), 48));
        let titles = w::vbox(2);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);
        let name = w::line(&t.name, &["title-large"]);
        name.set_selectable(true);
        titles.append(&name);
        let path: Vec<&str> = MARKET.path(t.group).iter().map(|g| g.name.as_str()).collect();
        let mut sub = path.join(" › ");
        if t.volume > 0.0 {
            sub.push_str(&format!("  ·  {} m³", fmt::decimal(t.volume, if t.volume < 10.0 { 2 } else { 0 })));
        }
        titles.append(&w::line(&sub, &["body-medium", "dim"]));
        header.append(&titles);
        let refresh = w::icon_button("refresh", "Reload the order book");
        {
            let wk = self.weak();
            refresh.connect_clicked(move |_| {
                if let Some(ui) = wk.upgrade() {
                    ui.load_book(true);
                }
            });
        }
        header.append(&refresh);
        let copy = w::icon_button("content_copy", "Copy the name");
        {
            let (ctx, name) = (self.ctx.clone(), t.name.clone());
            copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &name));
        }
        header.append(&copy);
        let web = w::icon_button("open_in_new", "Open on EVE Tycoon");
        {
            let ctx = self.ctx.clone();
            web.connect_clicked(move |_| w::open_url(&ctx, &format!("https://evetycoon.com/market/{id}")));
        }
        header.append(&web);
        fill(&self.header, &header);

        if let Some(tree) = self.tree.borrow().as_ref() {
            let mut list = tree.bound.borrow_mut();
            list.retain(|(_, r)| r.upgrade().is_some());
            for (t, r) in list.iter() {
                if let Some(b) = r.upgrade() {
                    if *t == id {
                        b.add_css_class("mkt-browser-current");
                    } else {
                        b.remove_css_class("mkt-browser-current");
                    }
                }
            }
        }
        self.load_book(false);
        self.load_history();
    }

    /// Expands the tree down to an item and scrolls to it.
    fn reveal_in_tree(&self, id: i64) {
        let Some(t) = MARKET.get(id) else { return };
        let tree = self.tree.borrow();
        let Some(tree) = tree.as_ref() else { return };
        let find = |want: &dyn Fn(Node) -> bool| -> Option<(u32, gtk::TreeListRow)> {
            (0..tree.model.n_items()).find_map(|i| {
                let row = tree.model.row(i)?;
                let node = row.item().as_ref().and_then(node_of)?;
                want(node).then_some((i, row))
            })
        };
        for g in MARKET.path(t.group) {
            let gid = g.id;
            match find(&|n| matches!(n, Node::Group(x) if x == gid)) {
                Some((_, row)) => row.set_expanded(true),
                None => return,
            }
        }
        if let Some((pos, _)) = find(&|n| matches!(n, Node::Type(x) if x == id)) {
            tree.view.scroll_to(pos, gtk::ListScrollFlags::NONE, None);
        }
    }

    /// The regions to load for the current scope.
    fn scope_regions(&self) -> Vec<i64> {
        let st = self.st.borrow();
        if st.type_id == Some(PLEX_TYPE_ID) {
            return vec![GLOBAL_PLEX_MARKET];
        }
        match st.mode {
            Mode::Region => vec![st.region],
            Mode::Hubs => hub_regions(),
            Mode::All => regions().into_iter().map(|r| r.0).collect(),
        }
    }

    fn load_book(self: &Rc<Self>, force: bool) {
        let Some(type_id) = self.st.borrow().type_id else { return };
        let (mode, region) = {
            let st = self.st.borrow();
            (st.mode, st.region)
        };
        let regions = self.scope_regions();
        let gen = self.book_gen.get() + 1;
        self.book_gen.set(gen);
        {
            let mut st = self.st.borrow_mut();
            st.loading = true;
            st.book_error = None;
            if st.book.as_ref().is_some_and(|b| b.type_id != type_id || b.mode != mode || b.region != region) {
                st.book = None;
            }
        }

        let total = regions.len();
        let done = Arc::new(AtomicUsize::new(0));
        if total > 1 {
            let bar = w::progress(0.0);
            bar.set_show_text(true);
            bar.set_text(Some(&format!("0 / {total} regions")));
            bar.set_size_request(200, -1);
            fill(&self.status, &bar);
            let (wk, done) = (self.weak(), done.clone());
            glib::timeout_add_local(StdDuration::from_millis(150), move || {
                let Some(ui) = wk.upgrade() else { return glib::ControlFlow::Break };
                if ui.book_gen.get() != gen || !ui.st.borrow().loading {
                    return glib::ControlFlow::Break;
                }
                let n = done.load(AtomicOrdering::Relaxed);
                bar.set_fraction(n as f64 / total as f64);
                bar.set_text(Some(&format!("{n} / {total} regions")));
                glib::ControlFlow::Continue
            });
        } else {
            let row = w::hbox(8);
            let spinner = gtk::Spinner::new();
            spinner.set_spinning(true);
            row.append(&spinner);
            row.append(&w::line("Loading orders…", &["body-small", "dim"]));
            fill(&self.status, &row);
        }
        self.render_book();

        let core = self.ctx.core().clone();
        let wk = self.weak();
        crate::rt::run(
            async move {
                if regions.len() == 1 {
                    return core.market.orders(&core.esi, regions[0], type_id, force).await.map(|b| (b.to_vec(), 0)).map_err(|e| e.to_string());
                }
                let mut all = Vec::new();
                let mut failed = 0;
                for chunk in regions.chunks(REGION_STEP) {
                    let (orders, f) = core.market.orders_in(&core.esi, chunk, type_id, force).await;
                    all.extend(orders);
                    failed += f;
                    done.fetch_add(chunk.len(), AtomicOrdering::Relaxed);
                }
                if failed == regions.len() {
                    return Err("ESI didn't answer for any region".to_string());
                }
                if mode == Mode::Hubs && type_id != PLEX_TYPE_ID {
                    all.retain(|o| TRADE_HUBS.iter().any(|h| h.1 == o.location_id));
                }
                Ok((all, failed))
            },
            move |result| {
                let Some(ui) = wk.upgrade() else { return };
                if ui.book_gen.get() != gen {
                    return;
                }
                {
                    let mut st = ui.st.borrow_mut();
                    st.loading = false;
                    match result {
                        Ok((orders, failed)) => {
                            st.book = Some(Book { type_id, mode, region, orders: Rc::new(orders), failed, regions: total, at: Utc::now() });
                        }
                        Err(e) => st.book_error = Some(e),
                    }
                }
                ui.render_status();
                ui.render_book();
                ui.resolve_names();
            },
        );
    }

    fn render_status(&self) {
        let st = self.st.borrow();
        let row = w::hbox(8);
        if st.type_id == Some(PLEX_TYPE_ID) {
            let chip = w::status("Global PLEX market", Status::Info);
            chip.set_tooltip_text(Some("PLEX trades on one market for all of New Eden, whichever region or scope is picked"));
            row.append(&chip);
        }
        if let Some(e) = &st.book_error {
            let l = w::line(&format!("Couldn't load orders: {e}"), &["body-small", "error-text"]);
            l.set_tooltip_text(Some(e));
            row.append(&l);
        } else if let Some(b) = &st.book {
            if b.failed > 0 {
                row.append(&w::line(&format!("{} of {} regions failed", b.failed, b.regions), &["body-small", "warning-text"]));
            }
            let when = b.at.with_timezone(&chrono::Local).format("%H:%M").to_string();
            row.append(&w::line(&format!("{} orders · updated {when}", fmt::int(b.orders.len() as i64)), &["body-small", "dim"]));
        }
        fill(&self.status, &row);
    }

    /// Names the stations and structures in the loaded book (cached by `universe`, so usually instant).
    fn resolve_names(self: &Rc<Self>) {
        let ids: Vec<i64> = {
            let st = self.st.borrow();
            let Some(b) = &st.book else { return };
            let seen: HashSet<i64> = b.orders.iter().map(|o| o.location_id).filter(|id| !st.names.contains_key(id)).collect();
            seen.into_iter().collect()
        };
        if ids.is_empty() {
            return;
        }
        let core = self.ctx.core().clone();
        let wk = self.weak();
        crate::rt::run(location_names(core, ids), move |names| {
            let Some(ui) = wk.upgrade() else { return };
            if names.is_empty() {
                return;
            }
            ui.st.borrow_mut().names.extend(names);
            ui.render_book();
        });
    }

    /// Statistics and the order tables, from the loaded book and the filters.
    fn render_book(&self) {
        let st = self.st.borrow();
        let Some(book) = &st.book else {
            let msg = if st.loading {
                "Loading the order book…".to_string()
            } else if let Some(e) = &st.book_error {
                format!("The order book didn't load: {e}")
            } else {
                String::new()
            };
            fill(&self.stats, &w::text(&msg, &["body-medium", "dim"]));
            fill(&self.orders, &w::vbox(0));
            return;
        };
        let own = own_order_ids(self.ctx.core());
        let applied = st.filters.applied();
        let orders: Vec<&BookOrder> = book.orders.iter().filter(|o| applied.keep(o, &own)).collect();
        let stats = trade::book_stats(orders.iter().copied());
        fill(&self.stats, &stats_view(&stats, orders.len(), book.orders.len()));

        let mut sellers: Vec<Row> = Vec::new();
        let mut buyers: Vec<Row> = Vec::new();
        for o in &orders {
            let r = Row::new(o, &st.names, &own);
            if o.is_buy_order {
                buyers.push(r);
            } else {
                sellers.push(r);
            }
        }
        // already in the tables' initial order, so sorting doesn't move the view
        sellers.sort_by(|a, b| a.o.price.total_cmp(&b.o.price));
        buyers.sort_by(|a, b| b.o.price.total_cmp(&a.o.price));
        // 0 sellers, 1 buyers; the switch remembers it
        let side = self.ctx.recall(KEY_SIDE).and_then(|s| s.parse().ok()).filter(|s| *s < 2).unwrap_or(0);
        fill(&self.orders, &self.orders_card(sellers, buyers, side));
    }

    fn orders_card(&self, sellers: Vec<Row>, buyers: Vec<Row>, side: usize) -> gtk::Box {
        let (ns, nb) = (sellers.len(), buyers.len());
        let tables = gtk::Stack::new();
        tables.set_vhomogeneous(false);
        tables.add_named(&order_table(sellers, false), Some("0"));
        tables.add_named(&order_table(buyers, true), Some("1"));
        tables.set_visible_child_name(&side.to_string());
        let sells = format!("Sellers ({})", fmt::int(ns as i64));
        let buys = format!("Buyers ({})", fmt::int(nb as i64));
        let switch = {
            let (ctx, tables) = (self.ctx.clone(), tables.clone());
            w::segmented(&[&sells, &buys], side, move |i| {
                ctx.remember(KEY_SIDE, i.to_string());
                tables.set_visible_child_name(&i.to_string());
            })
        };
        w::card_with("Orders", Some("Your orders are marked. Double-check prices in game before trading."), &[switch.upcast()], &tables)
    }

    fn load_history(self: &Rc<Self>) {
        let Some(type_id) = self.st.borrow().type_id else { return };
        let region = self.history_region();
        if let Some((r, t, History::Loaded(_))) = &self.st.borrow().history {
            if *r == region && *t == type_id {
                self.render_history();
                return;
            }
        }
        let gen = self.history_gen.get() + 1;
        self.history_gen.set(gen);
        self.st.borrow_mut().history = Some((region, type_id, History::Loading));
        self.render_history();
        let core = self.ctx.core().clone();
        let wk = self.weak();
        crate::rt::run(async move { core.market.history(&core.esi, region, type_id).await.map_err(|e| e.to_string()) }, move |r| {
            let Some(ui) = wk.upgrade() else { return };
            if ui.history_gen.get() != gen {
                return;
            }
            ui.st.borrow_mut().history = Some((
                region,
                type_id,
                match r {
                    Ok(days) => History::Loaded(Rc::new(days.to_vec())),
                    Err(e) => History::Failed(e),
                },
            ));
            ui.render_history();
        });
    }

    /// History is per region: the selected one, or The Forge for the hubs and all regions.
    fn history_region(&self) -> i64 {
        let st = self.st.borrow();
        if st.type_id == Some(PLEX_TYPE_ID) {
            GLOBAL_PLEX_MARKET
        } else if st.mode == Mode::Region {
            st.region
        } else {
            THE_FORGE
        }
    }

    fn render_history(self: &Rc<Self>) {
        let st = self.st.borrow();
        let Some((region, _, h)) = &st.history else { return };
        let region_name = if *region == GLOBAL_PLEX_MARKET { "the global PLEX market" } else { MAP.region_name(*region).unwrap_or("?") };
        let subtitle = match st.mode {
            _ if *region == GLOBAL_PLEX_MARKET => "Daily trading on the global PLEX market".to_string(),
            Mode::Region => format!("Daily trading in {region_name}"),
            _ => format!("Daily trading in {region_name}: ESI only keeps price history per region"),
        };
        let range_switch = {
            let labels: Vec<&str> = RANGES.iter().map(|r| r.0).collect();
            let wk = self.weak();
            w::segmented(&labels, st.range, move |i| {
                let Some(ui) = wk.upgrade() else { return };
                ui.ctx.remember(KEY_RANGE, i.to_string());
                ui.st.borrow_mut().range = i;
                // after the toggle handler: rendering replaces the switch being clicked
                glib::idle_add_local_once(move || ui.render_history());
            })
        };
        let body = w::vbox(12);
        match h {
            History::Loading => {
                let row = w::hbox(8);
                let spinner = gtk::Spinner::new();
                spinner.set_spinning(true);
                row.append(&spinner);
                row.append(&w::line("Loading price history…", &["body-medium", "dim"]));
                body.append(&row);
            }
            History::Failed(e) => body.append(&w::text(&format!("Price history didn't load: {e}"), &["body-medium", "error-text"])),
            History::Loaded(days) if days.is_empty() => body.append(&w::text(&format!("Nobody traded this in {region_name} in the last year."), &["body-medium", "dim"])),
            History::Loaded(days) => {
                let n = RANGES[st.range].1;
                let last = days.last().map(|d| d.date).unwrap_or_default();
                let shown: Vec<HistoryDay> = days.iter().filter(|d| (last - d.date).num_days() < n).cloned().collect();
                body.append(&history_chart(Rc::new(shown)));
                body.append(&legend());
                body.append(&summary_tiles(days));
            }
        }
        let card = w::card_with("Price history", Some(&subtitle), &[range_switch.upcast()], &body);
        drop(st);
        fill(&self.history, &card);
    }
}

/// Order ids of every character's market orders.
fn own_order_ids(core: &Core) -> HashSet<i64> {
    core.characters().iter().filter_map(|c| c.market_orders.as_ref()).flat_map(|m| m.orders.iter().map(|o| o.order_id)).collect()
}

/// Station names (public) and, through a character with docking access, structure names.
async fn location_names(core: Arc<Core>, ids: Vec<i64>) -> HashMap<i64, String> {
    let (stations, structures): (Vec<i64>, Vec<i64>) = ids.into_iter().partition(|id| universe::is_station(*id));
    let mut out = if stations.is_empty() { HashMap::new() } else { core.universe.names(&core.esi, stations).await };
    if structures.is_empty() {
        return out;
    }
    let Some(who) = core.auth.all().into_iter().find(|a| a.has_scope(STRUCTURE_SCOPE) && !a.needs_reauth()) else { return out };
    let Ok(auth) = core.auth.auth(&core.esi.http, who.id, 60).await else { return out };
    let wanted: Vec<i64> = structures.into_iter().take(MAX_STRUCTURE_LOOKUPS).collect();
    for chunk in wanted.chunks(10) {
        let futs = chunk.iter().map(|&id| {
            let (core, auth) = (&core, &auth);
            async move { (id, core.universe.structure(&core.esi, id, auth).await) }
        });
        for (id, s) in futures::future::join_all(futs).await {
            if let Some(s) = s {
                out.insert(id, s.name);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------
// Statistics

fn side_volume_sub(s: &SideStats, buy: bool) -> String {
    let mut out = format!("{} orders", fmt::int(s.orders as i64));
    if s.outliers > 0 {
        let rel = if buy { "under" } else { "over" };
        out.push_str(&format!(" · {} outliers {rel} {}", s.outliers, opt_price(s.threshold)));
    }
    out
}

fn tile(label: &str, value: String, sub: Option<String>, tooltip: Option<String>) -> gtk::Box {
    let t = w::stat_tile(label, &value, sub.as_deref());
    t.set_tooltip_text(tooltip.as_deref());
    t
}

fn stats_view(s: &BookStats, shown: usize, total: usize) -> gtk::Box {
    let b = w::vbox(8);
    if shown == 0 {
        let msg = if total == 0 { "Nobody buys or sells this here right now." } else { "No orders pass the filters." };
        b.append(&w::empty_state("search_off", "No orders", msg));
        return b;
    }
    let five = "Volume-weighted average price of the best 5% of the volume, after dropping outliers";
    let margin_pct = match (s.margin(), s.sell.five_percent) {
        (Some(m), Some(sell)) if sell > 0.0 => Some(format!("{:.1}% of the 5% sell", m / sell * 100.0)),
        _ => None,
    };
    let spread_pct = match (s.spread(), s.sell.best) {
        (Some(sp), Some(sell)) if sell > 0.0 => Some(format!("{:.1}% of the best sell", sp / sell * 100.0)),
        _ => None,
    };
    let full = |v: Option<f64>| v.map(fmt::isk);
    let tiles = vec![
        tile("5% sell", opt_price(s.sell.five_percent), s.sell.average.map(|a| format!("Average {}", price(a))), Some(format!("{five}.\n{}", full(s.sell.five_percent).unwrap_or_default()))),
        tile("5% buy", opt_price(s.buy.five_percent), s.buy.average.map(|a| format!("Average {}", price(a))), Some(format!("{five}.\n{}", full(s.buy.five_percent).unwrap_or_default()))),
        tile("Best sell", opt_price(s.sell.best), Some("Lowest sell order".into()), full(s.sell.best)),
        tile("Best buy", opt_price(s.buy.best), Some("Highest buy order".into()), full(s.buy.best)),
        tile("Spread", opt_price(s.spread()), spread_pct, Some("Best sell − best buy".into())),
        tile("Margin", opt_price(s.margin()), margin_pct, Some("5% sell − 5% buy, before fees and taxes".into())),
        tile("Sell volume", fmt::int(s.sell.volume), Some(side_volume_sub(&s.sell, false)), Some("Units for sale, outliers excluded".into())),
        tile("Buy volume", fmt::int(s.buy.volume), Some(side_volume_sub(&s.buy, true)), Some("Units wanted, outliers excluded".into())),
    ];
    let row = w::tile_row(tiles);
    row.set_max_children_per_line(4);
    row.add_css_class("mkt-browser-tiles");
    b.append(&row);
    b.append(&w::text(
        "5% prices are the volume-weighted average of the cheapest 5% of sell volume and the highest-priced 5% of buy volume, \
         like EVE Tycoon's. Outliers are dropped first: sells over 10× the lowest sell and buys under 10% of the highest buy.",
        &["body-small", "dim"],
    ));
    b
}

// ---------------------------------------------------------------------------------------------------------------
// Order tables

struct Row {
    o: BookOrder,
    location: String,
    /// Shown when the name is a fallback.
    location_tip: Option<String>,
    sec: Option<(f64, SecurityClass)>,
    mine: bool,
}

impl Row {
    fn new(o: &BookOrder, names: &HashMap<i64, String>, own: &HashSet<i64>) -> Self {
        let system = MAP.system(o.system_id);
        let (location, location_tip) = match names.get(&o.location_id) {
            Some(n) => (n.clone(), None),
            None if !o.in_npc_station() => {
                let tip = Some(format!("Structure #{}: its name needs a character with docking access", o.location_id));
                (system.map(|s| format!("{} · structure", s.name)).unwrap_or_else(|| format!("Structure #{}", o.location_id)), tip)
            }
            None => (system.map(|s| s.name.clone()).unwrap_or_else(|| format!("Station #{}", o.location_id)), None),
        };
        Row {
            o: o.clone(),
            location,
            location_tip,
            sec: system.map(|s| (s.display_security(), s.security_class())),
            mine: own.contains(&o.order_id),
        }
    }
}

fn sec_badge(sec: Option<(f64, SecurityClass)>) -> gtk::Label {
    let (text, class) = match sec {
        Some((s, SecurityClass::High)) => (fmt::security(s), "high"),
        Some((s, SecurityClass::Low)) => (fmt::security(s), "low"),
        Some((s, SecurityClass::Null)) => (fmt::security(s), "null"),
        None => ("?".into(), "null"),
    };
    let l = gtk::Label::new(Some(&text));
    l.add_css_class("mkt-browser-sec");
    l.add_css_class(class);
    l.set_valign(gtk::Align::Center);
    l
}

fn location_cell(r: &Row) -> gtk::Widget {
    let b = w::hbox(8);
    b.append(&sec_badge(r.sec));
    if r.mine {
        b.append(&w::status("Yours", Status::Info));
    }
    let l = w::line(&r.location, &[]);
    l.set_hexpand(true);
    l.set_tooltip_text(Some(r.location_tip.as_deref().unwrap_or(&r.location)));
    b.append(&l);
    b.upcast()
}

fn dim_right(t: &str, tooltip: &str) -> gtk::Widget {
    let l = w::line(t, &["dim", "numeric"]);
    l.set_xalign(1.0);
    l.set_tooltip_text(Some(tooltip));
    l.upcast()
}

fn order_table(rows: Vec<Row>, buyers: bool) -> gtk::Widget {
    let now = Utc::now();
    let mut t = Table::new(rows)
        .text("Region", |r| MAP.region_name(r.o.region_id).unwrap_or(if r.o.region_id == GLOBAL_PLEX_MARKET { "Global" } else { "?" }).to_string())
        .width(130)
        .num("Quantity", |r| fmt::int(r.o.volume_remain), |r| r.o.volume_remain as f64)
        .width(130)
        .num("Price", |r| fmt::decimal(r.o.price, 2), |r| r.o.price)
        .width(130)
        .widget_sorted("Location", location_cell, |a, b| a.location.to_lowercase().cmp(&b.location.to_lowercase()))
        .width(300)
        .expand();
    if buyers {
        t = t
            .text("Range", |r| comms::order_range_label(&r.o.range))
            .num("Min volume", |r| fmt::int(r.o.min_volume.max(1)), |r| r.o.min_volume as f64);
    }
    t.widget_sorted(
        "Expires in",
        move |r| {
            let left = r.o.expires() - now;
            let text = if left <= chrono::Duration::zero() { "Expired".to_string() } else { fmt::duration(left, 2) };
            dim_right(&text, &fmt::datetime(r.o.expires()))
        },
        |a, b| a.o.expires().cmp(&b.o.expires()),
    )
    .widget_sorted("Issued", |r| dim_right(&fmt::relative(r.o.issued), &fmt::datetime(r.o.issued)), |a, b| a.o.issued.cmp(&b.o.issued))
    .sort_by(2, buyers)
    .min_height(440)
    .empty_text(if buyers { "No buy orders." } else { "No sell orders." })
    .build()
}

// ---------------------------------------------------------------------------------------------------------------
// Price history

const CHART_HEIGHT: i32 = 300;
const PAD_L: f64 = 64.0;
const PAD_R: f64 = 12.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 26.0;
/// Share of the plot the price takes; the volume bars sit in the rest, below a gap.
const PRICE_SHARE: f64 = 0.72;
const VOLUME_TOP: f64 = 0.78;

/// Price bounds for the chart: the daily lows and highs, but not so far from the averages that one wild trade
/// flattens the line.
fn price_bounds(days: &[HistoryDay]) -> (f64, f64) {
    let amin = days.iter().map(|d| d.average).fold(f64::INFINITY, f64::min);
    let amax = days.iter().map(|d| d.average).fold(f64::NEG_INFINITY, f64::max);
    let span = (amax - amin).max(amax.abs() * 0.02).max(1e-6);
    let lo = days.iter().map(|d| d.lowest).fold(f64::INFINITY, f64::min).max(amin - span).max(0.0);
    let hi = days.iter().map(|d| d.highest).fold(f64::NEG_INFINITY, f64::max).min(amax + span);
    let (lo, hi) = (lo.min(amin), hi.max(amax));
    let pad = (hi - lo) * 0.06;
    ((lo - pad).max(0.0), hi + pad)
}

struct Geo {
    n: usize,
    lo: f64,
    hi: f64,
    vmax: f64,
    w: f64,
    h: f64,
}

impl Geo {
    fn new(days: &[HistoryDay], w: f64, h: f64) -> Self {
        let (lo, hi) = price_bounds(days);
        let vmax = days.iter().map(|d| d.volume).max().unwrap_or(1).max(1) as f64;
        Geo { n: days.len().max(1), lo, hi: hi.max(lo + 1e-6), vmax, w, h }
    }
    fn plot_w(&self) -> f64 {
        (self.w - PAD_L - PAD_R).max(1.0)
    }
    fn plot_h(&self) -> f64 {
        (self.h - PAD_T - PAD_B).max(1.0)
    }
    fn x(&self, i: usize) -> f64 {
        PAD_L + (i as f64 + 0.5) / self.n as f64 * self.plot_w()
    }
    fn y(&self, v: f64) -> f64 {
        PAD_T + (1.0 - (v - self.lo) / (self.hi - self.lo)) * self.plot_h() * PRICE_SHARE
    }
    fn vol_top(&self) -> f64 {
        PAD_T + self.plot_h() * VOLUME_TOP
    }
    fn vol_h(&self) -> f64 {
        PAD_T + self.plot_h() - self.vol_top()
    }
    fn index_at(&self, px: f64) -> usize {
        (((px - PAD_L) / self.plot_w() * self.n as f64).floor().max(0.0) as usize).min(self.n - 1)
    }
}

fn source(cr: &gtk::cairo::Context, role: &str, alpha: f64) {
    let c = crate::theme::rgba(role);
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64 * alpha);
}

/// Daily average line over the low–high band, volume bars below, and a crosshair readout.
fn history_chart(days: Rc<Vec<HistoryDay>>) -> gtk::Widget {
    let area = gtk::DrawingArea::new();
    area.set_content_height(CHART_HEIGHT);
    area.set_hexpand(true);
    let hover: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    {
        let (days, hover) = (days.clone(), hover.clone());
        area.set_draw_func(move |_, cr, width, height| {
            if days.is_empty() {
                return;
            }
            let g = Geo::new(&days, width as f64, height as f64);
            cr.select_font_face("Inter", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(11.0);

            for v in comms::nice_ticks(g.lo, g.hi, 4) {
                let y = g.y(v).round() + 0.5;
                source(cr, "outline_variant", 0.6);
                cr.set_line_width(1.0);
                cr.move_to(PAD_L, y);
                cr.line_to(PAD_L + g.plot_w(), y);
                let _ = cr.stroke();
                let label = price(v);
                if let Ok(ext) = cr.text_extents(&label) {
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(PAD_L - 8.0 - ext.width() - ext.x_bearing(), y + ext.height() / 2.0);
                    let _ = cr.show_text(&label);
                }
            }
            // the largest day's volume, at the top of the bars
            let vlabel = fmt::isk_short(g.vmax);
            if let Ok(ext) = cr.text_extents(&vlabel) {
                source(cr, "on_surface_variant", 0.8);
                cr.move_to(PAD_L - 8.0 - ext.width() - ext.x_bearing(), g.vol_top() + ext.height());
                let _ = cr.show_text(&vlabel);
            }

            // date ticks
            let count = ((g.plot_w() / 110.0).floor() as usize).clamp(2, 8).min(days.len());
            for k in 0..count {
                let i = if count == 1 { 0 } else { k * (days.len() - 1) / (count - 1) };
                let label = days[i].date.format("%b %-d").to_string();
                if let Ok(ext) = cr.text_extents(&label) {
                    let x = (g.x(i) - ext.width() / 2.0).clamp(PAD_L, PAD_L + g.plot_w() - ext.width());
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(x, g.h - 8.0);
                    let _ = cr.show_text(&label);
                }
            }

            // volume bars
            let bar_w = (g.plot_w() / g.n as f64 * 0.7).clamp(1.0, 14.0);
            for (i, d) in days.iter().enumerate() {
                let bh = d.volume as f64 / g.vmax * g.vol_h();
                source(cr, "secondary", if hover.get() == Some(i) { 0.95 } else { 0.5 });
                cr.rectangle(g.x(i) - bar_w / 2.0, g.vol_top() + g.vol_h() - bh, bar_w, bh.max(0.5));
                let _ = cr.fill();
            }

            // the low–high band and the average, clipped to the price area
            let _ = cr.save();
            cr.rectangle(PAD_L, PAD_T, g.plot_w(), g.plot_h() * PRICE_SHARE);
            cr.clip();
            cr.move_to(g.x(0), g.y(days[0].highest));
            for (i, d) in days.iter().enumerate().skip(1) {
                cr.line_to(g.x(i), g.y(d.highest));
            }
            for (i, d) in days.iter().enumerate().rev() {
                cr.line_to(g.x(i), g.y(d.lowest));
            }
            cr.close_path();
            source(cr, "primary", 0.16);
            let _ = cr.fill();
            cr.move_to(g.x(0), g.y(days[0].average));
            for (i, d) in days.iter().enumerate().skip(1) {
                cr.line_to(g.x(i), g.y(d.average));
            }
            source(cr, "primary", 1.0);
            cr.set_line_width(2.0);
            cr.set_line_join(gtk::cairo::LineJoin::Round);
            let _ = cr.stroke();
            let _ = cr.restore();

            if let Some(i) = hover.get().filter(|i| *i < days.len()) {
                let (x, y) = (g.x(i), g.y(days[i].average));
                source(cr, "outline", 0.8);
                cr.set_line_width(1.0);
                cr.set_dash(&[4.0, 3.0], 0.0);
                cr.move_to(x.round() + 0.5, PAD_T);
                cr.line_to(x.round() + 0.5, PAD_T + g.plot_h());
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
                source(cr, "surface", 1.0);
                cr.arc(x, y, 6.5, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
                source(cr, "primary", 1.0);
                cr.arc(x, y, 4.5, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
        });
    }

    let tip = w::vbox(2);
    tip.add_css_class("mkt-browser-chart-tip");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);
    let tip_date = w::line("", &["body-small", "dim"]);
    let tip_avg = w::line("", &["title-small", "numeric"]);
    let tip_range = w::line("", &["body-small", "numeric"]);
    let tip_volume = w::line("", &["body-small", "numeric"]);
    for l in [&tip_date, &tip_avg, &tip_range, &tip_volume] {
        tip.append(l);
    }

    let motion = gtk::EventControllerMotion::new();
    {
        let (area, days, hover, tip) = (area.clone(), days.clone(), hover.clone(), tip.clone());
        motion.connect_motion(move |_, px, _| {
            if days.is_empty() {
                return;
            }
            let g = Geo::new(&days, area.width() as f64, area.height() as f64);
            let i = g.index_at(px);
            if hover.get() == Some(i) {
                return;
            }
            hover.set(Some(i));
            let d = &days[i];
            tip_date.set_text(&d.date.format("%a %b %-d, %Y").to_string());
            tip_avg.set_text(&format!("Average {}", fmt::isk(d.average)));
            tip_range.set_text(&format!("Low {} · High {}", price(d.lowest), price(d.highest)));
            tip_volume.set_text(&format!("Volume {} · {} orders", fmt::int(d.volume), fmt::int(d.order_count)));
            let x = g.x(i);
            let left = if x + 250.0 > g.w { x - 242.0 } else { x + 12.0 };
            tip.set_margin_start(left.max(PAD_L) as i32);
            tip.set_margin_top(PAD_T as i32);
            tip.set_visible(true);
            area.queue_draw();
        });
    }
    {
        let (area, hover, tip) = (area.clone(), hover.clone(), tip.clone());
        motion.connect_leave(move |_| {
            hover.set(None);
            tip.set_visible(false);
            area.queue_draw();
        });
    }
    area.add_controller(motion);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    overlay.add_overlay(&tip);
    overlay.upcast()
}

fn legend() -> gtk::Box {
    let row = w::hbox(16);
    for (class, label) in [("avg", "Daily average"), ("band", "Low – high"), ("volume", "Volume")] {
        let item = w::hbox(6);
        let dot = w::hbox(0);
        dot.add_css_class("mkt-browser-dot");
        dot.add_css_class(class);
        dot.set_valign(gtk::Align::Center);
        item.append(&dot);
        let l = gtk::Label::new(Some(label));
        l.add_css_class("body-small");
        l.add_css_class("dim");
        item.append(&l);
        row.append(&item);
    }
    row
}

/// The last 30 days of history at a glance.
#[derive(Debug, PartialEq)]
struct Summary {
    /// Units per calendar day (days nobody traded count as zero).
    avg_volume: f64,
    /// ISK traded per calendar day.
    avg_isk: f64,
    /// Latest daily average against the one 30 days before, in percent.
    change: Option<f64>,
}

fn summarize(days: &[HistoryDay]) -> Option<Summary> {
    let last = days.last()?;
    let recent: Vec<&HistoryDay> = days.iter().filter(|d| (last.date - d.date).num_days() < 30).collect();
    let avg_volume = recent.iter().map(|d| d.volume as f64).sum::<f64>() / 30.0;
    let avg_isk = recent.iter().map(|d| d.volume as f64 * d.average).sum::<f64>() / 30.0;
    let before = days.iter().rev().find(|d| (last.date - d.date).num_days() >= 30);
    let change = before.filter(|b| b.average > 0.0).map(|b| (last.average - b.average) / b.average * 100.0);
    Some(Summary { avg_volume, avg_isk, change })
}

fn summary_tiles(days: &[HistoryDay]) -> gtk::FlowBox {
    let Some(s) = summarize(days) else { return w::tile_row(vec![]) };
    let last = days.last().expect("summarize checked");
    let change = tile(
        "Price change (30d)",
        s.change.map(|c| format!("{}{c:.1}%", if c > 0.0 { "+" } else { "" })).unwrap_or_else(|| "–".into()),
        Some("Daily average vs. 30 days ago".into()),
        None,
    );
    match s.change {
        Some(c) if c > 0.05 => change.add_css_class("mkt-browser-up"),
        Some(c) if c < -0.05 => change.add_css_class("mkt-browser-down"),
        _ => {}
    }
    let row = w::tile_row(vec![
        tile("Avg daily volume (30d)", fmt::int(s.avg_volume.round() as i64), Some("Units per day".into()), None),
        change,
        tile("Avg daily ISK (30d)", fmt::isk_short(s.avg_isk), Some("Traded per day".into()), Some(fmt::isk(s.avg_isk))),
        tile("Latest average", price(last.average), Some(last.date.format("%b %-d, %Y").to_string()), Some(fmt::isk(last.average))),
    ]);
    row.set_max_children_per_line(4);
    row.add_css_class("mkt-browser-tiles");
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn day(d: u32, avg: f64, vol: i64) -> HistoryDay {
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(d as i64);
        HistoryDay { date, average: avg, highest: avg * 1.1, lowest: avg * 0.9, order_count: 10, volume: vol }
    }

    #[test]
    fn amounts() {
        assert_eq!(parse_amount(""), None);
        assert_eq!(parse_amount("  1,250 "), Some(1250.0));
        assert_eq!(parse_amount("1.5m"), Some(1.5e6));
        assert_eq!(parse_amount("20K"), Some(20e3));
        assert_eq!(parse_amount("2b"), Some(2e9));
        assert_eq!(parse_amount("abc"), None);
        assert_eq!(parse_amount("-5"), None);
    }

    #[test]
    fn filters() {
        let mut f = Filters::default();
        assert_eq!(f.active(), 0);
        f.min_price = "5".into();
        f.max_qty = "nope".into();
        f.book.exclude_null = true;
        assert_eq!(f.active(), 2);
        let o = BookOrder {
            order_id: 7,
            type_id: 34,
            is_buy_order: false,
            price: 4.0,
            volume_remain: 100,
            volume_total: 100,
            min_volume: 1,
            location_id: 60003760,
            system_id: 30000142,
            range: "region".into(),
            issued: Utc::now(),
            duration: 90,
            region_id: THE_FORGE,
        };
        let own = HashSet::from([7]);
        assert!(!f.applied().keep(&o, &own), "under the minimum price");
        f.min_price = "4".into();
        assert!(f.applied().keep(&o, &own));
        f.mine_only = true;
        assert!(f.applied().keep(&o, &own));
        assert!(!f.applied().keep(&o, &HashSet::new()));
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(serde_json::from_str::<Filters>(&json).unwrap(), f);
    }

    #[test]
    fn summary() {
        assert_eq!(summarize(&[]), None);
        // 40 days at 10 ISK, then the price doubles on the last day
        let mut days: Vec<HistoryDay> = (0..40).map(|d| day(d, 10.0, 300)).collect();
        days.push(day(40, 20.0, 300));
        let s = summarize(&days).unwrap();
        assert_eq!(s.avg_volume, 300.0);
        assert_eq!(s.change, Some(100.0));
        // a gap: only 3 traded days in the last 30
        let sparse = vec![day(0, 10.0, 90), day(20, 10.0, 90), day(29, 10.0, 90)];
        let s = summarize(&sparse).unwrap();
        assert_eq!(s.avg_volume, 9.0);
        assert_eq!(s.change, None);
    }

    #[test]
    fn bounds_ignore_wild_trades() {
        let mut days: Vec<HistoryDay> = (0..10).map(|d| day(d, 100.0 + d as f64, 1)).collect();
        days[3].highest = 10_000.0;
        days[4].lowest = 0.01;
        let (lo, hi) = price_bounds(&days);
        assert!(hi < 200.0 && lo > 50.0, "{lo}..{hi}");
        assert!(lo <= 90.0 && hi >= 109.0 * 1.1 - 1.0);
    }
}
