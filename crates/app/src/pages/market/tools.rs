//! Market → Tools: the margin calculator, the appraisal tool, and the trade settings the Profits tab uses (fees per
//! character, custom costs for items without a recorded purchase).

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::{set_child, type_cell};
use crate::widgets::{self as w, Table};
use gtk::prelude::*;
use podstack_core::features::trade::{self, Appraised, Fees};
use podstack_core::sde::{MARKET, TRADE_HUBS};
use podstack_core::{config, fmt};
use std::cell::RefCell;
use std::rc::Rc;

const CSS: &str = "
.mkt-tools-paste { font-family: monospace; padding: 12px; border-radius: 12px; background-color: @surface_container_highest; }
.mkt-tools-paste text { background-color: transparent; }
.mkt-tools-amounts label { padding: 4px 12px; }
.mkt-tools-amounts .head { font-weight: 500; }
";

const VIEWS: [&str; 3] = ["Margin calculator", "Appraisal", "Fees & costs"];
const VIEW_KEY: &str = "market.tools.view";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Tools));
    let view = ctx
        .recall(VIEW_KEY)
        .or_else(|| std::env::var("PODSTACK_TOOLS_VIEW").ok())
        .and_then(|v| VIEWS.iter().position(|l| l.eq_ignore_ascii_case(v.trim())))
        .unwrap_or(0);
    {
        let ctx = ctx.clone();
        body.append(&w::segmented(&VIEWS, view, move |i| {
            ctx.remember(VIEW_KEY, VIEWS[i]);
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        }));
    }
    match view {
        1 => {
            body.set_vexpand(true);
            body.append(&appraisal(ctx));
            Page::new("Market", body).static_page().no_scroll()
        }
        2 => {
            body.append(&fees_and_costs(ctx));
            Page::new("Market", body).static_page()
        }
        _ => {
            body.append(&margin_calculator(ctx));
            Page::new("Market", body).static_page()
        }
    }
}

/// "1,234.5", "1 234,5" is not supported (ambiguous); "12k", "1.5m", "2b" are.
fn parse_amount(s: &str) -> Option<f64> {
    let s = s.trim().to_lowercase().replace([',', ' ', '\u{a0}'], "");
    let (num, mult) = match s.chars().last()? {
        'k' => (&s[..s.len() - 1], 1e3),
        'm' => (&s[..s.len() - 1], 1e6),
        'b' => (&s[..s.len() - 1], 1e9),
        _ => (s.as_str(), 1.0),
    };
    num.parse::<f64>().ok().filter(|n| n.is_finite()).map(|n| n * mult)
}

fn percent_text(fraction: f64) -> String {
    let s = format!("{:.3}", fraction * 100.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn labelled(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = w::vbox(4);
    b.append(&w::line(label, &["label-medium", "dim"]));
    b.append(child);
    b
}

/// Form fields that wrap onto more lines in a narrow window.
fn fields(children: Vec<gtk::Widget>) -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_column_spacing(12);
    f.set_row_spacing(12);
    f.set_max_children_per_line(8);
    f.set_homogeneous(false);
    for c in children {
        c.set_valign(gtk::Align::End);
        f.insert(&c, -1);
    }
    f
}

fn entry(text: &str, suffix: Option<&str>) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_text(text);
    e.set_input_purpose(gtk::InputPurpose::Number);
    gtk::prelude::EditableExt::set_alignment(&e, 1.0);
    e.set_width_chars(12);
    if let Some(s) = suffix {
        e.set_secondary_icon_name(None);
        e.set_placeholder_text(Some(s));
    }
    e
}

// ---------------------------------------------------------------------------------------------------------------
// Margin calculator

fn margin_calculator(ctx: &Ctx) -> gtk::Box {
    let col = w::vbox(16);
    let chars = ctx.core().characters();
    let settings = trade::settings();
    let first_fees = chars.first().map(|c| trade::fees_for(&settings, c)).unwrap_or_default();

    let recall = |k: &str, d: String| ctx.recall(&format!("market.tools.margin.{k}")).unwrap_or(d);
    let buy = entry(&recall("buy", String::new()), None);
    let sell = entry(&recall("sell", String::new()), None);
    let buy_fee = entry(&recall("buy_fee", percent_text(first_fees.broker)), None);
    let sell_fee = entry(&recall("sell_fee", percent_text(first_fees.broker)), None);
    let tax = entry(&recall("tax", percent_text(first_fees.tax)), None);
    buy.set_placeholder_text(Some("ISK"));
    sell.set_placeholder_text(Some("ISK"));

    // fees from a character's skills (or its override in Fees & costs)
    let mut who: Vec<String> = chars.iter().map(|c| c.name()).collect();
    who.push("Custom".into());
    let who_refs: Vec<&str> = who.iter().map(|s| s.as_str()).collect();
    let who_dd = w::dropdown(&who_refs, ctx.recall("market.tools.margin.who").and_then(|v| v.parse().ok()).unwrap_or(0));
    {
        let (ctx, chars, buy_fee, sell_fee, tax) = (ctx.clone(), chars.clone(), buy_fee.clone(), sell_fee.clone(), tax.clone());
        who_dd.connect_selected_notify(move |dd| {
            let i = dd.selected() as usize;
            ctx.remember("market.tools.margin.who", i.to_string());
            if let Some(c) = chars.get(i) {
                let f = trade::fees_for(&trade::settings(), c);
                buy_fee.set_text(&percent_text(f.broker));
                sell_fee.set_text(&percent_text(f.broker));
                tax.set_text(&percent_text(f.tax));
            }
        });
    }

    // prices from an item's best orders at a hub
    let item = gtk::SearchEntry::new();
    item.set_placeholder_text(Some("Item name, e.g. PLEX"));
    item.set_width_chars(28);
    let hub_names: Vec<&str> = TRADE_HUBS.iter().map(|h| h.0).collect();
    let hub_dd = w::dropdown(&hub_names, 0);
    let match_label = w::line("Fill in prices from an item's best buy and sell order at a trade hub.", &["body-small", "dim"]);
    let load = w::button_icon("download", "Use market prices", w::Btn::Tonal);
    {
        let match_label = match_label.clone();
        item.connect_search_changed(move |e| {
            let hits = MARKET.search(&e.text(), 1);
            match hits.first() {
                Some(t) => match_label.set_text(&format!("→ {}", t.name)),
                None if e.text().is_empty() => match_label.set_text("Fill in prices from an item's best buy and sell order at a trade hub."),
                None => match_label.set_text("No market item matches."),
            }
        });
    }

    let results = w::vbox(16);
    let recompute: Rc<dyn Fn()> = {
        let (ctx, buy, sell, buy_fee, sell_fee, tax, results) = (ctx.clone(), buy.clone(), sell.clone(), buy_fee.clone(), sell_fee.clone(), tax.clone(), results.clone());
        Rc::new(move || {
            for (k, e) in [("buy", &buy), ("sell", &sell), ("buy_fee", &buy_fee), ("sell_fee", &sell_fee), ("tax", &tax)] {
                ctx.remember(&format!("market.tools.margin.{k}"), e.text().as_str());
            }
            let p = |e: &gtk::Entry| parse_amount(&e.text()).unwrap_or(0.0);
            let m = trade::margin(p(&buy), p(&sell), p(&buy_fee) / 100.0, p(&sell_fee) / 100.0, p(&tax) / 100.0);
            set_child(&results, &margin_results(&m));
        })
    };
    for e in [&buy, &sell, &buy_fee, &sell_fee, &tax] {
        let r = recompute.clone();
        e.connect_changed(move |_| r());
    }
    {
        let (ctx, query, hub_dd, buy, sell, match_label) = (ctx.clone(), item.clone(), hub_dd.clone(), buy.clone(), sell.clone(), match_label.clone());
        let go = move || {
            let Some(t) = MARKET.search(&query.text(), 1).first().map(|t| (t.id, t.name.clone())) else { return };
            let hub = TRADE_HUBS[hub_dd.selected() as usize];
            match_label.set_text(&format!("Loading {} at {}…", t.1, hub.0));
            let core = ctx.core().clone();
            let (buy, sell, match_label) = (buy.clone(), sell.clone(), match_label.clone());
            crate::rt::run(
                async move { core.market.best_prices(&core.esi, hub.3, Some(hub.1), &[t.0], false).await },
                move |prices| match prices.get(&t.0) {
                    Some(p) => {
                        // a trader buys with a buy order just above the best buy and sells just below the best sell
                        if let Some(b) = p.buy {
                            buy.set_text(&format!("{:.2}", trade::price_above(b)));
                        }
                        if let Some(s) = p.sell {
                            sell.set_text(&format!("{:.2}", trade::price_below(s)));
                        }
                        match_label.set_text(&format!(
                            "{} at {}: best buy {}, best sell {} (prices set one tick better).",
                            t.1,
                            hub.0,
                            p.buy.map(|b| fmt::decimal(b, 2)).unwrap_or_else(|| "none".into()),
                            p.sell.map(|s| fmt::decimal(s, 2)).unwrap_or_else(|| "none".into())
                        ));
                    }
                    None => match_label.set_text("Couldn't load the market for that item."),
                },
            );
        };
        let go = Rc::new(go);
        let g = go.clone();
        load.connect_clicked(move |_| g());
        item.connect_activate(move |_| go());
    }

    let form = w::vbox(16);
    form.add_css_class("mkt-tools-form");
    load.set_valign(gtk::Align::End);
    let lookup = fields(vec![labelled("Item", &item).upcast(), labelled("Hub", &hub_dd).upcast(), load.upcast()]);
    form.append(&lookup);
    form.append(&match_label);
    form.append(&fields(vec![
        labelled("Buy price (ISK)", &buy).upcast(),
        labelled("Sell price (ISK)", &sell).upcast(),
        labelled("Fees of", &who_dd).upcast(),
        labelled("Buy broker fee (%)", &buy_fee).upcast(),
        labelled("Sell broker fee (%)", &sell_fee).upcast(),
        labelled("Sales tax (%)", &tax).upcast(),
    ]));
    col.append(&w::card_with("Margin calculator", Some("Buying with a buy order and selling with a sell order, after fees."), &[], &form));
    col.append(&results);
    recompute();
    col
}

fn signed_class(n: f64) -> &'static str {
    if n > 0.0 {
        "success-text"
    } else if n < 0.0 {
        "error-text"
    } else {
        "dim"
    }
}

fn margin_results(m: &trade::Margin) -> gtk::Box {
    let v = w::vbox(16);
    let pct = |x: Option<f64>| x.map(|x| format!("{:.2}%", x * 100.0)).unwrap_or_else(|| "—".into());
    let profit_tile = w::stat_tile("Profit per unit", &format!("{} ISK", fmt::decimal(m.profit, 2)), Some(&format!("Margin {} · return {}", pct(m.margin), pct(m.roi))));
    if m.profit > 0.0 {
        profit_tile.add_css_class("accent");
    }
    v.append(&w::tile_row(vec![
        profit_tile,
        w::stat_tile("Total buy", &format!("{} ISK", fmt::decimal(m.total_buy, 2)), Some(&format!("Broker fee {}", fmt::decimal(m.buy_broker, 2)))),
        w::stat_tile(
            "Total sell",
            &format!("{} ISK", fmt::decimal(m.total_sell, 2)),
            Some(&format!("Broker fee {} · tax {}", fmt::decimal(m.sell_broker, 2), fmt::decimal(m.tax, 2))),
        ),
    ]));
    // the same trade at larger volumes
    let grid = gtk::Grid::new();
    grid.add_css_class("mkt-tools-amounts");
    grid.set_column_spacing(24);
    for (i, h) in ["Amount", "Cost", "Profit", "Margin"].iter().enumerate() {
        let l = w::line(h, &["head", "label-large"]);
        l.set_xalign(if i == 0 { 0.0 } else { 1.0 });
        grid.attach(&l, i as i32, 0, 1, 1);
    }
    for (row, n) in (0..8).map(|i| 10f64.powi(i)).enumerate() {
        let cells = [
            (fmt::int(n as i64), "body-medium"),
            (format!("{} ISK", fmt::isk_short(m.total_buy * n)), "dim"),
            (format!("{} ISK", fmt::isk_short(m.profit * n)), signed_class(m.profit)),
            (pct(m.margin), "dim"),
        ];
        for (c, (t, class)) in cells.iter().enumerate() {
            let l = w::line(t, &[class]);
            l.set_xalign(if c == 0 { 0.0 } else { 1.0 });
            grid.attach(&l, c as i32, row as i32 + 1, 1, 1);
        }
    }
    v.append(&w::card_with("By amount", None, &[], &grid));
    v
}

// ---------------------------------------------------------------------------------------------------------------
// Appraisal

/// Where the appraisal prices come from: a hub station, or its whole region.
fn appraisal_markets() -> Vec<(String, i64, Option<i64>)> {
    let mut v: Vec<(String, i64, Option<i64>)> = TRADE_HUBS.iter().map(|h| (format!("{} (station)", h.0), h.3, Some(h.1))).collect();
    v.extend(TRADE_HUBS.iter().map(|h| (format!("{} (whole region)", podstack_core::sde::MAP.region_name(h.3).unwrap_or(h.0)), h.3, None)));
    v
}

fn appraisal(ctx: &Ctx) -> gtk::Box {
    let col = w::vbox(12);
    col.set_vexpand(true);

    let markets = appraisal_markets();
    let names: Vec<&str> = markets.iter().map(|m| m.0.as_str()).collect();
    let market_dd = w::dropdown(&names, ctx.recall("market.tools.appraisal.market").and_then(|v| v.parse().ok()).unwrap_or(0));
    let view = gtk::TextView::new();
    view.add_css_class("mkt-tools-paste");
    view.set_wrap_mode(gtk::WrapMode::None);
    view.buffer().set_text(&ctx.recall("market.tools.appraisal.text").unwrap_or_default());
    let scroll = w::scrolled(&view);
    scroll.set_min_content_height(150);
    scroll.set_max_content_height(150);
    scroll.set_hexpand(true);
    col.append(&scroll);
    let actions = w::hbox(8);
    let go = w::button_icon("calculate", "Appraise", w::Btn::Filled);
    let clear = w::button("Clear", w::Btn::Text);
    actions.append(&go);
    actions.append(&clear);
    let from = w::line("Prices from", &["body-medium", "dim"]);
    from.set_margin_start(8);
    actions.append(&from);
    market_dd.set_valign(gtk::Align::Center);
    actions.append(&market_dd);
    let hint = w::line("Paste a hangar, contract, cargo scan, EFT fit or a list like \"5 Hobgoblin II\".", &["body-small", "dim"]);
    hint.set_hexpand(true);
    hint.set_xalign(1.0);
    hint.set_valign(gtk::Align::Center);
    actions.append(&hint);
    col.append(&actions);

    let (scroller, right) = w::async_holder();
    right.append(&w::empty_state("receipt_long", "Nothing appraised yet", "Paste items above and press Appraise."));
    col.append(&scroller);

    {
        let (ctx, view) = (ctx.clone(), view.clone());
        view.buffer().connect_changed(move |b| {
            let (s, e) = b.bounds();
            ctx.remember("market.tools.appraisal.text", b.text(&s, &e, false).as_str());
        });
    }
    {
        let ctx = ctx.clone();
        market_dd.connect_selected_notify(move |dd| ctx.remember("market.tools.appraisal.market", dd.selected().to_string()));
    }
    {
        let view = view.clone();
        clear.connect_clicked(move |_| view.buffer().set_text(""));
    }
    let run: Rc<dyn Fn()> = {
        let (ctx, view, right, market_dd) = (ctx.clone(), view.clone(), right.clone(), market_dd.clone());
        Rc::new(move || {
            let (s, e) = view.buffer().bounds();
            let parsed = trade::parse_items(view.buffer().text(&s, &e, false).as_str());
            if parsed.items.is_empty() {
                set_child(&right, &w::empty_state("help", "No items found", "None of the lines named an item sold on the market."));
                return;
            }
            let (label, region, location) = markets[market_dd.selected() as usize].clone();
            let spinner = w::vbox(12);
            spinner.set_valign(gtk::Align::Center);
            spinner.set_vexpand(true);
            let sp = gtk::Spinner::new();
            sp.start();
            sp.set_size_request(32, 32);
            spinner.append(&sp);
            spinner.append(&w::text(&format!("Pricing {} items at {label}…", parsed.items.len()), &["dim"]));
            set_child(&right, &spinner);
            let core = ctx.core().clone();
            let ids: Vec<i64> = parsed.items.iter().map(|i| i.0).collect();
            let (ctx, right) = (ctx.clone(), right.clone());
            crate::rt::run(async move { core.market.best_prices(&core.esi, region, location, &ids, false).await }, move |prices| {
                let rows = trade::appraise(&parsed.items, &prices);
                let missing = parsed.items.len() - parsed.items.iter().filter(|i| prices.contains_key(&i.0)).count();
                set_child(&right, &appraisal_results(&ctx, rows, &parsed.unknown, missing, &label));
            });
        })
    };
    {
        let run = run.clone();
        go.connect_clicked(move |_| run());
    }
    if !ctx.recall("market.tools.appraisal.text").unwrap_or_default().trim().is_empty() {
        run();
    }
    col
}

fn appraisal_results(ctx: &Ctx, rows: Vec<Appraised>, unknown: &[String], missing: usize, market: &str) -> gtk::Box {
    let v = w::vbox(12);
    v.set_vexpand(true);
    let sell: f64 = rows.iter().map(|r| r.sell_total()).sum();
    let buy: f64 = rows.iter().map(|r| r.buy_total()).sum();
    let volume: f64 = rows.iter().map(|r| r.volume).sum();
    let units: i64 = rows.iter().map(|r| r.quantity).sum();
    let summary = format!("{market}: sell {} ISK · buy {} ISK · {} m³", fmt::isk(sell), fmt::isk(buy), fmt::decimal(volume, 2));
    let copy = w::button_icon("content_copy", "Copy summary", w::Btn::Text);
    {
        let (ctx, summary) = (ctx.clone(), summary.clone());
        copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &summary));
    }
    let sell_tile = w::stat_tile("Sell value", &format!("{} ISK", fmt::isk_short(sell)), Some("At the lowest sell orders"));
    sell_tile.add_css_class("accent");
    v.append(&w::tile_row(vec![
        sell_tile,
        w::stat_tile("Buy value", &format!("{} ISK", fmt::isk_short(buy)), Some("At the highest buy orders")),
        w::stat_tile("Volume", &format!("{} m³", fmt::decimal(volume, 1)), Some("Packaged")),
        w::stat_tile("Items", &fmt::int(units), Some(&format!("{} kinds", rows.len()))),
    ]));
    let head = w::hbox(8);
    let l = w::line(&format!("Prices at {market}"), &["body-small", "dim"]);
    l.set_hexpand(true);
    head.append(&l);
    head.append(&copy);
    v.append(&head);
    if !unknown.is_empty() {
        let shown: Vec<&str> = unknown.iter().take(5).map(|s| s.as_str()).collect();
        let more = if unknown.len() > 5 { format!(" and {} more", unknown.len() - 5) } else { String::new() };
        v.append(&w::banner("help", &format!("Not recognised: {}{more}", shown.join(", ")), w::BannerKind::Notice));
    }
    if missing > 0 {
        v.append(&w::banner("cloud_off", &format!("{missing} items couldn't be priced (the market didn't answer)."), w::BannerKind::Notice));
    }
    let price = |p: Option<f64>| p.map(|p| fmt::decimal(p, 2)).unwrap_or_else(|| "—".into());
    let table = Table::new(rows)
        .widget_sorted("Item", |r| type_cell(r.type_id, &r.name), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .width(260)
        .num("Quantity", |r| fmt::int(r.quantity), |r| r.quantity as f64)
        .num("Volume", |r| format!("{} m³", fmt::decimal(r.volume, 2)), |r| r.volume)
        .num("Sell each", move |r| price(r.sell), |r| r.sell.unwrap_or(0.0))
        .num("Sell total", |r| fmt::isk_short(r.sell_total()), |r| r.sell_total())
        .num("Buy each", move |r| price(r.buy), |r| r.buy.unwrap_or(0.0))
        .num("Buy total", |r| fmt::isk_short(r.buy_total()), |r| r.buy_total())
        .sort_by(4, true)
        .build();
    v.append(&table);
    v
}

// ---------------------------------------------------------------------------------------------------------------
// Fees & costs

fn fees_and_costs(ctx: &Ctx) -> gtk::Box {
    let col = w::vbox(16);
    let settings = Rc::new(RefCell::new(trade::settings()));
    let save: Rc<dyn Fn()> = {
        let (ctx, settings) = (ctx.clone(), settings.clone());
        Rc::new(move || trade::save_settings(ctx.core(), &settings.borrow()))
    };

    // fees per character
    let list = w::vbox(8);
    let chars = ctx.core().characters();
    if chars.is_empty() {
        list.append(&w::line("Add a character to set its fees.", &["dim"]));
    }
    for c in chars.iter() {
        let skills = Fees::of_character(c);
        let custom = settings.borrow().fees.get(&c.id).copied();
        let row = w::hbox(12);
        row.add_css_class("list-item");
        row.append(&w::avatar(&config::character_portrait(c.id, 64), 32));
        let texts = w::vbox(2);
        texts.set_hexpand(true);
        texts.append(&w::line(&c.name(), &["body-large"]));
        texts.append(&w::line(
            &format!("From skills: broker fee {}% · sales tax {}%", percent_text(skills.broker), percent_text(skills.tax)),
            &["body-small", "dim"],
        ));
        row.append(&texts);
        let current = custom.unwrap_or(skills);
        let broker = entry(&percent_text(current.broker), None);
        let tax = entry(&percent_text(current.tax), None);
        broker.set_width_chars(7);
        tax.set_width_chars(7);
        broker.set_sensitive(custom.is_some());
        tax.set_sensitive(custom.is_some());
        row.append(&labelled("Broker fee (%)", &broker));
        row.append(&labelled("Sales tax (%)", &tax));
        let switch = gtk::Switch::new();
        switch.set_active(custom.is_some());
        switch.set_valign(gtk::Align::Center);
        switch.set_tooltip_text(Some("Use your own rates (standings or a player structure lower the broker fee)"));
        row.append(&labelled("Custom", &switch));
        let id = c.id;
        let store: Rc<dyn Fn()> = {
            let (settings, save, broker, tax, switch) = (settings.clone(), save.clone(), broker.clone(), tax.clone(), switch.clone());
            Rc::new(move || {
                if switch.is_active() {
                    let (Some(b), Some(t)) = (parse_amount(&broker.text()), parse_amount(&tax.text())) else { return };
                    settings.borrow_mut().fees.insert(id, Fees { broker: b / 100.0, tax: t / 100.0 });
                } else {
                    settings.borrow_mut().fees.remove(&id);
                }
                save();
            })
        };
        {
            let (store, broker, tax) = (store.clone(), broker.clone(), tax.clone());
            switch.connect_active_notify(move |s| {
                broker.set_sensitive(s.is_active());
                tax.set_sensitive(s.is_active());
                if !s.is_active() {
                    broker.set_text(&percent_text(skills.broker));
                    tax.set_text(&percent_text(skills.tax));
                }
                store();
            });
        }
        for e in [&broker, &tax] {
            let store = store.clone();
            e.connect_changed(move |_| store());
        }
        list.append(&row);
    }
    col.append(&w::card_with(
        "Fees",
        Some("Used by the Profits tab and the margin calculator. Skills give the NPC station rates; set your own for standings or player structures."),
        &[],
        &list,
    ));

    // custom costs
    let costs = w::vbox(8);
    let rows = w::vbox(4);
    let fill: Rc<dyn Fn()> = {
        let (rows, settings, save) = (rows.clone(), settings.clone(), save.clone());
        Rc::new(move || {
            while let Some(c) = rows.first_child() {
                rows.remove(&c);
            }
            let mut entries: Vec<(i64, f64)> = settings.borrow().custom_costs.iter().map(|(k, v)| (*k, *v)).collect();
            entries.sort_by_key(|(id, _)| MARKET.get(*id).map(|t| t.name.clone()).unwrap_or_default());
            if entries.is_empty() {
                rows.append(&w::line("No custom costs. Items sold without a recorded purchase count as unknown cost.", &["dim"]));
            }
            for (id, cost) in entries {
                let r = w::hbox(12);
                r.add_css_class("list-item");
                let cell = type_cell(id, &MARKET.get(id).map(|t| t.name.clone()).unwrap_or_else(|| format!("Type #{id}")));
                cell.set_hexpand(true);
                r.append(&cell);
                r.append(&w::line(&format!("{} ISK each", fmt::decimal(cost, 2)), &[]));
                let remove = w::icon_button("delete", "Remove");
                let (settings, save, rows2, r2) = (settings.clone(), save.clone(), rows.clone(), r.clone());
                remove.connect_clicked(move |_| {
                    settings.borrow_mut().custom_costs.remove(&id);
                    save();
                    rows2.remove(&r2);
                });
                r.append(&remove);
                rows.append(&r);
            }
        })
    };
    fill();
    let add_row = w::hbox(12);
    let item = gtk::SearchEntry::new();
    item.set_placeholder_text(Some("Item"));
    item.set_hexpand(true);
    let cost = entry("", None);
    cost.set_placeholder_text(Some("ISK each"));
    let add = w::button_icon("add", "Add", w::Btn::Tonal);
    add_row.append(&item);
    add_row.append(&cost);
    add_row.append(&add);
    let hint = w::line("", &["body-small", "dim"]);
    {
        let hint = hint.clone();
        item.connect_search_changed(move |e| {
            hint.set_text(&MARKET.search(&e.text(), 1).first().map(|t| format!("→ {}", t.name)).unwrap_or_default());
        });
    }
    {
        let (settings, save, fill, item, cost, hint) = (settings.clone(), save.clone(), fill.clone(), item.clone(), cost.clone(), hint.clone());
        add.connect_clicked(move |_| {
            let Some(t) = MARKET.search(&item.text(), 1).first().map(|t| t.id) else {
                hint.set_text("Pick an item sold on the market.");
                return;
            };
            let Some(c) = parse_amount(&cost.text()).filter(|c| *c >= 0.0) else {
                hint.set_text("Enter the cost per unit in ISK (12k, 1.5m work too).");
                return;
            };
            settings.borrow_mut().custom_costs.insert(t, c);
            save();
            item.set_text("");
            cost.set_text("");
            hint.set_text("");
            fill();
        });
    }
    costs.append(&add_row);
    costs.append(&hint);
    costs.append(&rows);
    col.append(&w::card_with(
        "Custom item costs",
        Some("What an item cost you when there's no purchase to match a sale with: loot, things you built, contract buys."),
        &[],
        &costs,
    ));
    col
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts() {
        assert_eq!(parse_amount("1,234.5"), Some(1234.5));
        assert_eq!(parse_amount("12k"), Some(12_000.0));
        assert_eq!(parse_amount(" 1.5M "), Some(1_500_000.0));
        assert_eq!(parse_amount("x"), None);
        assert_eq!(percent_text(0.03375), "3.375");
        assert_eq!(percent_text(0.015), "1.5");
    }
}
