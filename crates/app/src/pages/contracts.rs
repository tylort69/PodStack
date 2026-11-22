//! Contracts across all characters: open and completed tables with headline numbers. Also the contracts table,
//! filter bar and contract dialog the character Contracts tab uses.

use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::{label_cell, set_child, type_cell};
use crate::widgets::{self as w, Btn, Status, Table};
use chrono::{DateTime, Utc};
use gtk::prelude::*;
use podstack_core::domains::contracts::{Contract, ContractItem};
use podstack_core::features::comms::{self, Tone};
use podstack_core::fmt;
use std::cell::RefCell;
use std::rc::Rc;

pub const CSS: &str = "
.comms-filter-bar { padding: 4px 0; }
.comms-section-title { margin-top: 4px; }
.comms-kv-key { min-width: 160px; }
.comms-isk-in { color: @success; font-weight: 500; }
.comms-isk-out { color: @error; font-weight: 500; }
";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let chars = ctx.core().characters();
    let open = comms::all_contracts(&chars, Some(false));
    let done = comms::all_contracts(&chars, Some(true));
    let couriers = open.iter().filter(|(_, c)| c.kind == "courier").count();
    let in_progress = open.iter().filter(|(_, c)| c.status == "in_progress").count();
    let outstanding_value: f64 = open.iter().filter_map(|(_, c)| comms::contract_value(c)).filter(|v| *v > 0.0).sum();

    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&w::tile_row(vec![
        w::stat_tile("Open", &open.len().to_string(), Some(&format!("{in_progress} in progress"))),
        w::stat_tile("Couriers in flight", &couriers.to_string(), None),
        w::stat_tile("Open value", &format!("{} ISK", fmt::isk_short(outstanding_value)), Some("Prices and rewards")),
        w::stat_tile("Completed", &done.len().to_string(), Some("Last 30 days")),
    ]));
    let missing: Vec<String> = chars.iter().filter(|c| c.contracts.is_none()).map(|c| c.name()).collect();
    if !missing.is_empty() {
        body.append(&w::banner(
            "lock",
            &format!("No contracts loaded for {} (they may need to log in again to grant Read Contracts).", missing.join(", ")),
            w::BannerKind::Notice,
        ));
    }
    body.append(&contracts_section(ctx, "contracts", open, done));
    Page::new("Contracts", body).no_scroll()
}

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
