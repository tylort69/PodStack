//! SP Farming: characters trained for skill injectors, when their next injector is ready, and what the farms earn
//! per 30 days at market (or the player's own) prices.
//!
//! The page updates itself (it holds price inputs a rebuild would reset): the tiles, the farm list and the profit
//! table are rebuilt when farms or characters change; the price inputs stay.

use super::overview::fleet::{self, Updaters, CSS};
use crate::ctx::{CharTab, Ctx, Page};
use crate::rt;
use crate::widgets::{self as w, Btn};
use gtk::{glib, prelude::*};
use podstack_core::character::{DEFAULT_FARM_BASE_SP, INJECTOR_SP};
use podstack_core::features::farming::{self, Farm, MarketPrices, PriceSource, PricedItem, Prices, ProfitSettings, Subscription};
use podstack_core::{fmt, Character, Event};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let core = ctx.core().clone();
    let st = Rc::new(State {
        ctx: ctx.clone(),
        market: RefCell::new(None),
        error: Cell::new(false),
        updating: Cell::new(false),
        settings: RefCell::new(core.setting::<ProfitSettings>(farming::SETTINGS_KEY)),
        tiles: w::vbox(0),
        farms: w::vbox(0),
        profit: w::vbox(0),
        price_labels: RefCell::new(None),
        pending: Cell::new(false),
        quiet: Cell::new(false),
    });

    let body = w::page_body();
    body.append(&st.tiles);
    body.append(&st.farms);
    body.append(&profit_card(&st));

    let add = w::button_icon("note_add", "Add / update farm", Btn::Filled);
    {
        let ctx = ctx.clone();
        add.connect_clicked(move |_| farm_dialog(&ctx, None));
    }

    st.refresh();
    {
        let st = st.clone();
        ctx.on_event(&body, move |e| match e {
            Event::CharacterUpdated(_) | Event::CharactersChanged => st.refresh_soon(),
            Event::Doc(d) if d == farming::DOC => st.refresh_soon(),
            _ => {}
        });
    }
    load_prices(&st, false);

    Page::new("SP Farming", body).static_page().action(add)
}

struct State {
    ctx: Ctx,
    /// None until the first load finishes.
    market: RefCell<Option<MarketPrices>>,
    error: Cell<bool>,
    updating: Cell<bool>,
    settings: RefCell<ProfitSettings>,
    tiles: gtk::Box,
    farms: gtk::Box,
    profit: gtk::Box,
    price_labels: RefCell<Option<PriceLabels>>,
    pending: Cell<bool>,
    /// Set while the page changes its own dropdowns, so their handlers ignore it.
    quiet: Cell<bool>,
}

/// The price card's labels that follow the prices.
struct PriceLabels {
    plex_source: gtk::DropDown,
    item_sources: Vec<(PricedItem, gtk::DropDown)>,
    plex_used: gtk::Label,
    omega_used: gtk::Label,
    mct_used: gtk::Label,
    item_used: Vec<(PricedItem, gtk::Label)>,
    fee_used: gtk::Label,
    custom_plex: gtk::Box,
    custom_items: Vec<(PricedItem, gtk::Box)>,
    date: gtk::Label,
    update: gtk::Button,
}

/// The farms whose character is still in PodStack, with that character.
fn farm_rows(core: &podstack_core::Core) -> Vec<(Farm, Arc<Character>)> {
    farming::farms().into_iter().filter_map(|f| core.character(f.id).map(|c| (f, c))).collect()
}

impl State {
    fn prices(&self) -> Option<Prices> {
        self.market.borrow().as_ref().map(|m| farming::resolve_prices(m, &self.settings.borrow()))
    }

    fn refresh_soon(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        let st = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            st.pending.set(false);
            st.refresh();
        });
    }

    fn refresh(self: &Rc<Self>) {
        let rows = farm_rows(self.ctx.core());
        fill(&self.tiles, &tiles(self, &rows));
        fill(&self.farms, &farms_card(&self.ctx, &rows));
        self.refresh_profit();
    }

    /// The price labels and the profit table (after a price or setting change).
    fn refresh_profit(self: &Rc<Self>) {
        let rows = farm_rows(self.ctx.core());
        self.update_price_labels();
        fill(&self.profit, &profit_body(self, &rows));
    }

    fn save_settings(self: &Rc<Self>) {
        self.ctx.core().set_setting(farming::SETTINGS_KEY, &*self.settings.borrow());
        let rows = farm_rows(self.ctx.core());
        fill(&self.tiles, &tiles(self, &rows));
        self.refresh_profit();
    }

    fn update_price_labels(&self) {
        let labels = self.price_labels.borrow();
        let Some(l) = labels.as_ref() else { return };
        let market = *self.market.borrow();
        let market_text = |v: Option<f64>| match &market {
            None => "loading…".to_string(),
            Some(_) => v.map(|v| format!("{} ISK", isk(Some(v)))).unwrap_or_else(|| "no sellers".into()),
        };
        self.quiet.set(true);
        set_first_item(&l.plex_source, &format!("New Eden market ({})", market_text(market.and_then(|m| m.plex))));
        for (item, dd) in &l.item_sources {
            set_first_item(dd, &format!("Jita ({})", market_text(market.and_then(|m| m.get(*item)))));
        }
        self.quiet.set(false);
        let s = self.settings.borrow();
        let p = self.prices().unwrap_or_default();
        l.plex_used.set_text(&isk(p.plex));
        l.omega_used.set_text(&isk(p.omega));
        l.mct_used.set_text(&isk(p.mct));
        for (item, lab) in &l.item_used {
            lab.set_text(&isk(p.get(*item)));
        }
        l.fee_used.set_text(&format!("{} per injector", isk(p.injector.map(|i| i * s.fees_percent / 100.0))));
        l.custom_plex.set_visible(s.plex.source == PriceSource::Custom);
        for (item, b) in &l.custom_items {
            b.set_visible(s.item(*item).source == PriceSource::Custom);
        }
        let updating = self.updating.get();
        l.update.set_sensitive(!updating);
        set_button_label(&l.update, if updating { "Updating…" } else { "Update prices" });
        match farming::market_price_date(&self.ctx.core().market) {
            Some(d) if !updating => {
                l.date.set_text(&format!("Prices from {} ago", fmt::time_since(d)));
                l.date.set_visible(true);
            }
            _ => l.date.set_visible(false),
        }
    }
}

fn fill(slot: &gtk::Box, child: &impl IsA<gtk::Widget>) {
    while let Some(c) = slot.first_child() {
        slot.remove(&c);
    }
    slot.append(child);
}

fn set_first_item(dd: &gtk::DropDown, text: &str) {
    if let Some(list) = dd.model().and_downcast::<gtk::StringList>() {
        if list.string(0).map(|s| s != text).unwrap_or(true) {
            let selected = dd.selected();
            list.splice(0, 1, &[text]);
            dd.set_selected(selected);
        }
    }
}

fn set_button_label(b: &gtk::Button, text: &str) {
    // button_icon's child is a box: icon, label
    if let Some(l) = b.child().and_then(|c| c.last_child()).and_downcast::<gtk::Label>() {
        l.set_text(text);
    }
}

/// Fetches market prices (cached 6h unless `force`), then updates the page.
fn load_prices(st: &Rc<State>, force: bool) {
    st.updating.set(true);
    st.update_price_labels();
    let core = st.ctx.core().clone();
    let st = st.clone();
    rt::run(async move { farming::market_prices(&core, force).await }, move |m| {
        st.updating.set(false);
        if m.is_empty() {
            // the player's own prices still work without the market
            st.error.set(true);
            if st.market.borrow().is_none() {
                *st.market.borrow_mut() = Some(m);
            }
        } else {
            st.error.set(false);
            *st.market.borrow_mut() = Some(m);
        }
        let rows = farm_rows(st.ctx.core());
        fill(&st.tiles, &tiles(&st, &rows));
        st.refresh_profit();
    });
}

/// Compact ISK, "—" when unknown.
fn isk(v: Option<f64>) -> String {
    match v {
        Some(v) if v.is_finite() => fmt::isk_short(v),
        _ => "—".into(),
    }
}

fn signed_isk(v: f64) -> String {
    format!("{}{}", if v < 0.0 { "−" } else { "+" }, fmt::isk_short(v.abs()))
}

// ---------- tiles ----------

fn tiles(st: &State, rows: &[(Farm, Arc<Character>)]) -> gtk::FlowBox {
    let injectors: i64 = rows.iter().map(|(f, c)| c.injectors_ready(f.base_sp)).sum();
    let sp_per_hour: i64 = rows.iter().map(|(_, c)| c.sp_per_hour()).sum();
    let idle = rows.iter().filter(|(_, c)| !c.is_training()).count();
    let farms: Vec<Farm> = rows.iter().map(|(f, _)| f.clone()).collect();
    let s = st.settings.borrow();
    let profit = st.prices().filter(|p| p.is_complete(farming::needs_mct(&farms))).and_then(|p| {
        rows.iter().try_fold(0.0, |sum, (f, c)| farming::for_farm(c.sp_per_hour(), c.injectors_ready(f.base_sp), f.subscription, &p, &s).map(|r| sum + r.profit))
    });
    let loss = profit.map(|p| p < 0.0).unwrap_or(false);
    w::tile_row(vec![
        fleet::tile("opacity", "Farms", &rows.len().to_string(), None, false),
        fleet::tile("vaccines", "Injectors ready", &fmt::int(injectors), Some("500,000 SP each"), false),
        fleet::tile(
            "speed",
            "Combined rate",
            &format!("{} SP/h", fmt::int(sp_per_hour)),
            Some(&format!("{} injectors / 30 days", fmt::decimal(sp_per_hour as f64 * 24.0 * 30.0 / INJECTOR_SP as f64, 1))),
            false,
        ),
        fleet::tile("warning", "Idle farms", &idle.to_string(), Some(if idle > 0 { "Queues need attention" } else { "All farms training" }), idle > 0),
        fleet::tile(
            "savings",
            if loss { "Loss / 30 days" } else { "Profit / 30 days" },
            &profit.map(|p| format!("{} ISK", fmt::isk_short(p))).unwrap_or_else(|| "—".into()),
            Some("After fees, extractors and subscriptions"),
            loss,
        ),
    ])
}

// ---------- farms ----------

fn farms_card(ctx: &Ctx, rows: &[(Farm, Arc<Character>)]) -> gtk::Widget {
    if rows.is_empty() {
        let e = w::empty_state("opacity", "No farms yet", "Use \"Add / update farm\" to track a character you train for skill injectors.");
        e.set_vexpand(false);
        return fleet::list_card("Farms", None, &[], &e).upcast();
    }
    let updaters = Updaters::default();
    let list = fleet::list();
    list.append(&fleet::head_row(&[("", 56), ("Capsuleer", -180), ("Skill points", 160), ("Next injector", 220), ("Training", 200), ("", 88)]));
    for (farm, c) in rows {
        list.append(&farm_row(ctx, farm, c, &updaters));
    }
    fleet::activate_rows(ctx, &list, 1, rows.iter().map(|(f, _)| f.id).collect(), CharTab::Summary);
    let card = fleet::list_card("Farms", Some("Everything above a farm's base SP can be extracted. Select a farm to open its character."), &[], &list);
    updaters.tick(&card, 1);
    card.upcast()
}

fn farm_row(ctx: &Ctx, farm: &Farm, c: &Character, updaters: &Updaters) -> gtk::Widget {
    let core = ctx.core().clone();
    let (id, base) = (c.id, farm.base_sp);
    let row = w::hbox(16);
    row.add_css_class("fleet-row");
    row.append(&fleet::portrait(&core, c, 48));
    row.append(&fleet::name_block(c, false));

    let sp = fleet::figure(&fmt::int(c.total_sp()), &format!("Base {}", fmt::int(base)), 160);
    let (sp_big, _) = fleet::figure_labels(&sp);
    row.append(&sp);

    // next injector: ready count, countdown and progress
    let next = w::vbox(4);
    next.set_size_request(220, -1);
    next.set_valign(gtk::Align::Center);
    let top = w::hbox(8);
    let ready = w::line("", &["fleet-figure", "numeric"]);
    ready.set_hexpand(true);
    top.append(&ready);
    let countdown = w::line("", &["body-small", "dim", "numeric"]);
    top.append(&countdown);
    next.append(&top);
    let bar = w::progress(0.0);
    bar.add_css_class("tertiary");
    next.append(&bar);
    row.append(&next);

    let training = w::vbox(2);
    training.set_size_request(200, -1);
    training.set_valign(gtk::Align::Center);
    let rate = w::line("", &["fleet-figure", "numeric"]);
    let ends = w::line("", &["body-small", "dim"]);
    if c.is_training() {
        training.append(&rate);
        training.append(&ends);
    } else {
        training.append(&fleet::not_training());
    }
    row.append(&training);

    let update = move || {
        let Some(c) = core.character(id) else { return };
        let now = chrono::Utc::now();
        let total = c.total_sp();
        sp_big.set_text(&fmt::int(total));
        let n = c.injectors_ready(base);
        ready.set_text(&format!("{n} ready"));
        if n > 0 {
            ready.add_css_class("tertiary-text");
        } else {
            ready.remove_css_class("tertiary-text");
        }
        countdown.set_text(&match (c.is_training(), c.next_injector_date(base)) {
            (true, Some(d)) => fmt::duration(d - now, 2),
            _ => "—".into(),
        });
        bar.set_fraction(farming::injector_progress(total, base));
        rate.set_text(&format!("{} SP/h", fmt::int(c.sp_per_hour())));
        if let Some(e) = c.queue_end() {
            ends.set_text(&format!("Queue ends in {}", fmt::duration(e - now, 3)));
        }
    };
    update();
    updaters.add(update);

    let actions = w::hbox(0);
    actions.set_size_request(88, -1);
    actions.set_halign(gtk::Align::End);
    let edit = w::icon_button("edit", "Change base SP");
    {
        let ctx = ctx.clone();
        edit.connect_clicked(move |_| farm_dialog(&ctx, Some(id)));
    }
    let del = w::icon_button("delete", "Remove farm");
    {
        let ctx = ctx.clone();
        let name = c.name();
        del.connect_clicked(move |_| {
            let core = ctx.core().clone();
            w::confirm(
                &ctx,
                "Remove farm?",
                &format!("Stop tracking {name} as an SP farm? The character stays in PodStack."),
                "Remove",
                true,
                move || farming::remove(&core, id),
            );
        });
    }
    actions.append(&edit);
    actions.append(&del);
    row.append(&actions);
    row.upcast()
}

/// "Add character as SP farm": pick a character and its base SP (what stays after extracting, at least 5,000,000).
fn farm_dialog(ctx: &Ctx, preselect: Option<i64>) {
    let core = ctx.core().clone();
    let mut chars = core.characters();
    if chars.is_empty() {
        ctx.toast("Add a character first");
        return;
    }
    chars.sort_by_key(|c| std::cmp::Reverse(c.total_sp()));
    let names: Vec<String> = chars.iter().map(|c| c.name()).collect();
    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let selected = preselect.and_then(|id| chars.iter().position(|c| c.id == id)).unwrap_or(0);
    let ids: Vec<i64> = chars.iter().map(|c| c.id).collect();

    let (win, content, actions) = w::dialog(ctx, "Add character as SP farm", 420);
    content.append(&w::text(
        "Everything a farm trains above its base SP can be extracted into skill injectors (500,000 SP each). \
         Adding a character that's already a farm changes its base SP.",
        &["body-medium", "dim"],
    ));
    content.append(&w::text("Character", &["label-large"]));
    let dd = w::dropdown(&name_refs, selected as u32);
    dd.set_hexpand(true);
    content.append(&dd);
    content.append(&w::text("Base character SP", &["label-large"]));
    let spin = gtk::SpinButton::with_range(DEFAULT_FARM_BASE_SP as f64, 1e10, INJECTOR_SP as f64);
    spin.set_digits(0);
    let base_for = |i: usize| farming::get(ids[i]).map(|f| f.base_sp).unwrap_or(DEFAULT_FARM_BASE_SP) as f64;
    spin.set_value(base_for(selected));
    content.append(&spin);
    content.append(&w::text("Base SP below 5,000,000 is raised to it: a character can't extract below that.", &["body-small", "dim"]));
    {
        let spin = spin.clone();
        let ids = ids.clone();
        dd.connect_selected_notify(move |dd| {
            let i = dd.selected() as usize;
            if i < ids.len() {
                spin.set_value(farming::get(ids[i]).map(|f| f.base_sp).unwrap_or(DEFAULT_FARM_BASE_SP) as f64);
            }
        });
    }

    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button("Save", Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let win = win.clone();
        let ctx = ctx.clone();
        save.connect_clicked(move |_| {
            spin.update();
            let i = dd.selected() as usize;
            let Some(&id) = ids.get(i) else {
                ctx.toast("Pick a character");
                return;
            };
            farming::add_or_update(ctx.core(), id, spin.value().round() as i64);
            win.close();
        });
    }
    actions.append(&cancel);
    actions.append(&save);
    win.present();
}

// ---------- profitability ----------

fn profit_card(st: &Rc<State>) -> gtk::Widget {
    let date = gtk::Label::new(None);
    date.add_css_class("body-small");
    date.add_css_class("dim");
    date.set_valign(gtk::Align::Center);
    let update = w::button_icon("sync", "Update prices", Btn::Tonal);
    update.set_tooltip_text(Some("Fetch the latest market prices now"));
    {
        let st = st.clone();
        update.connect_clicked(move |_| load_prices(&st, true));
    }
    let head_actions = w::hbox(12);
    head_actions.append(&date);
    head_actions.append(&update);

    let body = w::vbox(8);
    body.append(&prices_grid(st, date, update));
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("fleet-rule");
    body.append(&rule);
    st.profit.set_margin_bottom(4);
    body.append(&st.profit);

    let card = fleet::list_card("Profitability", Some("Per 30 days at each farm's current training speed"), &[head_actions.upcast()], &body);
    // keeps "Prices from … ago" current
    let st2 = st.clone();
    w::ticker(&card, 60, move || st2.update_price_labels());
    card.upcast()
}

fn fmt_input(v: Option<f64>) -> String {
    match v {
        Some(v) if v.fract() == 0.0 => format!("{v:.0}"),
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// A non-negative number, `None` when empty or not a number.
fn parse_input(t: &str) -> Option<f64> {
    t.trim().replace([',', '_', ' '], "").parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0)
}

/// A label that keeps its full width (grids scroll sideways instead of cutting their text).
fn plain(t: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A number entry with a unit after it; `on_change` gets the parsed value.
fn input(value: Option<f64>, placeholder: &str, unit: &str, on_change: impl Fn(Option<f64>) + 'static) -> gtk::Box {
    let b = w::hbox(8);
    let e = gtk::Entry::new();
    e.set_text(&fmt_input(value));
    e.set_placeholder_text(Some(placeholder));
    e.set_input_purpose(gtk::InputPurpose::Number);
    e.set_width_chars(10);
    e.set_max_width_chars(12);
    e.connect_changed(move |e| on_change(parse_input(&e.text())));
    b.append(&e);
    let u = plain(unit, &["body-medium", "dim"]);
    u.set_valign(gtk::Align::Center);
    b.append(&u);
    b
}

fn grid_label(t: &str, note: Option<&str>) -> gtk::Box {
    let b = w::vbox(0);
    b.set_valign(gtk::Align::Center);
    b.append(&plain(t, &["body-medium"]));
    if let Some(n) = note {
        b.append(&plain(n, &["body-small", "dim"]));
    }
    b
}

fn right(t: &str, classes: &[&str]) -> gtk::Label {
    let l = plain(t, classes);
    l.add_css_class("numeric");
    l.set_xalign(1.0);
    l.set_hexpand(true);
    l
}

fn prices_grid(st: &Rc<State>, date: gtk::Label, update: gtk::Button) -> gtk::Widget {
    let g = gtk::Grid::new();
    g.add_css_class("fleet-grid");
    g.set_column_spacing(24);
    g.set_row_spacing(10);
    for (i, h) in ["Price", "Source", "Your price", "ISK used"].iter().enumerate() {
        let l = plain(h, &["fleet-grid-head"]);
        if i == 3 {
            l.set_xalign(1.0);
        }
        g.attach(&l, i as i32, 0, 1, 1);
    }
    let s = st.settings.borrow().clone();
    let mut row = 1;

    // PLEX: market or own price in ISK
    let plex_source = w::dropdown(&["New Eden market (loading…)", "My price"], (s.plex.source == PriceSource::Custom) as u32);
    let custom_plex = {
        let st = st.clone();
        input(s.plex.isk, "ISK", "ISK each", move |v| {
            st.settings.borrow_mut().plex.isk = v;
            st.save_settings();
        })
    };
    {
        let st = st.clone();
        let custom = custom_plex.clone();
        plex_source.connect_selected_notify(move |dd| {
            if st.quiet.get() {
                return;
            }
            let source = if dd.selected() == 1 { PriceSource::Custom } else { PriceSource::Market };
            let market = st.market.borrow().unwrap_or_default();
            st.settings.borrow_mut().set_plex_source(source, &market);
            let v = st.settings.borrow().plex.isk;
            set_entry(&custom, v);
            st.save_settings();
        });
    }
    let plex_used = right("—", &["body-medium"]);
    g.attach(&grid_label("PLEX", None), 0, row, 1, 1);
    g.attach(&plex_source, 1, row, 1, 1);
    g.attach(&custom_plex, 2, row, 1, 1);
    g.attach(&plex_used, 3, row, 1, 1);
    row += 1;

    // Omega and MCT: New Eden Store prices in PLEX
    let omega = {
        let st = st.clone();
        input(Some(s.omega_plex), "PLEX", "PLEX", move |v| {
            st.settings.borrow_mut().omega_plex = v.unwrap_or(0.0);
            st.save_settings();
        })
    };
    let omega_used = right("—", &["body-medium"]);
    g.attach(&grid_label("Omega (30 days)", None), 0, row, 1, 1);
    g.attach(&plain("New Eden Store", &["body-medium", "dim"]), 1, row, 1, 1);
    g.attach(&omega, 2, row, 1, 1);
    g.attach(&omega_used, 3, row, 1, 1);
    row += 1;

    let mct = {
        let st = st.clone();
        input(s.mct_plex, "store price", "PLEX", move |v| {
            st.settings.borrow_mut().mct_plex = v;
            st.save_settings();
        })
    };
    let mct_used = right("—", &["body-medium"]);
    g.attach(&grid_label("MCT (30 days)", Some("training on another character")), 0, row, 1, 1);
    g.attach(&plain("New Eden Store", &["body-medium", "dim"]), 1, row, 1, 1);
    g.attach(&mct, 2, row, 1, 1);
    g.attach(&mct_used, 3, row, 1, 1);
    row += 1;

    // injectors and extractors: Jita or own price in PLEX
    let mut item_sources = vec![];
    let mut item_used = vec![];
    let mut custom_items = vec![];
    for item in PricedItem::ALL {
        let own = s.item(item).clone();
        let dd = w::dropdown(&["Jita (loading…)", "My price (PLEX)"], (own.source == PriceSource::Custom) as u32);
        let custom = {
            let st = st.clone();
            input(own.plex, "PLEX", "PLEX", move |v| {
                st.settings.borrow_mut().item_mut(item).plex = v;
                st.save_settings();
            })
        };
        {
            let st = st.clone();
            let custom = custom.clone();
            dd.connect_selected_notify(move |dd| {
                if st.quiet.get() {
                    return;
                }
                let source = if dd.selected() == 1 { PriceSource::Custom } else { PriceSource::Market };
                let market = st.market.borrow().unwrap_or_default();
                st.settings.borrow_mut().set_item_source(item, source, &market);
                let v = st.settings.borrow().item(item).plex;
                set_entry(&custom, v);
                st.save_settings();
            });
        }
        let used = right("—", &["body-medium"]);
        g.attach(&grid_label(item.label(), Some(item.note())), 0, row, 1, 1);
        g.attach(&dd, 1, row, 1, 1);
        g.attach(&custom, 2, row, 1, 1);
        g.attach(&used, 3, row, 1, 1);
        row += 1;
        item_sources.push((item, dd));
        item_used.push((item, used));
        custom_items.push((item, custom));
    }

    // sell fees
    let fees = {
        let st = st.clone();
        input(Some(s.fees_percent), "%", "%", move |v| {
            st.settings.borrow_mut().fees_percent = v.unwrap_or(0.0).min(50.0);
            st.save_settings();
        })
    };
    let fee_used = right("—", &["body-medium", "dim"]);
    g.attach(&grid_label("Sell fees", None), 0, row, 1, 1);
    g.attach(&plain("Sales tax + broker fee", &["body-medium", "dim"]), 1, row, 1, 1);
    g.attach(&fees, 2, row, 1, 1);
    g.attach(&fee_used, 3, row, 1, 1);

    *st.price_labels.borrow_mut() = Some(PriceLabels {
        plex_source,
        item_sources,
        plex_used,
        omega_used,
        mct_used,
        item_used,
        fee_used,
        custom_plex,
        custom_items,
        date,
        update,
    });
    g.upcast()
}

/// Sets an input's entry (after "my price" was seeded from the market) without it reporting a change.
fn set_entry(input: &gtk::Box, v: Option<f64>) {
    if let Some(e) = input.first_child().and_downcast::<gtk::Entry>() {
        if parse_input(&e.text()) != v {
            // the entry's own handler saves the same value again: harmless
            e.set_text(&fmt_input(v));
        }
    }
}

fn message(t: &str) -> gtk::Widget {
    let l = w::text(t, &["body-medium", "dim"]);
    l.set_margin_start(16);
    l.set_margin_end(16);
    l.set_margin_top(12);
    l.set_margin_bottom(12);
    l.upcast()
}

fn profit_body(st: &Rc<State>, rows: &[(Farm, Arc<Character>)]) -> gtk::Widget {
    let Some(prices) = st.prices() else { return message("Loading market prices…") };
    let farms: Vec<Farm> = rows.iter().map(|(f, _)| f.clone()).collect();
    let needs_mct = farming::needs_mct(&farms);
    if !prices.is_complete(needs_mct) {
        return message(if st.error.get() {
            "Couldn't load market prices. Set your own prices above, or use Update prices to try again."
        } else if !prices.is_complete(false) {
            "Some prices are missing. Enter your own for the ones marked \"—\"."
        } else {
            "A farm pays for MCT: enter the MCT price in PLEX above (from the New Eden Store)."
        });
    }
    if rows.is_empty() {
        return message("Add a farm to see its monthly profit.");
    }
    let s = st.settings.borrow().clone();
    let results: Vec<farming::FarmResult> =
        rows.iter().filter_map(|(f, c)| farming::for_farm(c.sp_per_hour(), c.injectors_ready(f.base_sp), f.subscription, &prices, &s)).collect();
    let total = results.iter().copied().fold(farming::FarmResult::default(), |a, b| a + b);

    let g = gtk::Grid::new();
    g.add_css_class("fleet-grid");
    g.set_column_spacing(24);
    g.set_row_spacing(10);
    let heads = ["Farm", "Pays for", "SP/min", "Injectors", "Sales", "Extractors", "Subscription", "Profit"];
    for (i, h) in heads.iter().enumerate() {
        let l = plain(h, &["fleet-grid-head"]);
        if i >= 2 {
            l.set_xalign(1.0);
        }
        g.attach(&l, i as i32, 0, 1, 1);
    }
    let profit_class = |v: f64| if v >= 0.0 { "success-text" } else { "error-text" };
    let numbers = |g: &gtk::Grid, row: i32, r: &farming::FarmResult, extra: &[&str]| {
        let cells = [
            (fmt::decimal(r.sp_per_minute, 1), None),
            (fmt::decimal(r.injectors_per_month, 2), None),
            (isk(Some(r.income)), None),
            (format!("−{}", isk(Some(r.extractors))), Some("dim")),
            (format!("−{}", isk(Some(r.subscription))), Some("dim")),
            (signed_isk(r.profit), Some(profit_class(r.profit))),
        ];
        for (i, (t, class)) in cells.iter().enumerate() {
            let mut classes = vec!["body-medium"];
            classes.extend_from_slice(extra);
            if let Some(c) = class {
                classes.push(c);
            }
            g.attach(&right(t, &classes), i as i32 + 2, row, 1, 1);
        }
    };
    for (i, ((farm, c), r)) in rows.iter().zip(&results).enumerate() {
        let row = i as i32 + 1;
        g.attach(&plain(&c.name(), &["body-medium"]), 0, row, 1, 1);
        let labels: Vec<&str> = Subscription::ALL.iter().map(|s| s.label()).collect();
        let current = Subscription::ALL.iter().position(|s| *s == farm.subscription).unwrap_or(0);
        let dd = w::dropdown(&labels, current as u32);
        let id = farm.id;
        let core = st.ctx.core().clone();
        dd.connect_selected_notify(move |dd| {
            if let Some(s) = Subscription::ALL.get(dd.selected() as usize) {
                farming::set_subscription(&core, id, *s);
            }
        });
        g.attach(&dd, 1, row, 1, 1);
        numbers(&g, row, r, &[]);
    }
    let total_row = rows.len() as i32 + 1;
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("fleet-rule");
    g.attach(&rule, 0, total_row, heads.len() as i32, 1);
    g.attach(&plain("Total", &["body-medium", "fleet-total"]), 0, total_row + 1, 2, 1);
    numbers(&g, total_row + 1, &total, &["fleet-total"]);

    let body = w::vbox(12);
    let sw = gtk::ScrolledWindow::new();
    sw.set_vscrollbar_policy(gtk::PolicyType::Never);
    sw.set_propagate_natural_height(true);
    sw.set_child(Some(&g));
    body.append(&sw);

    // the result and how it's worked out
    let result = w::hbox(24);
    result.add_css_class("fleet-result");
    result.add_css_class(if total.profit >= 0.0 { "good" } else { "bad" });
    result.set_margin_start(16);
    result.set_margin_end(16);
    result.set_margin_bottom(16);
    let left = w::vbox(2);
    left.append(&plain(if total.profit >= 0.0 { "Monthly profit" } else { "Monthly loss" }, &["label-medium"]));
    let value = w::hbox(8);
    value.append(&plain(&signed_isk(total.profit), &["fleet-result-value", "numeric"]));
    let unit = plain("ISK / 30 days", &["body-small"]);
    unit.set_valign(gtk::Align::End);
    unit.set_margin_bottom(5);
    value.append(&unit);
    left.append(&value);
    result.append(&left);
    let math = w::markup(
        &format!(
            "SP/min × {} min ÷ {} SP = injectors per 30 days. Each sells for {} less {}% fees, minus an extractor ({}): \
             <b>{}</b> per injector, before subscriptions. Injectors ready now are worth {}.",
            fmt::int(farming::MINUTES_PER_MONTH as i64),
            fmt::int(INJECTOR_SP),
            isk(prices.injector),
            fmt_input(Some(s.fees_percent)),
            isk(prices.extractor),
            isk(prices.profit_per_injector(&s)),
            isk(Some(total.ready_value)),
        ),
        &["body-small"],
    );
    math.set_hexpand(true);
    math.set_size_request(280, -1);
    math.set_valign(gtk::Align::Center);
    result.append(&math);
    body.append(&result);
    body.upcast()
}
