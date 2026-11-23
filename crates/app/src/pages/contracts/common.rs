//! Shared by the Contracts tabs (and the character Contracts tab): the contracts table and its filter bar, the
//! contract dialog, the settings dialog and price loading.

use crate::ctx::Ctx;
use crate::pages::industry::form::{fields, labelled, security_label, system_name};
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
use podstack_core::sde::{Route, RoutePref, MAP, TRADE_HUBS};
use std::collections::HashMap;
use std::cell::RefCell;
use std::rc::Rc;

pub const CSS: &str = "
.comms-filter-bar { padding: 4px 0; }
.comms-section-title { margin-top: 4px; }
.comms-kv-key { min-width: 160px; }
.comms-isk-in { color: @success; font-weight: 500; }
.comms-isk-out { color: @error; font-weight: 500; }
.comms-note { border-radius: 8px; padding: 8px; background-color: @surface_container_highest; }
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

pub(crate) fn status_kind(t: Tone) -> Status {
    match t {
        Tone::Good => Status::Good,
        Tone::Warn => Status::Warn,
        Tone::Bad => Status::Bad,
        Tone::Info => Status::Info,
        Tone::Neutral => Status::Neutral,
    }
}

pub(crate) fn date_cell(t: DateTime<Utc>) -> gtk::Widget {
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

/// The contract's details: parties, dates, the courier route with its jumps and security, the price against what
/// the items are worth (as valued when it finished, or at the hub now while it's open), auction bids, what it meant
/// for each of the user's characters and corporations, its items, and the user's tags and note.
pub(crate) fn open_contract(ctx: &Ctx, c: &Contract) {
    crate::theme::add_css(CSS);
    let chars = ctx.core().characters();
    let mine = ct::Mine::of(&chars);
    let s = settings();
    let archived = ct::archive_snapshot().get(c.contract_id).cloned();
    // the archive may have what this listing lacks (items another character loaded, bids)
    let mut c = c.clone();
    if let Some(a) = &archived {
        if !c.items_loaded && a.contract.items_loaded {
            c.items = a.contract.items.clone();
            c.items_loaded = true;
        }
        if c.bids.is_empty() {
            c.bids = a.contract.bids.clone();
        }
    }
    let c = &c;
    let roles = ct::roles(c, &mine);
    let valuation = archived.as_ref().and_then(|a| a.valuation.clone());
    let flows = ct::flows(c, &mine, valuation.as_ref(), s.value_at_sell);

    let title = if c.title.trim().is_empty() { format!("{} contract", comms::title_case(&c.kind)) } else { c.title.clone() };
    let (win, content, actions) = w::dialog(ctx, &title, 680);
    win.set_default_height(720);

    let inner = w::vbox(12);
    let chips = w::hbox(8);
    chips.append(&w::status(&comms::title_case(&c.kind), Status::Neutral));
    chips.append(&w::status(&comms::title_case(&c.status), status_kind(comms::contract_status_tone(&c.status))));
    chips.append(&w::status(&comms::title_case(&c.availability), Status::Neutral));
    if c.for_corporation {
        chips.append(&w::status("On behalf of corporation", Status::Neutral));
    }
    let (issuer, acceptor) = ct::sides(c, &mine);
    if issuer.is_some() && acceptor.is_some() {
        chips.append(&w::status("Between your characters", Status::Info));
    }
    if c.status == "outstanding" && roles.iter().any(|r| r.1 == ct::Role::Assigned) {
        chips.append(&w::status("Waiting for you", Status::Warn));
    }
    inner.append(&chips);

    let availability = match &c.assignee_name {
        Some(a) => format!("{} – {a}", comms::title_case(&c.availability)),
        None => comms::title_case(&c.availability),
    };
    let mut overview = vec![
        ("Description", if c.title.trim().is_empty() { "N/A".into() } else { c.title.clone() }, None),
        ("Issuer", c.issuer_name.clone(), None),
        ("Issuer corporation", c.issuer_corporation_name.clone(), None),
        ("On behalf of corp", if c.for_corporation { "Yes" } else { "No" }.into(), None),
        ("Availability", availability, None),
        ("Acceptor", c.acceptor_name.clone().unwrap_or_else(|| "Not accepted".into()), None),
    ];
    if !roles.is_empty() {
        let mut yours: Vec<String> = Vec::new();
        for (p, r) in &roles {
            let t = format!("{} ({})", mine.name(*p), r.label().to_lowercase());
            if !yours.contains(&t) {
                yours.push(t);
            }
        }
        overview.push(("Yours", yours.join(", "), None));
    }
    inner.append(&block("Overview", &kv(&overview)));

    let mut dates = vec![
        ("Issued", when(Some(c.date_issued), ""), None),
        ("Expires", when(Some(c.date_expired), ""), None),
        ("Accepted", when(c.date_accepted, "Not accepted"), None),
        ("Completed", when(c.date_completed, "Not completed"), None),
    ];
    if let Some(due) = ct::courier_due(c) {
        let late = due <= Utc::now();
        dates.push(("Deliver by", when(Some(due), ""), Some(if late { "error-text" } else { "warning-text" })));
    }
    if let Some(a) = &archived {
        dates.push(("First seen", when(Some(a.first_seen), ""), None));
    }
    inner.append(&block("Dates", &kv(&dates)));

    match c.kind.as_str() {
        "courier" => {
            let place = |p: &Option<podstack_core::universe::Place>| p.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Unknown".into());
            let volume = c.volume.unwrap_or(0.0);
            let mut rows = vec![
                ("Pickup", place(&c.start_location), None),
                ("Delivery", place(&c.end_location), None),
                ("Volume", format!("{} m³", fmt::decimal(volume, 2)), None),
                ("Reward", isk(c.reward), Some("comms-isk-in")),
                ("Collateral", isk(c.collateral), Some("comms-isk-out")),
                ("Days to complete", c.days_to_complete.unwrap_or(0).to_string(), None),
            ];
            if volume > 0.0 && c.reward.unwrap_or(0.0) > 0.0 {
                rows.push(("Reward per m³", fmt::isk(c.reward.unwrap_or(0.0) / volume), None));
            }
            inner.append(&block("Courier", &kv(&rows)));
            if let Some(r) = route_block(c) {
                inner.append(&r);
            }
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
            if let Some(top) = c.bids.first() {
                rows.push((if c.is_finished() { "Winning bid" } else { "Highest bid" }, fmt::isk(top.amount), Some("comms-isk-in")));
            }
            if let Some(p) = &c.start_location {
                rows.push(("Location", p.name.clone(), None));
            }
            inner.append(&block("Auction", &kv(&rows)));
            if !c.bids.is_empty() {
                inner.append(&block(&format!("Bids ({})", c.bids.len()), &bids_list(c, &mine)));
            }
        }
        _ => {}
    }

    if c.has_items() && c.items_loaded && !c.items.is_empty() {
        if let Some(v) = &valuation {
            let hub = TRADE_HUBS.iter().find(|h| h.1 == v.station).map(|h| h.0).unwrap_or("the hub");
            let mut rows = value_rows(c, v.included(s.value_at_sell), v.asked(s.value_at_sell));
            rows.push(("Valued", format!("At {hub} {} on {}", basis(s.value_at_sell), fmt::date(v.at)), None));
            if v.unpriced > 0 {
                rows.push(("Without orders", format!("{} item type{} (counted as nothing)", v.unpriced, if v.unpriced == 1 { "" } else { "s" }), None));
            }
            inner.append(&block("Worth", &kv(&rows)));
        } else if !c.is_completed() {
            inner.append(&live_worth(ctx, c, s.value_at_sell));
        }
    }

    if !flows.is_empty() {
        let list = w::vbox(4);
        for f in &flows {
            list.append(&flow_row(&mine, f));
        }
        inner.append(&block("What it meant for you", &list));
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

    let tags = w::vbox(8);
    render_tags(ctx, c.contract_id, &tags);
    inner.append(&block("Tags", &tags));

    let note_text = ct::load_doc().notes.get(&c.contract_id).cloned().unwrap_or_default();
    let note = gtk::TextView::new();
    note.set_wrap_mode(gtk::WrapMode::WordChar);
    note.add_css_class("comms-note");
    note.buffer().set_text(&note_text);
    note.set_size_request(-1, 72);
    inner.append(&block("Note", &note));

    let sw = w::scrolled(&inner);
    sw.set_min_content_height(360);
    content.append(&sw);
    let close = w::button("Close", Btn::Text);
    let wc = win.clone();
    close.connect_clicked(move |_| wc.close());
    actions.append(&close);
    {
        // the note is saved when the dialog closes, however it closes
        let (ctx, id) = (ctx.clone(), c.contract_id);
        win.connect_close_request(move |_| {
            let b = note.buffer();
            let text = b.text(&b.start_iter(), &b.end_iter(), false).trim().to_string();
            if text != note_text.trim() {
                let mut doc = ct::load_doc();
                if text.is_empty() {
                    doc.notes.remove(&id);
                } else {
                    doc.notes.insert(id, text);
                }
                ct::save_doc(ctx.core(), &doc);
            }
            gtk::glib::Propagation::Proceed
        });
    }
    win.present();
}

fn basis(at_sell: bool) -> &'static str {
    if at_sell {
        "sell orders"
    } else {
        "buy orders"
    }
}

/// "92% of market (8% under)".
pub(crate) fn ratio_text(r: f64) -> String {
    let pct = r * 100.0;
    if (pct - 100.0).abs() < 0.5 {
        "At market".into()
    } else if r < 1.0 {
        format!("{pct:.0}% of market ({:.0}% under)", 100.0 - pct)
    } else {
        format!("{pct:.0}% of market ({:.0}% over)", pct - 100.0)
    }
}

/// What the items are worth and what the acceptor pays for them.
fn value_rows(c: &Contract, included: f64, asked: f64) -> Vec<(&'static str, String, Option<&'static str>)> {
    let mut rows = vec![("Items included", fmt::isk(included), None)];
    if asked > 0.0 {
        rows.push(("Items asked for", fmt::isk(asked), None));
    }
    if let Some(d) = ct::deal(c, included, asked) {
        rows.push(("Acceptor pays", fmt::isk(d.price), None));
        if let Some(r) = d.ratio() {
            // good or bad depends on the side, so only a price far off the market stands out
            rows.push(("Price against market", ratio_text(r), (!(0.5..=1.5).contains(&r)).then_some("warning-text")));
        }
    }
    rows
}

/// An open contract's items at the hub's prices now (loaded in the background).
fn live_worth(ctx: &Ctx, c: &Contract, at_sell: bool) -> gtk::Box {
    let holder = w::vbox(8);
    holder.append(&w::text("Loading hub prices…", &["body-medium", "dim"]));
    let b = block("Worth now", &holder);
    let c2 = c.clone();
    let types: Vec<i64> = c.items.iter().map(|i| i.type_id).collect();
    load_prices(ctx, types, false, move |p| {
        while let Some(ch) = holder.first_child() {
            holder.remove(&ch);
        }
        let (mut included, mut asked, mut unpriced) = (0.0, 0.0, 0);
        for i in &c2.items {
            // blueprint copies have no market: their original's price would be wildly wrong
            if i.is_blueprint_copy() {
                continue;
            }
            match p.unit(i.type_id, at_sell) {
                Some(u) if i.is_included => included += u * i.quantity as f64,
                Some(u) => asked += u * i.quantity as f64,
                None => unpriced += 1,
            }
        }
        let mut rows = value_rows(&c2, included, asked);
        rows.push(("Prices", format!("{} {} now", p.hub_name, basis(at_sell)), None));
        if unpriced > 0 {
            rows.push(("Without orders", format!("{unpriced} item{} (counted as nothing)", if unpriced == 1 { "" } else { "s" }), None));
        }
        holder.append(&kv(&rows));
    });
    b
}

/// The gate route between pickup and delivery: jumps, security on the way and the systems themselves.
fn route_block(c: &Contract) -> Option<gtk::Box> {
    let from = c.start_location.as_ref()?.system_id?;
    let to = c.end_location.as_ref()?.system_id?;
    let b = w::vbox(10);
    let Some(short) = MAP.route(from, to, RoutePref::Shortest) else {
        b.append(&w::text("No stargate route connects these systems (wormhole space, or a jump-only destination).", &["body-medium", "dim"]));
        return Some(block("Route", &b));
    };
    let describe = |r: &Route| {
        let (h, l, n) = r.security_counts();
        let mut parts = vec![format!("{} jump{}", r.jumps(), if r.jumps() == 1 { "" } else { "s" })];
        if r.all_high_sec() {
            parts.push("all high-sec".into());
        } else {
            parts.push(format!("{h} high, {l} low, {n} null"));
            if let Some(m) = r.min_security() {
                parts.push(format!("lowest {}", fmt::security(m)));
            }
        }
        parts.join(" · ")
    };
    let mut rows = vec![("Shortest", describe(&short), if short.all_high_sec() { None } else { Some("warning-text") })];
    let safer = MAP.route(from, to, RoutePref::Safer).filter(|s| s.systems != short.systems);
    if let Some(s) = &safer {
        rows.push(("Safer", describe(s), if s.all_high_sec() { None } else { Some("warning-text") }));
    }
    let reward = c.reward.unwrap_or(0.0);
    if short.jumps() > 0 && reward > 0.0 {
        rows.push(("Reward per jump", fmt::isk(reward / short.jumps() as f64), None));
    }
    b.append(&kv(&rows));
    // the systems along the shortest way, for routes short enough to read
    if short.systems.len() <= 40 {
        let systems = gtk::FlowBox::new();
        systems.set_selection_mode(gtk::SelectionMode::None);
        systems.set_column_spacing(4);
        systems.set_row_spacing(4);
        systems.set_max_children_per_line(40);
        for (i, id) in short.systems.iter().enumerate() {
            let s = w::hbox(4);
            if i > 0 {
                s.append(&w::line("›", &["dim"]));
            }
            s.append(&w::line(&system_name(*id), &["body-small"]));
            s.append(&security_label(*id));
            systems.insert(&s, -1);
        }
        b.append(&systems);
    }
    Some(block("Route", &b))
}

fn bids_list(c: &Contract, mine: &ct::Mine) -> gtk::Box {
    let list = w::vbox(6);
    for (i, bid) in c.bids.iter().take(10).enumerate() {
        let row = w::hbox(12);
        let who = if mine.characters.contains_key(&bid.bidder_id) {
            format!("{} (you)", mine.name(ct::Party::Character(bid.bidder_id)))
        } else if mine.corporations.contains_key(&bid.bidder_id) {
            format!("{} (you)", mine.name(ct::Party::Corporation(bid.bidder_id)))
        } else if Some(bid.bidder_id) == c.acceptor_id {
            c.acceptor_name.clone().unwrap_or_else(|| format!("#{}", bid.bidder_id))
        } else {
            format!("Bidder #{}", bid.bidder_id)
        };
        let name = w::line(&who, &["body-medium"]);
        name.set_hexpand(true);
        row.append(&name);
        row.append(&w::line(&fmt::relative(bid.date_bid), &["body-small", "dim"]));
        row.append(&label_cell(&fmt::isk(bid.amount), &["numeric", if i == 0 { "comms-isk-in" } else { "dim" }]));
        list.append(&row);
    }
    if c.bids.len() > 10 {
        list.append(&w::line(&format!("and {} lower bids", c.bids.len() - 10), &["body-small", "dim"]));
    }
    list
}

/// One of the user's sides: "Alice · Bought", what changed hands, and the result.
fn flow_row(mine: &ct::Mine, f: &ct::Flow) -> gtk::Box {
    let mut parts = Vec::new();
    if f.isk_out > 0.0 {
        parts.push(format!("paid {}", fmt::isk_short(f.isk_out)));
    }
    if f.isk_in > 0.0 {
        parts.push(format!("got {}", fmt::isk_short(f.isk_in)));
    }
    let worth = |v: Option<f64>| v.map(|v| format!(" worth {}", fmt::isk_short(v))).unwrap_or_default();
    if !f.items_in.is_empty() {
        parts.push(format!("received items{}", worth(f.value_in)));
    }
    if !f.items_out.is_empty() {
        parts.push(format!("gave items{}", worth(f.value_out)));
    }
    if f.internal {
        parts.push("with your own character".into());
    }
    let mut text = parts.join(" · ");
    if let Some(first) = text.get(..1) {
        text = first.to_uppercase() + &text[1..];
    }
    let result = f.net_worth().unwrap_or(f.net());
    let value = w::line(&super::browse::signed(result), &["numeric", "title-small"]);
    if result != 0.0 {
        value.add_css_class(if result > 0.0 { "comms-isk-in" } else { "comms-isk-out" });
    }
    value.set_tooltip_text(Some(if f.net_worth().is_some() && (!f.items_in.is_empty() || !f.items_out.is_empty()) {
        "ISK and items together"
    } else {
        "ISK"
    }));
    let icon = w::icon(f.category.icon());
    w::list_row(Some(icon.upcast_ref()), &format!("{} · {}", mine.name(f.party), f.category.label()), Some(&text), Some(value.upcast_ref()))
}

/// Changes a contract's tags in the contracts document.
fn set_tags(ctx: &Ctx, id: i64, f: impl FnOnce(&mut Vec<String>)) {
    let mut doc = ct::load_doc();
    let tags = doc.tags.entry(id).or_default();
    f(tags);
    if tags.is_empty() {
        doc.tags.remove(&id);
    }
    ct::save_doc(ctx.core(), &doc);
}

fn add_tag(ctx: &Ctx, id: i64, holder: &gtk::Box, tag: &str) {
    let tag = tag.trim().to_string();
    if tag.is_empty() {
        return;
    }
    set_tags(ctx, id, |t| {
        if !t.iter().any(|x| x.eq_ignore_ascii_case(&tag)) {
            t.push(tag);
        }
    });
    rerender_tags(ctx, id, holder);
}

/// Once the clicked button's handler has returned: rendering replaces it.
fn rerender_tags(ctx: &Ctx, id: i64, holder: &gtk::Box) {
    let (ctx, holder) = (ctx.clone(), holder.clone());
    gtk::glib::idle_add_local_once(move || render_tags(&ctx, id, &holder));
}

/// The contract's tags as removable chips, an entry for a new one and the user's other tags to pick from.
fn render_tags(ctx: &Ctx, id: i64, holder: &gtk::Box) {
    while let Some(c) = holder.first_child() {
        holder.remove(&c);
    }
    let doc = ct::load_doc();
    let tags = doc.tags.get(&id).cloned().unwrap_or_default();
    let chips = gtk::FlowBox::new();
    chips.set_selection_mode(gtk::SelectionMode::None);
    chips.set_column_spacing(8);
    chips.set_row_spacing(8);
    chips.set_max_children_per_line(12);
    for t in &tags {
        let chip = w::button_icon("close", t, Btn::Tonal);
        chip.set_tooltip_text(Some("Remove this tag"));
        let (ctx, holder, t) = (ctx.clone(), holder.clone(), t.clone());
        chip.connect_clicked(move |_| {
            set_tags(&ctx, id, |list| list.retain(|x| *x != t));
            rerender_tags(&ctx, id, &holder);
        });
        chips.insert(&chip, -1);
    }
    if tags.is_empty() {
        holder.append(&w::text("Tag contracts to total them on the Overview: doctrine ships, SRP, buyback, a project…", &["body-medium", "dim"]));
    } else {
        holder.append(&chips);
    }

    let row = w::hbox(8);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("New tag"));
    entry.set_hexpand(true);
    let add = w::button("Add", Btn::Tonal);
    {
        let (ctx, holder, entry2) = (ctx.clone(), holder.clone(), entry.clone());
        add.connect_clicked(move |_| add_tag(&ctx, id, &holder, &entry2.text()));
    }
    {
        let (ctx, holder) = (ctx.clone(), holder.clone());
        entry.connect_activate(move |e| add_tag(&ctx, id, &holder, &e.text()));
    }
    row.append(&entry);
    row.append(&add);
    holder.append(&row);

    let others: Vec<String> = doc.all_tags().into_iter().filter(|t| !tags.contains(t)).take(16).collect();
    if !others.is_empty() {
        let suggestions = gtk::FlowBox::new();
        suggestions.set_selection_mode(gtk::SelectionMode::None);
        suggestions.set_column_spacing(8);
        suggestions.set_row_spacing(8);
        suggestions.set_max_children_per_line(12);
        for t in others {
            let chip = w::button_icon("add", &t, Btn::Outlined);
            let (ctx, holder) = (ctx.clone(), holder.clone());
            chip.connect_clicked(move |_| add_tag(&ctx, id, &holder, &t));
            suggestions.insert(&chip, -1);
        }
        holder.append(&w::line("Your tags", &["label-medium", "dim"]));
        holder.append(&suggestions);
    }
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
