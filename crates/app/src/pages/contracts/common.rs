//! Shared by the Contracts tabs (and the character Contracts tab): the contracts table and its filter bar, the
//! contract dialog, the settings dialog and price loading.

use crate::ctx::Ctx;
use crate::pages::industry::form::{fields, labelled};
use crate::rt;
use crate::pages::market_orders::{label_cell, set_child, type_cell};
use crate::widgets::{self as w, Btn, Status, Table};
use chrono::{DateTime, Utc};
use gtk::prelude::*;
use podstack_core::domains::contracts::{Contract, ContractItem};
use podstack_core::features::comms::{self, Tone};
use podstack_core::features::contracts as ct;
use podstack_core::fmt;
use podstack_core::market::BestPrices;
use podstack_core::sde::TRADE_HUBS;
use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;

pub const CSS: &str = "
.comms-filter-bar { padding: 4px 0; }
.comms-section-title { margin-top: 4px; }
.comms-kv-key { min-width: 160px; }
.comms-isk-in { color: @success; font-weight: 500; }
.comms-isk-out { color: @error; font-weight: 500; }
";

/// Open/Completed switch, search and the contracts table.
pub(crate) fn contracts_section(ctx: &Ctx, key: &str, open: Vec<(i64, Contract)>, done: Vec<(i64, Contract)>) -> gtk::Box {
    let section = w::vbox(12);
    section.set_vexpand(true);
    let view_key = format!("{key}.view");
    let search_key = format!("{key}.search");
    let completed = ctx.recall(&view_key).as_deref() == Some("completed");
    let query = ctx.recall(&search_key).unwrap_or_default();
    let state = Rc::new(RefCell::new((completed, query.clone())));
    let (open, done) = (Rc::new(open), Rc::new(done));
    let holder = w::vbox(0);
    holder.set_vexpand(true);

    let refill = {
        let (ctx, holder, state, open, done) = (ctx.clone(), holder.clone(), state.clone(), open.clone(), done.clone());
        Rc::new(move || {
            let (completed, query) = state.borrow().clone();
            let q = query.trim().to_lowercase();
            let rows: Vec<(i64, Contract)> = if completed { done.as_ref() } else { open.as_ref() }
                .iter()
                .filter(|(_, c)| {
                    q.is_empty()
                        || [comms::title_case(&c.kind), comms::contract_details(c), comms::contract_from(c).to_string(), comms::contract_to(c).to_string()]
                            .iter()
                            .chain(c.items.iter().map(|i| &i.name))
                            .any(|t| t.to_lowercase().contains(&q))
                })
                .cloned()
                .collect();
            let empty = if !q.is_empty() { "Nothing matches." } else { "No contracts found." };
            set_child(&holder, &contracts_table(&ctx, rows, completed, empty));
        })
    };

    let bar = w::hbox(12);
    bar.add_css_class("comms-filter-bar");
    let open_label = format!("Open ({})", open.len());
    let done_label = format!("Completed ({})", done.len());
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        bar.append(&w::segmented(&[&open_label, &done_label], completed as usize, move |i| {
            ctx.remember(&view_key, if i == 1 { "completed" } else { "open" });
            state.borrow_mut().0 = i == 1;
            refill();
        }));
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let search = w::search_entry("Search type, details, parties or items");
    search.set_text(&query);
    search.set_hexpand(false);
    search.set_max_width_chars(40);
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        search.connect_search_changed(move |s| {
            ctx.remember(&search_key, s.text().as_str());
            state.borrow_mut().1 = s.text().to_string();
            refill();
        });
    }
    bar.append(&search);
    section.append(&bar);
    section.append(&holder);
    refill();
    section
}

fn status_kind(t: Tone) -> Status {
    match t {
        Tone::Good => Status::Good,
        Tone::Warn => Status::Warn,
        Tone::Bad => Status::Bad,
        Tone::Info => Status::Info,
        Tone::Neutral => Status::Neutral,
    }
}

fn date_cell(t: DateTime<Utc>) -> gtk::Widget {
    let l = w::line(&fmt::date(t), &["dim"]);
    l.set_tooltip_text(Some(&format!("{} ({})", fmt::datetime(t), fmt::relative(t))));
    l.upcast()
}

/// Contracts table; `complete` adds the completion date (and drops the time left).
pub(crate) fn contracts_table(ctx: &Ctx, mut rows: Vec<(i64, Contract)>, complete: bool, empty: &str) -> gtk::Widget {
    let now = Utc::now();
    // already in the default order, so the table's initial sort doesn't move rows (the view would follow the
    // first one down)
    if complete {
        rows.sort_by_key(|(_, c)| std::cmp::Reverse(c.date_completed.unwrap_or(c.date_issued)));
    } else {
        rows.sort_by_key(|(_, c)| std::cmp::Reverse(c.date_issued));
    }
    let mut t = Table::new(rows)
        .text("Type", |(_, c)| comms::title_case(&c.kind))
        .widget_sorted(
            "Status",
            |(_, c)| w::status(&comms::title_case(&c.status), status_kind(comms::contract_status_tone(&c.status))).upcast(),
            |a, b| a.1.status.cmp(&b.1.status),
        )
        .text("Details", |(_, c)| comms::contract_details(c))
        .expand()
        .text("From", |(_, c)| comms::contract_from(c).to_string())
        .text("To", |(_, c)| comms::contract_to(c).to_string())
        .num("ISK", |(_, c)| comms::contract_value(c).map(fmt::isk_short).unwrap_or_default(), |(_, c)| comms::contract_value(c).unwrap_or(0.0))
        .widget_sorted("Issued", |(_, c)| date_cell(c.date_issued), |a, b| a.1.date_issued.cmp(&b.1.date_issued));
    t = if complete {
        t.widget_sorted(
            "Completed",
            |(_, c)| date_cell(c.date_completed.unwrap_or(c.date_issued)),
            |a, b| a.1.date_completed.unwrap_or(a.1.date_issued).cmp(&b.1.date_completed.unwrap_or(b.1.date_issued)),
        )
        .sort_by(7, true)
    } else {
        t.widget_sorted(
            "Expires",
            move |(_, c)| {
                // an accepted courier runs against its days to complete, everything else against its expiry
                let due = match (c.status.as_str(), c.date_accepted, c.days_to_complete) {
                    ("in_progress", Some(acc), Some(days)) if days > 0 => acc + chrono::Duration::days(days),
                    _ => c.date_expired,
                };
                let l = if due > now { w::line(&fmt::duration(due - now, 2), &["dim"]) } else { w::line("Expired", &["error-text"]) };
                l.set_tooltip_text(Some(&fmt::datetime(due)));
                l.upcast()
            },
            |a, b| a.1.date_expired.cmp(&b.1.date_expired),
        )
        .sort_by(6, true)
    };
    let ctx2 = ctx.clone();
    t = t.widget("", move |(_, c)| {
        let b = w::icon_button("chevron_right", "Contract details");
        let (ctx, c) = (ctx2.clone(), c.clone());
        b.connect_clicked(move |_| open_contract(&ctx, &c));
        b.upcast()
    });
    let ctx = ctx.clone();
    t.on_activate(move |(_, c)| open_contract(&ctx, c)).empty_text(empty).build()
}

/// Key/value rows; a value can carry a style class (e.g. income/expense colors).
fn kv(rows: &[(&str, String, Option<&str>)]) -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_column_spacing(24);
    g.set_row_spacing(8);
    for (i, (k, v, class)) in rows.iter().enumerate() {
        let kl = w::line(k, &["body-medium", "dim", "comms-kv-key"]);
        kl.set_valign(gtk::Align::Start);
        g.attach(&kl, 0, i as i32, 1, 1);
        let vl = w::text(v, &["body-medium"]);
        if let Some(c) = class {
            vl.add_css_class(c);
        }
        vl.set_selectable(true);
        vl.set_hexpand(true);
        g.attach(&vl, 1, i as i32, 1, 1);
    }
    g
}

fn when(t: Option<DateTime<Utc>>, none: &str) -> String {
    match t {
        Some(t) => format!("{} ({})", fmt::datetime(t), fmt::relative(t)),
        None => none.to_string(),
    }
}

fn isk(v: Option<f64>) -> String {
    fmt::isk(v.unwrap_or(0.0))
}

fn items_list(items: &[&ContractItem]) -> gtk::Box {
    let list = w::vbox(6);
    for i in items {
        let row = w::hbox(12);
        let cell = type_cell(i.type_id, &i.name);
        cell.set_hexpand(true);
        row.append(&cell);
        if i.is_blueprint_copy() {
            row.append(&w::status("Copy", Status::Neutral));
        }
        row.append(&label_cell(&format!("× {}", fmt::int(i.quantity)), &["numeric", "dim"]));
        list.append(&row);
    }
    list
}

/// A filled card with a heading, for the dialog's sections.
fn block(title: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = w::vbox(10);
    b.add_css_class("card-filled");
    b.append(&w::text(title, &["title-small", "comms-section-title"]));
    b.append(child);
    b
}

/// The contract's details: parties, dates, the courier route or price, and its items.
pub(crate) fn open_contract(ctx: &Ctx, c: &Contract) {
    crate::theme::add_css(CSS);
    let title = if c.title.trim().is_empty() { format!("{} contract", comms::title_case(&c.kind)) } else { c.title.clone() };
    let (win, content, actions) = w::dialog(ctx, &title, 640);
    win.set_default_height(640);

    let inner = w::vbox(12);
    let chips = w::hbox(8);
    chips.append(&w::status(&comms::title_case(&c.kind), Status::Neutral));
    chips.append(&w::status(&comms::title_case(&c.status), status_kind(comms::contract_status_tone(&c.status))));
    chips.append(&w::status(&comms::title_case(&c.availability), Status::Neutral));
    if c.for_corporation {
        chips.append(&w::status("On behalf of corporation", Status::Neutral));
    }
    inner.append(&chips);

    let availability = match &c.assignee_name {
        Some(a) => format!("{} – {a}", comms::title_case(&c.availability)),
        None => comms::title_case(&c.availability),
    };
    inner.append(&block(
        "Overview",
        &kv(&[
            ("Description", if c.title.trim().is_empty() { "N/A".into() } else { c.title.clone() }, None),
            ("Issuer", c.issuer_name.clone(), None),
            ("Issuer corporation", c.issuer_corporation_name.clone(), None),
            ("On behalf of corp", if c.for_corporation { "Yes" } else { "No" }.into(), None),
            ("Availability", availability, None),
            ("Acceptor", c.acceptor_name.clone().unwrap_or_else(|| "Not accepted".into()), None),
        ]),
    ));
    inner.append(&block(
        "Dates",
        &kv(&[
            ("Issued", when(Some(c.date_issued), ""), None),
            ("Expires", when(Some(c.date_expired), ""), None),
            ("Accepted", when(c.date_accepted, "Not accepted"), None),
            ("Completed", when(c.date_completed, "Not completed"), None),
        ]),
    ));

    match c.kind.as_str() {
        "courier" => {
            let place = |p: &Option<podstack_core::universe::Place>| p.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Unknown".into());
            inner.append(&block(
                "Courier",
                &kv(&[
                    ("Pickup", place(&c.start_location), None),
                    ("Delivery", place(&c.end_location), None),
                    ("Volume", format!("{} m³", fmt::decimal(c.volume.unwrap_or(0.0), 2)), None),
                    ("Reward", isk(c.reward), Some("comms-isk-in")),
                    ("Collateral", isk(c.collateral), Some("comms-isk-out")),
                    ("Days to complete", c.days_to_complete.unwrap_or(0).to_string(), None),
                ]),
            ));
        }
        "item_exchange" => {
            let price = c.price.unwrap_or(0.0);
            let mut rows = vec![(
                if price >= 0.0 { "Buyer will pay" } else { "Buyer will get" },
                fmt::isk(price.abs()),
                Some(if price >= 0.0 { "comms-isk-out" } else { "comms-isk-in" }),
            )];
            if c.reward.unwrap_or(0.0) > 0.0 {
                rows.push(("Buyer will get", isk(c.reward), Some("comms-isk-in")));
            }
            if let Some(p) = &c.start_location {
                rows.push(("Location", p.name.clone(), None));
            }
            inner.append(&block("Price", &kv(&rows)));
        }
        "auction" => {
            let mut rows = vec![("Opening bid", isk(c.price), Some("comms-isk-out"))];
            if c.buyout.unwrap_or(0.0) > 0.0 {
                rows.push(("Buyout", isk(c.buyout), None));
            }
            if let Some(p) = &c.start_location {
                rows.push(("Location", p.name.clone(), None));
            }
            inner.append(&block("Auction", &kv(&rows)));
        }
        _ => {}
    }

    if c.has_items() {
        if !c.items_loaded {
            inner.append(&block("Items", &w::text("The items haven't loaded yet.", &["body-medium", "dim"])));
        } else if c.items.is_empty() {
            inner.append(&block("Items", &w::text("ESI no longer lists this contract's items.", &["body-medium", "dim"])));
        } else {
            let given: Vec<&ContractItem> = c.items.iter().filter(|i| i.is_included).collect();
            let asked: Vec<&ContractItem> = c.items.iter().filter(|i| !i.is_included).collect();
            if !given.is_empty() {
                inner.append(&block(&format!("Items included ({})", given.len()), &items_list(&given)));
            }
            if !asked.is_empty() {
                inner.append(&block(&format!("Items asked for ({})", asked.len()), &items_list(&asked)));
            }
        }
    }

    let sw = w::scrolled(&inner);
    sw.set_min_content_height(320);
    content.append(&sw);
    let close = w::button("Close", Btn::Text);
    let wc = win.clone();
    close.connect_clicked(move |_| wc.close());
    actions.append(&close);
    win.present();
}

// ---------- settings ----------

pub fn settings() -> ct::ContractSettings {
    ct::settings()
}

/// The price hub's name.
pub fn hub_name(s: &ct::ContractSettings) -> &'static str {
    TRADE_HUBS.iter().find(|h| h.1 == s.price_station).map(|h| h.0).unwrap_or("Jita")
}

/// Best buy and sell orders at the contracts price hub.
#[derive(Clone, Default)]
pub struct Prices {
    pub hub: HashMap<i64, BestPrices>,
    pub hub_name: String,
}

impl Prices {
    /// The unit price items are valued at: the lowest sell order or the highest buy order, as the settings say.
    pub fn unit(&self, type_id: i64, at_sell: bool) -> Option<f64> {
        let p = self.hub.get(&type_id)?;
        if at_sell {
            p.sell.or(p.buy)
        } else {
            p.buy.or(p.sell)
        }
    }
}

/// Loads hub prices for `types` off the GTK thread.
pub fn load_prices(ctx: &Ctx, mut types: Vec<i64>, force: bool, done: impl FnOnce(Prices) + 'static) {
    let core = ctx.core().clone();
    let s = settings();
    let name = hub_name(&s).to_string();
    let (station, region) = ct::hub(&s);
    types.sort();
    types.dedup();
    rt::run(
        async move {
            let hub = if types.is_empty() { HashMap::new() } else { core.market.best_prices(&core.esi, region, Some(station), &types, force).await };
            Prices { hub, hub_name: name }
        },
        done,
    )
}

/// The gear button that opens the contracts settings.
pub fn settings_button(ctx: &Ctx) -> gtk::Button {
    let b = w::icon_button("settings", "Contracts settings");
    let ctx = ctx.clone();
    b.connect_clicked(move |_| settings_dialog(&ctx));
    b
}

fn settings_dialog(ctx: &Ctx) {
    let doc = ct::load_doc();
    let s = doc.settings.clone();
    let (win, content, actions) = w::dialog(ctx, "Contracts settings", 520);
    let hubs: Vec<&str> = TRADE_HUBS.iter().map(|h| h.0).collect();
    let hub_dd = w::dropdown(&hubs, TRADE_HUBS.iter().position(|h| h.1 == s.price_station).unwrap_or(0) as u32);
    let basis = w::dropdown(&["Sell orders", "Buy orders"], if s.value_at_sell { 0 } else { 1 });
    content.append(&fields(vec![labelled("Value items at", &hub_dd).upcast(), labelled("Price", &basis).upcast()]));
    let internal = Rc::new(std::cell::Cell::new(s.count_internal));
    {
        let internal = internal.clone();
        content.append(&w::switch_row(
            "Count contracts between my characters",
            Some("Moving a ship from a main to an alt is a purchase for the alt; in the all-characters totals it's usually just a transfer."),
            s.count_internal,
            move |on| internal.set(on),
        ));
    }
    let note = w::line(
        "Finished contracts are valued once, at the prices of the day PodStack first sees them finished, so past profit doesn't move with today's market.",
        &["body-small", "dim"],
    );
    note.set_wrap(true);
    note.set_xalign(0.0);
    content.append(&note);
    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button("Save", Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (win, ctx, internal) = (win.clone(), ctx.clone(), internal.clone());
        save.connect_clicked(move |_| {
            let mut doc = ct::load_doc();
            doc.settings = ct::ContractSettings {
                price_station: TRADE_HUBS.get(hub_dd.selected() as usize).map(|h| h.1).unwrap_or(60003760),
                value_at_sell: basis.selected() == 0,
                count_internal: internal.get(),
            };
            ct::save_doc(ctx.core(), &doc);
            win.close();
            ctx.rebuild();
        });
    }
    actions.append(&cancel);
    actions.append(&save);
    win.present();
}
