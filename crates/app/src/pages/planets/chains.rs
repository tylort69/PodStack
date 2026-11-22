//! Planetary Industry → Chains, in two views:
//!
//! - Scheme: every planet kind and commodity in columns from planets to P4. Picking one highlights everything it
//!   needs (down to the planets that have its raw resources) and everything it goes into (up to P4), and a card
//!   below shows its schematic, what uses it and which planets cover it.
//! - Calculator: pick a product and an amount (units, or factories running for some hours) and see the recipe
//!   tree, every step with factories needed, the raw resources and planet kinds, volumes, and what it's worth at
//!   hub prices bought at each tier against selling the output.

use super::common::{self, commodity_cell, icon_url, tier_badge, Prices};
use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::{fields, labelled, number_entry, parse_amount};
use crate::widgets::{self as w, Btn, Table};
use gtk::prelude::*;
use podstack_core::features::planetary as pl;
use podstack_core::fmt;
use podstack_core::pi::{Tier, PI};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};

const VIEW_KEY: &str = "planets.chains.view";
const SELECTED_KEY: &str = "planets.chains.selected";
const PRODUCT_KEY: &str = "planets.chains.product";
const TIER_KEY: &str = "planets.chains.tier";
const SEARCH_KEY: &str = "planets.chains.search";
const MODE_KEY: &str = "planets.chains.mode";
const QTY_KEY: &str = "planets.chains.quantity";
const FACTORIES_KEY: &str = "planets.chains.factories";
const HOURS_KEY: &str = "planets.chains.hours";
const SHOP_TIER_KEY: &str = "planets.chains.shop_tier";

const CSS: &str = "
button.pi-scheme-item { min-height: 0; padding: 3px 8px; border-radius: 8px; color: @on_surface; font-weight: 400; font-size: 13px; }
button.pi-scheme-item:hover { background-color: alpha(@on_surface, 0.08); }
button.pi-scheme-item.pi-related { background-color: alpha(@primary, 0.10); }
button.pi-scheme-item.pi-selected { background-color: @secondary_container; color: @on_secondary_container; font-weight: 600; }
.pi-dim { opacity: 0.28; }
.pi-scheme-head { padding: 0 8px 6px 8px; }
button.pi-product { min-height: 0; padding: 4px 10px; border-radius: 10px; color: @on_surface; font-weight: 400; font-size: 13px; border: 1px solid @outline_variant; }
button.pi-product.pi-selected { background-color: @secondary_container; color: @on_secondary_container; border-color: @secondary_container; }
.pi-kind { border-radius: 8px; padding: 2px 8px; font-size: 12px; background-color: @surface_container_highest; color: @on_surface_variant; }
.pi-kind.pi-kind-chosen { background-color: @secondary_container; color: @on_secondary_container; font-weight: 600; }
.pi-tree-qty { color: @on_surface_variant; }
";

/// Opens a product in the calculator view of this tab (from the Profit tab, say).
pub fn open_in_calculator(ctx: &Ctx, product: i64) {
    ctx.remember(PRODUCT_KEY, product.to_string());
    ctx.remember(VIEW_KEY, "calculator");
    super::open(ctx, Tab::Chains);
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(common::CSS);
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Chains));
    // the view last chosen, or `PODSTACK_PI_CHAINS_VIEW` for UI testing
    let view = ctx.recall(VIEW_KEY).or_else(|| std::env::var("PODSTACK_PI_CHAINS_VIEW").ok()).unwrap_or_default();
    let calculator = view.trim().eq_ignore_ascii_case("calculator");
    {
        let ctx = ctx.clone();
        body.append(&w::segmented(&["Scheme", "Calculator"], calculator as usize, move |i| {
            ctx.remember(VIEW_KEY, if i == 1 { "calculator" } else { "scheme" });
            // once the toggle handler has returned: the rebuild replaces the button being clicked
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        }));
    }
    if calculator {
        body.append(&calculator_view(ctx));
    } else {
        body.append(&scheme_view(ctx));
    }
    Page::new("Planetary Industry", body).static_page()
}

// ---------- shared bits ----------

fn cycle_text(secs: i64) -> String {
    if secs > 0 && secs % 3600 == 0 {
        format!("{} h", secs / 3600)
    } else if secs > 0 && secs % 60 == 0 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// Whole units needed (rounded up: you can't buy part of one).
fn units_text(n: f64) -> String {
    fmt::int((n - 1e-6).ceil().max(0.0) as i64)
}

/// A rate or count with one decimal unless it's whole.
fn rate_text(n: f64) -> String {
    if (n - n.round()).abs() < 0.05 {
        fmt::int(n.round() as i64)
    } else {
        fmt::decimal(n, 1)
    }
}

fn planet_icon(kind: usize, size: i32) -> gtk::Widget {
    match PI.kinds[kind].types.first() {
        Some(t) => w::eve_image(&icon_url(*t), size).upcast(),
        None => w::icon("public").upcast(),
    }
}

fn kind_label(key: &str) -> String {
    PI.kind(key).map(|k| k.label.to_string()).unwrap_or_else(|| key.to_string())
}

/// A small planet-kind tag; `chosen` marks the ones in the smallest covering set.
fn kind_tag(key: &str, chosen: bool) -> gtk::Label {
    let l = gtk::Label::new(Some(&kind_label(key)));
    l.add_css_class("pi-kind");
    if chosen {
        l.add_css_class("pi-kind-chosen");
    }
    l.set_valign(gtk::Align::Center);
    l
}

/// Widgets that wrap onto more lines when the page is narrow.
fn wrap(children: Vec<gtk::Widget>) -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_column_spacing(6);
    f.set_row_spacing(6);
    f.set_max_children_per_line(30);
    f.set_homogeneous(false);
    f.set_valign(gtk::Align::Start);
    for c in children {
        f.insert(&c, -1);
    }
    f
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn section(title: &str, body: &impl IsA<gtk::Widget>) -> gtk::Box {
    let v = w::vbox(6);
    v.append(&w::line(title, &["title-small", "dim"]));
    v.append(body);
    v
}

// ---------- scheme ----------

/// Something in the scheme: a planet kind (index into `PI.kinds`) or a commodity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Item {
    Kind(usize),
    Type(i64),
}

impl Item {
    fn key(self) -> String {
        match self {
            Item::Kind(k) => format!("k:{}", PI.kinds[k].key),
            Item::Type(t) => format!("t:{t}"),
        }
    }

    fn parse(s: &str) -> Option<Item> {
        let (kind, v) = s.split_once(':')?;
        match kind {
            "k" => PI.kinds.iter().position(|k| k.key == v).map(Item::Kind),
            "t" => v.parse().ok().filter(|t| PI.commodity(*t).is_some()).map(Item::Type),
            _ => None,
        }
    }

    fn name(self) -> String {
        match self {
            Item::Kind(k) => PI.kinds[k].label.to_string(),
            Item::Type(t) => PI.name(t),
        }
    }

    fn icon(self, size: i32) -> gtk::Widget {
        match self {
            Item::Kind(k) => planet_icon(k, size),
            Item::Type(t) => w::eve_image(&icon_url(t), size).upcast(),
        }
    }
}

/// Everything highlighted with `item`: what it needs down to raw resources and the planet kinds that have them,
/// and what it goes into up to P4. A planet kind leads to its resources and everything made from them.
fn related(item: Item) -> HashSet<Item> {
    let d = &*PI;
    let mut types: HashSet<i64> = HashSet::new();
    let mut kinds: HashSet<usize> = HashSet::new();
    match item {
        Item::Kind(k) => {
            kinds.insert(k);
            for r in &d.kinds[k].resources {
                types.insert(*r);
                types.extend(pl::downstream(*r));
            }
        }
        Item::Type(t) => {
            types.insert(t);
            let up = pl::upstream(t);
            let raw: Vec<i64> = up.iter().copied().chain([t]).filter(|x| d.tier(*x) == Some(0)).collect();
            for (i, k) in d.kinds.iter().enumerate() {
                if raw.iter().any(|r| k.resources.contains(r)) {
                    kinds.insert(i);
                }
            }
            types.extend(up);
            types.extend(pl::downstream(t));
        }
    }
    types.into_iter().map(Item::Type).chain(kinds.into_iter().map(Item::Kind)).collect()
}

struct Scheme {
    ctx: Ctx,
    items: RefCell<Vec<(Item, gtk::Button)>>,
    details: gtk::Box,
    clear: gtk::Button,
    selected: Cell<Option<Item>>,
}

impl Scheme {
    /// Selects `item`, or nothing.
    fn select(self: &Rc<Self>, item: Option<Item>) {
        self.selected.set(item);
        self.ctx.remember(SELECTED_KEY, item.map(|i| i.key()).unwrap_or_default());
        let rel = item.map(related);
        for (it, b) in self.items.borrow().iter() {
            for c in ["pi-selected", "pi-related", "pi-dim"] {
                b.remove_css_class(c);
            }
            if let (Some(sel), Some(rel)) = (item, &rel) {
                b.add_css_class(if *it == sel {
                    "pi-selected"
                } else if rel.contains(it) {
                    "pi-related"
                } else {
                    "pi-dim"
                });
            }
        }
        self.clear.set_sensitive(item.is_some());
        clear(&self.details);
        self.details.append(&match item {
            Some(i) => self.details_card(i).upcast::<gtk::Widget>(),
            None => w::empty_state(
                "account_tree",
                "Pick anything in the scheme",
                "Choose a planet kind or a commodity to light up everything it needs and everything it goes into. Pick it again, or Clear, to see everything.",
            )
            .upcast(),
        });
    }

    /// A clickable item: in the columns (where picking the selected item again clears the selection), and as links
    /// in the details card.
    fn link(self: &Rc<Self>, item: Item, text: &str, icon_size: i32) -> gtk::Button {
        self.item_button(item, text, icon_size, false)
    }

    fn item_button(self: &Rc<Self>, item: Item, text: &str, icon_size: i32, toggles: bool) -> gtk::Button {
        let row = w::hbox(6);
        row.append(&item.icon(icon_size));
        let l = w::line(text, &[]);
        l.set_hexpand(true);
        row.append(&l);
        let b = gtk::Button::new();
        b.set_child(Some(&row));
        b.add_css_class("pi-scheme-item");
        b.set_tooltip_text(Some(&item.name()));
        let weak: Weak<Scheme> = Rc::downgrade(self);
        b.connect_clicked(move |_| {
            // after the click handler: picking a link in the details card replaces that card
            let weak = weak.clone();
            gtk::glib::idle_add_local_once(move || {
                if let Some(s) = weak.upgrade() {
                    let again = s.selected.get() == Some(item);
                    s.select(if again && toggles { None } else { Some(item) });
                }
            });
        });
        b
    }

    fn links(self: &Rc<Self>, items: impl IntoIterator<Item = (Item, String)>) -> gtk::FlowBox {
        wrap(items.into_iter().map(|(i, t)| self.link(i, &t, 20).upcast()).collect())
    }

    fn details_card(self: &Rc<Self>, item: Item) -> gtk::Box {
        let d = &*PI;
        let v = w::vbox(16);
        let head = w::hbox(12);
        head.append(&item.icon(40));
        let titles = w::vbox(2);
        titles.set_hexpand(true);
        let name_row = w::hbox(8);
        name_row.append(&w::line(&item.name(), &["title-large"]));
        if let Item::Type(t) = item {
            if let Some(tier) = d.tier(t) {
                name_row.append(&tier_badge(tier));
            }
        }
        titles.append(&name_row);
        head.append(&titles);
        match item {
            Item::Kind(k) => {
                let kind = &d.kinds[k];
                titles.append(&w::line("Planet kind · 5 raw resources", &["body-medium", "dim"]));
                v.append(&head);
                v.append(&section("Raw resources", &self.links(kind.resources.iter().map(|r| (Item::Type(*r), d.name(*r))))));
                let alone = pl::makeable_on(&[kind.key]);
                for tier in 1..=4u8 {
                    let of: Vec<i64> = alone.iter().copied().filter(|t| d.tier(*t) == Some(tier)).collect();
                    if !of.is_empty() {
                        let title = format!("P{tier} it can make on its own ({})", of.len());
                        v.append(&section(&title, &self.links(of.into_iter().map(|t| (Item::Type(t), d.name(t))))));
                    }
                }
            }
            Item::Type(t) => {
                let tier = d.tier(t).unwrap_or(0);
                if let Some(c) = d.commodity(t) {
                    let sub = format!("{} m³ per unit · customs tax base {}", fmt::decimal(c.volume, 2), fmt::isk(c.tax_base));
                    titles.append(&w::line(&sub, &["body-medium", "dim"]));
                }
                if tier >= 1 {
                    let calc = w::button_icon("calculate", "Open in calculator", Btn::Tonal);
                    calc.set_valign(gtk::Align::Center);
                    let ctx = self.ctx.clone();
                    calc.connect_clicked(move |_| {
                        ctx.remember(PRODUCT_KEY, t.to_string());
                        ctx.remember(VIEW_KEY, "calculator");
                        let ctx = ctx.clone();
                        gtk::glib::idle_add_local_once(move || ctx.rebuild());
                    });
                    head.append(&calc);
                }
                v.append(&head);
                match d.schematic_for(t) {
                    Some(s) => {
                        let mut parts: Vec<gtk::Widget> = Vec::new();
                        for (n, (i, q)) in s.inputs.iter().enumerate() {
                            if n > 0 {
                                parts.push(w::line("+", &["dim"]).upcast());
                            }
                            parts.push(self.link(Item::Type(*i), &format!("{} × {}", fmt::int(*q), d.name(*i)), 20).upcast());
                        }
                        parts.push(w::icon("arrow_forward").upcast());
                        parts.push(self.link(Item::Type(s.output.0), &format!("{} × {}", fmt::int(s.output.1), d.name(s.output.0)), 20).upcast());
                        let sch = w::vbox(6);
                        sch.append(&wrap(parts));
                        let inputs: Vec<String> = s.inputs.iter().map(|(i, _)| format!("{} {}", rate_text(s.input_per_hour(*i)), d.name(*i))).collect();
                        let per_hour = format!(
                            "Every {} cycle. One factory makes {} an hour from {}.",
                            cycle_text(s.cycle),
                            rate_text(s.per_hour()),
                            inputs.join(" and ")
                        );
                        let l = w::text(&per_hour, &["body-medium", "dim"]);
                        sch.append(&l);
                        v.append(&section("Schematic", &sch));
                    }
                    None => {
                        let kinds: Vec<(Item, String)> =
                            d.kinds.iter().enumerate().filter(|(_, k)| k.resources.contains(&t)).map(|(i, k)| (Item::Kind(i), k.label.to_string())).collect();
                        v.append(&section(&format!("Extracted on {} planet kinds", kinds.len()), &self.links(kinds)));
                    }
                }
                let users: Vec<(Item, String)> = d.used_by(t).iter().map(|s| (Item::Type(s.output.0), d.name(s.output.0))).collect();
                if users.is_empty() {
                    v.append(&section("Used in", &w::text("Nothing: this is the top of the chain.", &["body-medium", "dim"])));
                } else {
                    v.append(&section(&format!("Used in ({})", users.len()), &self.links(users)));
                }
                if tier >= 1 {
                    let raw = d.raw_inputs(t);
                    v.append(&section(&format!("Raw resources ({})", raw.len()), &self.links(raw.into_iter().map(|r| (Item::Type(r), d.name(r))))));
                    let body: gtk::Widget = match pl::planets_for(t) {
                        Some(keys) => {
                            let tags: Vec<gtk::Widget> = keys.iter().filter_map(|k| d.kinds.iter().position(|x| x.key == *k)).map(|i| self.link(Item::Kind(i), d.kinds[i].label, 20).upcast()).collect();
                            wrap(tags).upcast()
                        }
                        None => w::text("Some of its resources aren't on any planet.", &["body-medium", "dim"]).upcast(),
                    };
                    v.append(&section("Smallest set of planet kinds with all of them", &body));
                }
            }
        }
        w::card(&v)
    }
}

fn scheme_view(ctx: &Ctx) -> gtk::Box {
    let d = &*PI;
    let v = w::vbox(16);
    let head = w::hbox(12);
    let hint = w::text(
        "Every planet kind and commodity from raw resources to P4. Pick one to highlight what it needs and what it goes into.",
        &["body-medium", "dim"],
    );
    hint.set_hexpand(true);
    head.append(&hint);
    let clear_btn = w::button_icon("close", "Clear", Btn::Outlined);
    clear_btn.set_valign(gtk::Align::Center);
    head.append(&clear_btn);
    v.append(&head);

    let scheme = Rc::new(Scheme { ctx: ctx.clone(), items: RefCell::default(), details: w::vbox(0), clear: clear_btn.clone(), selected: Cell::new(None) });
    {
        let weak = Rc::downgrade(&scheme);
        clear_btn.connect_clicked(move |_| {
            if let Some(s) = weak.upgrade() {
                s.select(None);
            }
        });
    }

    let grid = w::hbox(8);
    let column = |title: &gtk::Widget, sub: &str, width: i32, items: Vec<Item>| {
        let col = w::vbox(2);
        col.set_width_request(width);
        col.set_valign(gtk::Align::Start);
        let h = w::vbox(2);
        h.add_css_class("pi-scheme-head");
        h.append(title);
        h.append(&w::line(sub, &["body-small", "dim"]));
        col.append(&h);
        for i in items {
            let b = scheme.item_button(i, &i.name(), 20, true);
            col.append(&b);
            scheme.items.borrow_mut().push((i, b));
        }
        col
    };
    let planets_title = w::line("Planets", &["title-small"]);
    grid.append(&column(planets_title.upcast_ref(), &format!("{} kinds", d.kinds.len()), 120, (0..d.kinds.len()).map(Item::Kind).collect()));
    for tier in 0..=4u8 {
        let t = w::hbox(6);
        t.append(&tier_badge(tier));
        t.append(&w::line(["Raw", "Basic", "Refined", "Specialized", "Advanced"][tier as usize], &["title-small"]));
        let sub = if tier == 0 {
            "Extracted".to_string()
        } else {
            // cycle times straight from the schematics making this tier
            let mut cycles: Vec<i64> = d.schematics.values().filter(|s| d.tier(s.output.0) == Some(tier)).map(|s| s.cycle).collect();
            cycles.sort();
            cycles.dedup();
            let list: Vec<String> = cycles.iter().map(|c| format!("{} s", fmt::int(*c))).collect();
            format!("{} cycles", list.join(", "))
        };
        let items: Vec<Item> = d.of_tier(tier).into_iter().map(|c| Item::Type(c.id)).collect();
        grid.append(&column(t.upcast_ref(), &format!("{sub} · {}", items.len()), 176, items));
    }
    let (sw, holder) = w::async_holder();
    sw.set_vexpand(false);
    holder.append(&grid);
    v.append(&w::card(&sw));
    v.append(&scheme.details);

    let initial = ctx.recall(SELECTED_KEY).and_then(|s| Item::parse(&s));
    scheme.select(initial);
    keep_alive(&v, scheme);
    v
}

// ---------- calculator ----------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Units,
    Factories,
}

/// One type in the chain for the chosen amount.
#[derive(Clone)]
struct Step {
    type_id: i64,
    tier: Tier,
    units: f64,
    per_hour: f64,
    factories: f64,
    cycle: Option<i64>,
    volume: f64,
}

struct Calc {
    ctx: Ctx,
    product: Cell<Option<i64>>,
    tier: Cell<Option<Tier>>,
    mode: Cell<Mode>,
    prices: RefCell<Option<Prices>>,
    products: gtk::FlowBox,
    buttons: RefCell<Vec<(i64, gtk::Button)>>,
    search: gtk::SearchEntry,
    qty: gtk::Entry,
    factories: gtk::Entry,
    hours: gtk::Entry,
    results: gtk::Box,
    /// Where the hub value goes once prices arrive.
    value: RefCell<Option<gtk::Box>>,
    /// The chain for the current product and amount: (amount, steps).
    steps: RefCell<(f64, Vec<Step>)>,
}

fn calculator_view(ctx: &Ctx) -> gtk::Box {
    let v = w::vbox(16);
    let recall_f = |k: &str, d: f64| ctx.recall(k).and_then(|s| parse_amount(&s)).filter(|n| *n > 0.0).unwrap_or(d);
    let product = ctx.recall(PRODUCT_KEY).and_then(|s| s.parse::<i64>().ok()).filter(|t| PI.schematic_for(*t).is_some());
    let tier = ctx.recall(TIER_KEY).and_then(|s| s.parse::<Tier>().ok()).filter(|t| (1..=4).contains(t));
    let mode = if ctx.recall(MODE_KEY).as_deref() == Some("factories") { Mode::Factories } else { Mode::Units };
    let search = w::search_entry("Search products…");
    search.set_text(&ctx.recall(SEARCH_KEY).unwrap_or_default());
    let products = gtk::FlowBox::new();
    products.set_selection_mode(gtk::SelectionMode::None);
    products.set_column_spacing(6);
    products.set_row_spacing(6);
    products.set_max_children_per_line(8);
    products.set_min_children_per_line(1);
    products.set_homogeneous(true);
    products.set_valign(gtk::Align::Start);
    let calc = Rc::new(Calc {
        ctx: ctx.clone(),
        product: Cell::new(product),
        tier: Cell::new(tier),
        mode: Cell::new(mode),
        prices: RefCell::new(None),
        products: products.clone(),
        buttons: RefCell::default(),
        search: search.clone(),
        qty: number_entry(&rate_text(recall_f(QTY_KEY, 1000.0)), 9),
        factories: number_entry(&rate_text(recall_f(FACTORIES_KEY, 10.0)), 6),
        hours: number_entry(&rate_text(recall_f(HOURS_KEY, 24.0)), 6),
        results: w::vbox(16),
        value: RefCell::new(None),
        steps: RefCell::new((0.0, Vec::new())),
    });

    // picking a product: tier chips and a search over a grid of buttons
    let pick = w::vbox(12);
    let chips = w::hbox(8);
    let mut first: Option<gtk::ToggleButton> = None;
    for (i, label) in ["All", "P1", "P2", "P3", "P4"].iter().enumerate() {
        let t: Option<Tier> = if i == 0 { None } else { Some(i as Tier) };
        let count = PI.commodities.values().filter(|c| c.tier >= 1 && t.is_none_or(|t| c.tier == t)).count();
        let chip = w::chip(&format!("{label} ({count})"), t == tier);
        match &first {
            Some(f) => chip.set_group(Some(f)),
            None => first = Some(chip.clone()),
        }
        let weak = Rc::downgrade(&calc);
        chip.connect_toggled(move |c| {
            if let (true, Some(calc)) = (c.is_active(), weak.upgrade()) {
                calc.tier.set(t);
                calc.ctx.remember(TIER_KEY, t.map(|t| t.to_string()).unwrap_or_default());
                calc.fill_products();
            }
        });
        chips.append(&chip);
    }
    pick.append(&wrap(vec![chips.upcast(), search.clone().upcast()]));
    {
        let weak = Rc::downgrade(&calc);
        search.connect_search_changed(move |s| {
            if let Some(calc) = weak.upgrade() {
                calc.ctx.remember(SEARCH_KEY, s.text().as_str());
                calc.fill_products();
            }
        });
    }
    let grid_scroll = gtk::ScrolledWindow::new();
    grid_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    grid_scroll.set_max_content_height(232);
    grid_scroll.set_propagate_natural_height(true);
    grid_scroll.set_child(Some(&products));
    pick.append(&grid_scroll);

    // the amount: units, or factories running for some hours
    let amount = w::hbox(16);
    {
        let weak = Rc::downgrade(&calc);
        let seg = w::segmented(&["Units", "Factories"], (mode == Mode::Factories) as usize, move |i| {
            if let Some(calc) = weak.upgrade() {
                let m = if i == 1 { Mode::Factories } else { Mode::Units };
                calc.mode.set(m);
                calc.ctx.remember(MODE_KEY, if m == Mode::Factories { "factories" } else { "units" });
                calc.show_mode_fields();
                calc.render();
            }
        });
        seg.set_valign(gtk::Align::End);
        amount.append(&seg);
    }
    let qty_field = labelled("Units to make", &calc.qty);
    let factories_field = labelled("Factories of it", &calc.factories);
    let hours_field = labelled("Running for (hours)", &calc.hours);
    amount.append(&fields(vec![qty_field.upcast(), factories_field.upcast(), hours_field.upcast()]));
    for (e, key) in [(&calc.qty, QTY_KEY), (&calc.factories, FACTORIES_KEY), (&calc.hours, HOURS_KEY)] {
        let weak = Rc::downgrade(&calc);
        e.connect_changed(move |e| {
            if let Some(calc) = weak.upgrade() {
                calc.ctx.remember(key, e.text().as_str());
                calc.render();
            }
        });
    }
    pick.append(&amount);
    v.append(&w::card(&pick));
    v.append(&calc.results);

    calc.fill_products();
    calc.show_mode_fields();
    calc.render();
    {
        let weak = Rc::downgrade(&calc);
        common::load_prices(ctx, Vec::new(), false, move |p| {
            if let Some(calc) = weak.upgrade() {
                *calc.prices.borrow_mut() = Some(p);
                calc.render_value();
            }
        });
    }
    keep_alive(&v, calc);
    v
}

impl Calc {
    fn fill_products(self: &Rc<Self>) {
        while let Some(c) = self.products.first_child() {
            self.products.remove(&c);
        }
        self.buttons.borrow_mut().clear();
        let q = self.search.text().to_lowercase();
        let mut list: Vec<(Tier, String, i64)> = PI
            .commodities
            .values()
            .filter(|c| c.tier >= 1 && self.tier.get().is_none_or(|t| c.tier == t))
            .filter(|c| q.trim().is_empty() || c.name.to_lowercase().contains(q.trim()))
            .map(|c| (c.tier, c.name.clone(), c.id))
            .collect();
        list.sort();
        if list.is_empty() {
            self.products.insert(&w::text("No product matches.", &["body-medium", "dim"]), -1);
        }
        for (tier, name, id) in list {
            let row = w::hbox(8);
            row.append(&w::eve_image(&icon_url(id), 24));
            let l = w::line(&name, &[]);
            l.set_hexpand(true);
            row.append(&l);
            row.append(&tier_badge(tier));
            let b = gtk::Button::new();
            b.set_child(Some(&row));
            b.add_css_class("pi-product");
            b.set_tooltip_text(Some(&name));
            b.set_width_request(200);
            if Some(id) == self.product.get() {
                b.add_css_class("pi-selected");
            }
            let weak = Rc::downgrade(self);
            b.connect_clicked(move |_| {
                if let Some(calc) = weak.upgrade() {
                    calc.product.set(Some(id));
                    calc.ctx.remember(PRODUCT_KEY, id.to_string());
                    for (t, b) in calc.buttons.borrow().iter() {
                        if *t == id {
                            b.add_css_class("pi-selected");
                        } else {
                            b.remove_css_class("pi-selected");
                        }
                    }
                    calc.render();
                }
            });
            self.products.insert(&b, -1);
            self.buttons.borrow_mut().push((id, b));
        }
    }

    fn show_mode_fields(&self) {
        let factories = self.mode.get() == Mode::Factories;
        // the labelled boxes sit inside FlowBox children
        for (e, show) in [(&self.qty, !factories), (&self.factories, factories), (&self.hours, factories)] {
            if let Some(child) = e.parent().and_then(|p| p.parent()) {
                child.set_visible(show);
            }
        }
    }

    /// (units of the product, its units per hour) for the inputs; None when they don't parse.
    fn amount(&self, product: i64) -> Option<(f64, f64)> {
        let per_factory = PI.schematic_for(product)?.per_hour();
        let num = |e: &gtk::Entry| parse_amount(&e.text()).filter(|n| *n > 0.0);
        match self.mode.get() {
            // rates as for one factory of the product
            Mode::Units => Some((num(&self.qty)?, per_factory)),
            Mode::Factories => {
                let rate = num(&self.factories)? * per_factory;
                Some((rate * num(&self.hours)?, rate))
            }
        }
    }

    fn render(self: &Rc<Self>) {
        clear(&self.results);
        *self.value.borrow_mut() = None;
        let Some(product) = self.product.get() else {
            self.results.append(&w::empty_state(
                "account_tree",
                "Pick a product",
                "Choose any P1 to P4 above to see its whole chain from raw resources, the factories and planets it takes, and what it's worth.",
            ));
            return;
        };
        let Some((amount, rate)) = self.amount(product) else {
            self.results.append(&w::banner("info", "Enter an amount above zero.", w::BannerKind::Info));
            return;
        };
        let d = &*PI;
        let hours = amount / rate;
        let steps: Vec<Step> = pl::chain(product, rate)
            .into_iter()
            .map(|i| Step {
                type_id: i.type_id,
                tier: i.tier,
                units: i.per_hour * hours,
                per_hour: i.per_hour,
                factories: i.factories,
                cycle: i.schematic.and_then(|s| d.schematics.get(&s)).map(|s| s.cycle),
                volume: i.per_hour * hours * d.commodity(i.type_id).map(|c| c.volume).unwrap_or(0.0),
            })
            .collect();
        *self.steps.borrow_mut() = (amount, steps.clone());
        let tier = d.tier(product).unwrap_or(1);
        let minimal = pl::planets_for(product).unwrap_or_default();

        // header and actions
        let head = w::hbox(12);
        head.append(&w::eve_image(&icon_url(product), 48));
        let titles = w::vbox(2);
        titles.set_hexpand(true);
        let name_row = w::hbox(8);
        name_row.append(&w::line(&d.name(product), &["title-large"]));
        name_row.append(&tier_badge(tier));
        titles.append(&name_row);
        let n_factories = rate / d.schematic_for(product).map(|s| s.per_hour()).unwrap_or(1.0);
        let sub = match self.mode.get() {
            Mode::Units => format!("{} units · {} with one factory running; rates below are for that factory", units_text(amount), fmt::duration(hours_dur(hours), 2)),
            Mode::Factories => format!("{} factories for {} make {} units", rate_text(n_factories), fmt::duration(hours_dur(hours), 2), units_text(amount)),
        };
        let sub = w::text(&sub, &["body-medium", "dim"]);
        titles.append(&sub);
        head.append(&titles);
        self.results.append(&head);

        let total_factories: f64 = steps.iter().filter(|s| s.cycle.is_some()).map(|s| (s.factories - 1e-6).ceil()).sum();
        let raw_volume: f64 = steps.iter().filter(|s| s.tier == 0).map(|s| s.volume).sum();
        let out_volume = steps.iter().find(|s| s.type_id == product).map(|s| s.volume).unwrap_or(0.0);
        let raw_per_hour: f64 = steps.iter().filter(|s| s.tier == 0).map(|s| s.per_hour).sum();
        self.results.append(&w::tile_row(vec![
            w::stat_tile("Output", &units_text(amount), Some(&format!("{} an hour", rate_text(rate)))),
            w::stat_tile("Factories in the chain", &fmt::int(total_factories as i64), Some("each step rounded up")),
            w::stat_tile("Raw resources", &format!("{}/h", rate_text(raw_per_hour)), Some(&format!("{} kinds", steps.iter().filter(|s| s.tier == 0).count()))),
            w::stat_tile("Planet kinds", &minimal.len().to_string(), Some(&minimal.iter().map(|k| kind_label(k)).collect::<Vec<_>>().join(", "))),
            w::stat_tile("Volume", &format!("{} m³", fmt::int(out_volume.ceil() as i64)), Some(&format!("{} m³ of raw resources", fmt::int(raw_volume.ceil() as i64)))),
        ]));

        // recipe tree
        let tree = w::vbox(2);
        tree_rows(&pl::tree(product, 1), amount, 0, &tree);
        self.results.append(&w::card_with("Recipe", Some("Units for the whole amount; expand a step to see what it's made of"), &[], &tree));

        // every step
        let table = Table::new(steps.clone())
            .widget_sorted("Commodity", |s: &Step| commodity_cell(s.type_id), |a, b| PI.name(a.type_id).cmp(&PI.name(b.type_id)))
            .expand()
            .num("Tier", |s| format!("P{}", s.tier), |s| s.tier as f64)
            .num("Units", |s| units_text(s.units), |s| s.units)
            .num("Per hour", |s| rate_text(s.per_hour), |s| s.per_hour)
            .num("Factories", |s| if s.cycle.is_some() { fmt::decimal(s.factories, 2) } else { "—".into() }, |s| s.factories)
            .num("Cycle", |s| s.cycle.map(cycle_text).unwrap_or_else(|| "extracted".into()), |s| s.cycle.unwrap_or(0) as f64)
            .num("Volume", |s| format!("{} m³", fmt::decimal(s.volume, 1)), |s| s.volume)
            .sort_by(1, true)
            .natural_height()
            .build();
        self.results.append(&w::card_with("Every step", Some("Summed over the whole chain"), &[], &table));

        // raw resources and planets
        let raw: Vec<Step> = steps.iter().filter(|s| s.tier == 0).cloned().collect();
        let raw_box = w::vbox(8);
        for s in &raw {
            let row = w::hbox(12);
            let cell = commodity_cell(s.type_id);
            cell.set_width_request(200);
            row.append(&cell);
            let q = w::line(&format!("{} · {}/h", units_text(s.units), rate_text(s.per_hour)), &["numeric", "dim"]);
            q.set_width_request(150);
            q.set_xalign(1.0);
            row.append(&q);
            let tags: Vec<gtk::Widget> = d.kinds_with(s.type_id).iter().map(|k| kind_tag(k.key, minimal.contains(&k.key)).upcast()).collect();
            let tags = wrap(tags);
            tags.set_hexpand(true);
            row.append(&tags);
            raw_box.append(&row);
        }
        let set_row = w::hbox(8);
        set_row.append(&w::line("Smallest set of planet kinds:", &["body-medium"]));
        for k in &minimal {
            set_row.append(&kind_tag(k, true));
        }
        set_row.set_margin_top(8);
        raw_box.append(&set_row);
        self.results.append(&w::card_with("Raw resources and planets", Some("Highlighted kinds together have every resource"), &[], &raw_box));

        // value at hub prices
        let value = w::vbox(12);
        let shop_tiers: Vec<Tier> = (0..tier).filter(|t| steps.iter().any(|s| s.tier == *t)).collect();
        let shop_labels: Vec<String> = shop_tiers.iter().map(|t| if *t == 0 { "P0 raw resources".to_string() } else { format!("P{t} inputs") }).collect();
        let shop_labels: Vec<&str> = shop_labels.iter().map(|s| s.as_str()).collect();
        let shop_now = self.ctx.recall(SHOP_TIER_KEY).and_then(|s| s.parse::<Tier>().ok()).and_then(|t| shop_tiers.iter().position(|x| *x == t)).unwrap_or(shop_tiers.len().saturating_sub(1));
        let shop = w::dropdown(&shop_labels, shop_now as u32);
        {
            let ctx = self.ctx.clone();
            let tiers = shop_tiers.clone();
            shop.connect_selected_notify(move |dd| {
                if let Some(t) = tiers.get(dd.selected() as usize) {
                    ctx.remember(SHOP_TIER_KEY, t.to_string());
                }
            });
        }
        let copy = w::button_icon("content_copy", "Copy shopping list", Btn::Tonal);
        {
            let weak = Rc::downgrade(self);
            let (shop, tiers) = (shop.clone(), shop_tiers.clone());
            copy.connect_clicked(move |_| {
                let Some(calc) = weak.upgrade() else { return };
                let Some(t) = tiers.get(shop.selected() as usize).copied() else { return };
                let text = shopping_list(&calc.steps.borrow().1, t);
                w::copy_to_clipboard(&calc.ctx, &text);
            });
        }
        let actions: Vec<gtk::Widget> = vec![shop.upcast(), copy.upcast()];
        let hub = common::hub(&common::settings()).0;
        let card = w::card_with(&format!("Value at {hub}"), Some("Buying the inputs at one tier against selling the output, before customs taxes and market fees"), &actions, &value);
        self.results.append(&card);
        *self.value.borrow_mut() = Some(value);
        self.render_value();
    }

    fn render_value(&self) {
        let Some(holder) = self.value.borrow().clone() else { return };
        let Some(product) = self.product.get() else { return };
        clear(&holder);
        let prices = self.prices.borrow();
        let Some(prices) = prices.as_ref() else {
            holder.append(&w::text("Loading prices…", &["body-medium", "dim"]));
            return;
        };
        let (sell, buy) = (prices.sell(), prices.buy());
        let steps = self.steps.borrow();
        let (amount, steps) = (&steps.0, &steps.1);
        let tier = PI.tier(product).unwrap_or(1);
        let out_sell = sell.get(&product).map(|p| p * amount);
        let out_buy = buy.get(&product).map(|p| p * amount);
        let money = |v: Option<f64>| v.map(fmt::isk_short).unwrap_or_else(|| "no orders".into());
        holder.append(&w::tile_row(vec![
            w::stat_tile("Output at sell orders", &money(out_sell), sell.get(&product).map(|p| format!("{} each", fmt::isk(*p))).as_deref()),
            w::stat_tile("Output sold instantly", &money(out_buy), buy.get(&product).map(|p| format!("{} each", fmt::isk(*p))).as_deref()),
        ]));

        #[derive(Clone)]
        struct Row {
            tier: Tier,
            cost: f64,
            missing: usize,
            out_sell: Option<f64>,
            out_buy: Option<f64>,
        }
        let rows: Vec<Row> = (0..tier)
            .filter(|t| steps.iter().any(|s| s.tier == *t))
            .map(|t| {
                let of: Vec<&Step> = steps.iter().filter(|s| s.tier == t).collect();
                let cost = of.iter().filter_map(|s| sell.get(&s.type_id).map(|p| p * s.units)).sum();
                let missing = of.iter().filter(|s| !sell.contains_key(&s.type_id)).count();
                Row { tier: t, cost, missing, out_sell, out_buy }
            })
            .collect();
        let profit = |out: Option<f64>, r: &Row| out.map(|o| o - r.cost);
        // of what the output sells for at sell orders
        let margin = move |r: &Row| r.out_sell.filter(|o| *o > 0.0).and_then(|o| profit(Some(o), r).map(|p| p / o));
        let profit_cell = move |v: Option<f64>| -> gtk::Widget {
            let l = w::line(&v.map(fmt::isk_short).unwrap_or_else(|| "—".into()), &["numeric"]);
            l.set_xalign(1.0);
            if let Some(v) = v {
                l.add_css_class(if v >= 0.0 { "success-text" } else { "error-text" });
            }
            l.upcast()
        };
        let table = Table::new(rows)
            .text("Buy inputs at", |r: &Row| if r.tier == 0 { "P0: extract nothing, buy it all raw".into() } else { format!("P{}", r.tier) })
            .expand()
            .num("Cost", |r| if r.missing > 0 { format!("{} ({} unpriced)", fmt::isk_short(r.cost), r.missing) } else { fmt::isk_short(r.cost) }, |r| r.cost)
            .widget_sorted("Profit at sell orders", move |r| profit_cell(profit(r.out_sell, r)), move |a, b| profit(a.out_sell, a).unwrap_or(f64::MIN).total_cmp(&profit(b.out_sell, b).unwrap_or(f64::MIN)))
            .widget_sorted("Profit selling instantly", move |r| profit_cell(profit(r.out_buy, r)), move |a, b| profit(a.out_buy, a).unwrap_or(f64::MIN).total_cmp(&profit(b.out_buy, b).unwrap_or(f64::MIN)))
            .num("Margin", move |r| margin(r).map(|m| format!("{:.1}%", m * 100.0)).unwrap_or_else(|| "—".into()), move |r| margin(r).unwrap_or(f64::MIN))
            .natural_height()
            .empty_text("Nothing to buy for this product.")
            .build();
        holder.append(&table);
    }
}

/// Keeps `state` (whose handlers hold it weakly) for as long as `widget` exists.
fn keep_alive<T: 'static>(widget: &gtk::Box, state: Rc<T>) {
    let state = RefCell::new(Some(state));
    widget.connect_destroy(move |_| drop(state.take()));
}

fn hours_dur(hours: f64) -> chrono::Duration {
    chrono::Duration::seconds((hours * 3600.0).round().min(1e12) as i64)
}

/// One row per node of the recipe with its units for `units` of it; nodes with inputs fold open.
fn tree_rows(node: &pl::ChainNode, units: f64, depth: usize, out: &gtk::Box) {
    let d = &*PI;
    let row = w::hbox(8);
    if let Some(t) = d.tier(node.type_id) {
        row.append(&tier_badge(t));
    }
    row.append(&w::eve_image(&icon_url(node.type_id), 20));
    row.append(&w::line(&d.name(node.type_id), &[]));
    row.append(&w::line(&format!("× {}", units_text(units)), &["numeric", "pi-tree-qty"]));
    if node.children.is_empty() {
        // line up with the rows that have an expander arrow
        row.set_margin_start(22);
        out.append(&row);
        return;
    }
    let ex = gtk::Expander::new(None);
    ex.set_label_widget(Some(&row));
    ex.set_expanded(depth < 2);
    let kids = w::vbox(2);
    kids.set_margin_start(24);
    let cycles = d.schematic_for(node.type_id).map(|s| units / s.output.1.max(1) as f64).unwrap_or(0.0);
    for c in &node.children {
        tree_rows(c, cycles * c.quantity as f64, depth + 1, &kids);
    }
    ex.set_child(Some(&kids));
    out.append(&ex);
}

/// Multibuy text for the chain's commodities of one tier: "Name\tqty" lines.
fn shopping_list(steps: &[Step], tier: Tier) -> String {
    let mut lines: Vec<(String, i64)> = steps.iter().filter(|s| s.tier == tier).map(|s| (PI.name(s.type_id), (s.units - 1e-6).ceil().max(0.0) as i64)).collect();
    lines.sort();
    lines.into_iter().map(|(n, q)| format!("{n}\t{q}")).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_links() {
        let by_name = |n: &str| PI.commodities.values().find(|c| c.name == n).unwrap().id;
        let coolant = by_name("Coolant");
        let rel = related(Item::Type(coolant));
        assert!(rel.contains(&Item::Type(by_name("Water"))));
        assert!(rel.contains(&Item::Type(by_name("Aqueous Liquids"))));
        assert!(!rel.contains(&Item::Type(by_name("Bacteria"))));
        // gas planets have both its raw resources
        let gas = PI.kinds.iter().position(|k| k.key == "gas").unwrap();
        assert!(rel.contains(&Item::Kind(gas)));
        let from_gas = related(Item::Kind(gas));
        assert!(from_gas.contains(&Item::Type(coolant)));
        assert_eq!(Item::parse(&Item::Kind(gas).key()), Some(Item::Kind(gas)));
        assert_eq!(Item::parse(&Item::Type(coolant).key()), Some(Item::Type(coolant)));
        assert_eq!(cycle_text(1800), "30 min");
        assert_eq!(units_text(40.0000000001), "40");
        let steps = vec![Step { type_id: coolant, tier: 2, units: 12.2, per_hour: 5.0, factories: 1.0, cycle: Some(3600), volume: 0.0 }];
        assert_eq!(shopping_list(&steps, 2), "Coolant\t13");
    }
}
