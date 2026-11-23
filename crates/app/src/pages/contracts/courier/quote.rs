//! Courier → Quote: courier services' rate cards (base fee, per jump, per m³, per m³ per jump, a share of the
//! collateral, a minimum, flat-rate routes, and limits), and a calculator that prices a contract on a service and
//! writes out what to fill in when creating it.

use super::{amount_text, clear, header, m3, opt_amount, set_quietly, View};
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::{fields, labelled, number_entry, parse_amount, search_systems, security_label, suggest_entry};
use crate::widgets::{self as w, BannerKind, Btn};
use gtk::prelude::*;
use podstack_core::features::courier::{self as cr, CourierDoc, FlatRoute, Service};
use podstack_core::fmt;
use podstack_core::sde::{RoutePref, MAP};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

const SERVICE_KEY: &str = "contracts.courier.quote.service";
const FROM_KEY: &str = "contracts.courier.quote.from";
const TO_KEY: &str = "contracts.courier.quote.to";
const VOLUME_KEY: &str = "contracts.courier.quote.volume";
const COLLATERAL_KEY: &str = "contracts.courier.quote.collateral";

fn system_name(id: i64) -> String {
    super::system_name(Some(id))
}

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.append(&header(ctx, View::Quote));
    let doc = cr::load_doc();
    body.append(&calculator(ctx, &doc));
    // with no service yet the calculator's own empty state has the one button needed
    if !doc.services.is_empty() {
        body.append(&services_card(ctx, &doc));
    }
    // the calculator holds what's being typed: it updates itself, and saving a service rebuilds the page
    Page::new("Contracts", body).static_page()
}

fn opt_text(v: Option<f64>) -> String {
    v.map(amount_text).unwrap_or_default()
}

fn amount(e: &gtk::Entry) -> f64 {
    opt_amount(e).unwrap_or(0.0)
}

// ---------- calculator ----------

struct State {
    service: usize,
    from: Option<i64>,
    to: Option<i64>,
}

fn calculator(ctx: &Ctx, doc: &CourierDoc) -> gtk::Box {
    if doc.services.is_empty() {
        let b = w::vbox(12);
        b.append(&w::empty_state(
            "request_quote",
            "No courier services yet",
            "Add a service with its rates and limits (yours, or one you use) to price courier contracts in a few clicks.",
        ));
        let add = w::button_icon("add", "Add a service", Btn::Filled);
        add.set_halign(gtk::Align::Center);
        let ctx = ctx.clone();
        add.connect_clicked(move |_| editor(&ctx, None));
        b.append(&add);
        b.set_vexpand(false);
        let card = w::card(&b);
        card.set_vexpand(false);
        return card;
    }
    let services = Rc::new(doc.services.clone());
    let chosen = ctx.recall(SERVICE_KEY).and_then(|id| services.iter().position(|s| s.id == id)).unwrap_or(0);
    let id_of = |k: &str| ctx.recall(k).and_then(|s| s.parse::<i64>().ok()).filter(|id| MAP.system(*id).is_some());
    let state = Rc::new(RefCell::new(State { service: chosen, from: id_of(FROM_KEY), to: id_of(TO_KEY) }));

    let names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
    let service_dd = w::dropdown(&names, chosen as u32);
    let volume = number_entry(&ctx.recall(VOLUME_KEY).unwrap_or_default(), 12);
    volume.set_placeholder_text(Some("m³"));
    let collateral = number_entry(&ctx.recall(COLLATERAL_KEY).unwrap_or_default(), 14);
    collateral.set_placeholder_text(Some("e.g. 1.5b"));
    let result = w::vbox(12);

    let compute: Rc<dyn Fn()> = {
        let (ctx, state, services, volume, collateral, result) = (ctx.clone(), state.clone(), services.clone(), volume.clone(), collateral.clone(), result.clone());
        Rc::new(move || {
            ctx.remember(VOLUME_KEY, volume.text().as_str());
            ctx.remember(COLLATERAL_KEY, collateral.text().as_str());
            let st = state.borrow();
            let Some(s) = services.get(st.service) else { return };
            fill_result(&ctx, &result, s, st.from, st.to, amount(&volume), amount(&collateral));
        })
    };

    let from_entry = suggest_entry("Pickup system", search_systems, {
        let (ctx, state, compute) = (ctx.clone(), state.clone(), compute.clone());
        move |id, _| {
            state.borrow_mut().from = Some(id);
            ctx.remember(FROM_KEY, id.to_string());
            compute();
        }
    });
    let to_entry = suggest_entry("Delivery system", search_systems, {
        let (ctx, state, compute) = (ctx.clone(), state.clone(), compute.clone());
        move |id, _| {
            state.borrow_mut().to = Some(id);
            ctx.remember(TO_KEY, id.to_string());
            compute();
        }
    });
    for (e, id) in [(&from_entry, state.borrow().from), (&to_entry, state.borrow().to)] {
        e.set_width_chars(18);
        if let Some(id) = id {
            set_quietly(e, &system_name(id));
        }
    }
    let swap = w::icon_button("swap_horiz", "Swap pickup and delivery");
    {
        let (ctx, state, compute, from_entry, to_entry) = (ctx.clone(), state.clone(), compute.clone(), from_entry.clone(), to_entry.clone());
        swap.connect_clicked(move |_| {
            let (f, t) = {
                let mut st = state.borrow_mut();
                (st.from, st.to) = (st.to, st.from);
                (st.from, st.to)
            };
            set_quietly(&from_entry, &f.map(system_name).unwrap_or_default());
            set_quietly(&to_entry, &t.map(system_name).unwrap_or_default());
            ctx.remember(FROM_KEY, f.map(|i| i.to_string()).unwrap_or_default());
            ctx.remember(TO_KEY, t.map(|i| i.to_string()).unwrap_or_default());
            compute();
        });
    }
    {
        let (ctx, state, compute, services) = (ctx.clone(), state.clone(), compute.clone(), services.clone());
        service_dd.connect_selected_notify(move |d| {
            let i = d.selected() as usize;
            state.borrow_mut().service = i;
            if let Some(s) = services.get(i) {
                ctx.remember(SERVICE_KEY, s.id.as_str());
            }
            compute();
        });
    }
    for e in [&volume, &collateral] {
        let compute = compute.clone();
        e.connect_changed(move |_| compute());
    }

    let swap_box = w::vbox(0);
    swap_box.append(&swap);
    let form = fields(vec![
        labelled("Service", &service_dd).upcast(),
        labelled("Pickup", &from_entry).upcast(),
        swap_box.upcast(),
        labelled("Delivery", &to_entry).upcast(),
        labelled("Volume (m³)", &volume).upcast(),
        labelled("Collateral (ISK)", &collateral).upcast(),
    ]);
    let inner = w::vbox(16);
    inner.append(&form);
    inner.append(&result);
    compute();
    w::card_with("Quote", Some("Price a courier contract on one of your services"), &[], &inner)
}

fn fill_result(ctx: &Ctx, b: &gtk::Box, s: &Service, from: Option<i64>, to: Option<i64>, volume: f64, collateral: f64) {
    clear(b);
    let (Some(from), Some(to)) = (from, to) else {
        b.append(&w::line("Pick the pickup and delivery systems.", &["body-medium", "dim"]));
        return;
    };
    let q = cr::quote(s, from, to, volume, collateral);

    let top = w::hbox(24);
    let reward = w::vbox(4);
    reward.append(&w::line("Reward", &["label-medium", "dim"]));
    let value = w::line(&q.reward.map(|r| format!("{} ISK", fmt::decimal(r, 0))).unwrap_or_else(|| "–".into()), &["courier-reward", "numeric"]);
    value.set_selectable(true);
    reward.append(&value);
    let mut sub = Vec::new();
    if let Some(v) = q.isk_per_jump() {
        sub.push(format!("{} per jump", fmt::isk_short(v)));
    }
    if let (Some(r), true) = (q.reward, volume > 0.0) {
        sub.push(format!("{} per m³", fmt::isk_short(r / volume)));
    }
    if q.minimum_applied {
        sub.push("the service's minimum".into());
    }
    reward.append(&w::line(&sub.join(" · "), &["body-small", "dim"]));
    top.append(&reward);

    let route = w::vbox(4);
    route.append(&w::line("Route", &["label-medium", "dim"]));
    let ends = w::hbox(6);
    ends.append(&security_label(from));
    ends.append(&w::line(&system_name(from), &["body-large"]));
    ends.append(&w::icon("arrow_forward"));
    ends.append(&security_label(to));
    ends.append(&w::line(&system_name(to), &["body-large"]));
    route.append(&ends);
    let how = match (q.route, q.flat) {
        (Some(r), true) => format!("{} · flat-rate route", r.describe()),
        (Some(r), false) => format!("{} ({})", r.describe(), s.route.label().to_lowercase()),
        (None, true) => "Flat-rate route".into(),
        (None, false) => "No stargate route".into(),
    };
    route.append(&w::line(&how, &["body-small", "dim"]));
    top.append(&route);
    if let Some(r) = q.reward {
        let copy = w::button_icon("content_copy", "Copy reward", Btn::Tonal);
        copy.set_valign(gtk::Align::Center);
        copy.set_hexpand(true);
        copy.set_halign(gtk::Align::End);
        let ctx = ctx.clone();
        copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &format!("{}", r.round() as i64)));
        top.append(&copy);
    }
    b.append(&top);

    for p in &q.problems {
        let banner = w::banner("block", &p.label(), BannerKind::Error);
        banner.add_css_class("courier-problem");
        b.append(&banner);
    }

    if !q.parts.is_empty() {
        let mut rows: Vec<(&str, String)> = q.parts.iter().map(|(l, v)| (l.as_str(), fmt::isk(*v))).collect();
        if q.minimum_applied {
            rows.push(("Raised to the minimum", fmt::isk(s.pricing.minimum)));
        }
        if let Some(r) = q.reward {
            rows.push(("Reward", fmt::isk(r)));
        }
        let kv = w::key_values(&rows);
        let block = w::vbox(8);
        block.add_css_class("card-filled");
        block.append(&w::text("How it adds up", &["title-small"]));
        block.append(&kv);
        b.append(&block);
    }

    let text = cr::contract_text(s, &system_name(from), &system_name(to), volume, collateral, &q);
    let tv = gtk::TextView::new();
    tv.set_editable(false);
    tv.set_monospace(true);
    tv.set_wrap_mode(gtk::WrapMode::WordChar);
    tv.add_css_class("courier-text");
    tv.buffer().set_text(&text);
    let block = w::vbox(8);
    block.add_css_class("card-filled");
    let head = w::hbox(8);
    let title = w::text("Contract to create", &["title-small"]);
    title.set_hexpand(true);
    head.append(&title);
    let copy = w::button_icon("content_copy", "Copy", Btn::Text);
    {
        let ctx = ctx.clone();
        copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &text));
    }
    head.append(&copy);
    block.append(&head);
    block.append(&tv);
    if volume <= 0.0 {
        block.append(&w::line("Enter the cargo's volume: the price and the limits depend on it.", &["body-small", "dim"]));
    }
    b.append(&block);
}

// ---------- the services ----------

fn limits_text(s: &Service) -> String {
    let mut parts = Vec::new();
    if let Some(v) = s.max_volume {
        parts.push(format!("up to {}", m3(v)));
    }
    if let Some(c) = s.max_collateral {
        parts.push(format!("collateral up to {}", fmt::isk_short(c)));
    }
    if let Some(j) = s.max_jumps {
        parts.push(format!("up to {j} jumps"));
    }
    if s.high_sec_only {
        parts.push("high-sec only".into());
    }
    if !s.pickups.is_empty() {
        parts.push(format!("from {}", list_names(&s.pickups)));
    }
    if !s.deliveries.is_empty() {
        parts.push(format!("to {}", list_names(&s.deliveries)));
    }
    parts.push(format!("{} day{} to complete", s.days_to_complete, if s.days_to_complete == 1 { "" } else { "s" }));
    parts.join(" · ")
}

fn list_names(ids: &[i64]) -> String {
    let names: Vec<String> = ids.iter().take(4).map(|i| system_name(*i)).collect();
    if ids.len() > 4 {
        format!("{} and {} more", names.join(", "), ids.len() - 4)
    } else {
        names.join(", ")
    }
}

fn services_card(ctx: &Ctx, doc: &CourierDoc) -> gtk::Box {
    let list = w::vbox(0);
    if doc.services.is_empty() {
        list.append(&w::line("Nothing here yet.", &["body-medium", "dim"]));
    }
    for s in &doc.services {
        let actions = w::hbox(4);
        let copy = w::icon_button("content_copy", "Duplicate");
        let edit = w::icon_button("edit", "Edit");
        let del = w::icon_button("delete", "Delete");
        {
            let (ctx, s) = (ctx.clone(), s.clone());
            copy.connect_clicked(move |_| {
                let mut d = cr::load_doc();
                let mut c = s.clone();
                c.id = new_id();
                c.name = format!("{} (copy)", s.name);
                d.services.push(c);
                cr::save_doc(ctx.core(), &d);
                ctx.rebuild();
            });
        }
        {
            let (ctx, s) = (ctx.clone(), s.clone());
            edit.connect_clicked(move |_| editor(&ctx, Some(s.clone())));
        }
        {
            let (ctx, s) = (ctx.clone(), s.clone());
            del.connect_clicked(move |_| {
                let (ctx2, id) = (ctx.clone(), s.id.clone());
                w::confirm(&ctx, "Delete service?", &format!("{} and its rates will be removed.", s.name), "Delete", true, move || {
                    let mut d = cr::load_doc();
                    d.services.retain(|x| x.id != id);
                    cr::save_doc(ctx2.core(), &d);
                    ctx2.rebuild();
                });
            });
        }
        actions.append(&copy);
        actions.append(&edit);
        actions.append(&del);
        let lead = w::icon("local_shipping");
        let sub = format!("{}\n{}", s.describe(), limits_text(s));
        let row = w::list_row(Some(lead.upcast_ref()), &s.name, Some(&sub), Some(actions.upcast_ref()));
        list.append(&row);
    }
    let add = w::button_icon("add", "Add service", Btn::Tonal);
    {
        let ctx = ctx.clone();
        add.connect_clicked(move |_| editor(&ctx, None));
    }
    w::card_with("Courier services", Some("Rate cards for the services you run or use"), &[add.upcast()], &list)
}

fn new_id() -> String {
    format!("s{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default())
}

/// A list's own refill, held weakly so the closure doesn't keep itself alive.
type Refill = Weak<dyn Fn()>;

/// Systems as removable chips, with an entry to add more.
fn system_list(placeholder: &str, systems: Rc<RefCell<Vec<i64>>>) -> gtk::Box {
    let b = w::vbox(8);
    let chips = gtk::FlowBox::new();
    chips.set_selection_mode(gtk::SelectionMode::None);
    chips.set_max_children_per_line(6);
    chips.set_column_spacing(6);
    chips.set_row_spacing(6);
    chips.set_halign(gtk::Align::Start);
    // the chips refill themselves when one is removed: weakly, so the closure doesn't keep itself alive
    let refill: Rc<RefCell<Option<Refill>>> = Rc::default();
    let f: Rc<dyn Fn()> = {
        let (chips, systems, refill) = (chips.clone(), systems.clone(), refill.clone());
        Rc::new(move || {
            chips.remove_all();
            for id in systems.borrow().iter().copied() {
                let btn = gtk::Button::new();
                btn.add_css_class("chip");
                btn.add_css_class("courier-chip");
                let row = w::hbox(4);
                row.append(&security_label(id));
                row.append(&gtk::Label::new(Some(&system_name(id))));
                row.append(&w::icon_sized("close", 16));
                btn.set_child(Some(&row));
                btn.set_tooltip_text(Some("Remove"));
                let (systems, refill) = (systems.clone(), refill.clone());
                btn.connect_clicked(move |_| {
                    systems.borrow_mut().retain(|s| *s != id);
                    // after the click handler: the refill removes this button
                    let refill = refill.clone();
                    gtk::glib::idle_add_local_once(move || {
                        if let Some(f) = refill.borrow().as_ref().and_then(|w| w.upgrade()) {
                            f();
                        }
                    });
                });
                chips.insert(&btn, -1);
            }
        })
    };
    *refill.borrow_mut() = Some(Rc::downgrade(&f));
    let holder: Rc<RefCell<Option<gtk::SearchEntry>>> = Rc::default();
    let entry = suggest_entry(placeholder, search_systems, {
        let (systems, f, holder) = (systems.clone(), f.clone(), holder.clone());
        move |id, _| {
            if !systems.borrow().contains(&id) {
                systems.borrow_mut().push(id);
            }
            f();
            // empty again, ready for the next one (once the entry has finished showing the pick)
            if let Some(e) = holder.borrow().clone() {
                gtk::glib::idle_add_local_once(move || set_quietly(&e, ""));
            }
        }
    });
    *holder.borrow_mut() = Some(entry.clone());
    entry.set_width_chars(24);
    entry.set_hexpand(false);
    f();
    b.append(&chips);
    b.append(&entry);
    b
}

/// The flat-rate routes, each removable, and a row to add one.
fn flat_routes(ctx: &Ctx, routes: Rc<RefCell<Vec<FlatRoute>>>) -> gtk::Box {
    let b = w::vbox(8);
    let list = w::vbox(4);
    // weakly, like the system chips
    let fill: Rc<RefCell<Option<Refill>>> = Rc::default();
    let f: Rc<dyn Fn()> = {
        let (list, routes, fill) = (list.clone(), routes.clone(), fill.clone());
        Rc::new(move || {
            clear(&list);
            if routes.borrow().is_empty() {
                list.append(&w::line("None: every route is priced by the rates above.", &["body-small", "dim"]));
            }
            for (i, r) in routes.borrow().iter().enumerate() {
                let row = w::hbox(8);
                let arrow = if r.both_ways { "⇄" } else { "→" };
                let price = if r.per_m3 { format!("{} per m³", fmt::isk_short(r.price)) } else { fmt::isk(r.price) };
                let fee = if r.collateral_fee { " + collateral share" } else { "" };
                let l = w::line(&format!("{} {arrow} {} · {price}{fee}", system_name(r.from), system_name(r.to)), &["body-medium"]);
                l.set_hexpand(true);
                row.append(&l);
                let del = w::icon_button("delete", "Remove route");
                let (routes, fill) = (routes.clone(), fill.clone());
                del.connect_clicked(move |_| {
                    if i < routes.borrow().len() {
                        routes.borrow_mut().remove(i);
                    }
                    let fill = fill.clone();
                    gtk::glib::idle_add_local_once(move || {
                        if let Some(f) = fill.borrow().as_ref().and_then(|w| w.upgrade()) {
                            f();
                        }
                    });
                });
                row.append(&del);
                list.append(&row);
            }
        })
    };
    *fill.borrow_mut() = Some(Rc::downgrade(&f));
    f();

    let picked: Rc<Cell<(Option<i64>, Option<i64>)>> = Rc::new(Cell::new((None, None)));
    let from = suggest_entry("From", search_systems, {
        let picked = picked.clone();
        move |id, _| picked.set((Some(id), picked.get().1))
    });
    let to = suggest_entry("To", search_systems, {
        let picked = picked.clone();
        move |id, _| picked.set((picked.get().0, Some(id)))
    });
    for e in [&from, &to] {
        e.set_width_chars(14);
        e.set_hexpand(false);
    }
    let price = number_entry("", 12);
    price.set_placeholder_text(Some("Price"));
    let unit = w::dropdown(&["for the contract", "per m³"], 0);
    let both = w::chip("Both ways", true);
    let fee = w::chip("Plus collateral share", true);
    let add = w::button_icon("add", "Add", Btn::Tonal);
    {
        let (ctx, routes, f, picked, from, to, price, unit, both, fee) =
            (ctx.clone(), routes.clone(), f.clone(), picked.clone(), from.clone(), to.clone(), price.clone(), unit.clone(), both.clone(), fee.clone());
        add.connect_clicked(move |_| {
            let (Some(a), Some(b)) = picked.get() else {
                ctx.toast("Pick both systems from the suggestions first");
                return;
            };
            let Some(p) = parse_amount(&price.text()).filter(|p| *p >= 0.0) else {
                ctx.toast("Enter the route's price");
                return;
            };
            routes.borrow_mut().retain(|r| !(r.from == a && r.to == b));
            routes.borrow_mut().push(FlatRoute { from: a, to: b, both_ways: both.is_active(), price: p, per_m3: unit.selected() == 1, collateral_fee: fee.is_active() });
            picked.set((None, None));
            from.set_text("");
            to.set_text("");
            price.set_text("");
            f();
        });
    }
    let form = fields(vec![from.upcast(), to.upcast(), price.upcast(), unit.upcast(), both.upcast(), fee.upcast(), add.upcast()]);
    b.append(&list);
    b.append(&form);
    b
}

fn section(title: &str) -> gtk::Label {
    let l = w::text(title, &["title-small"]);
    l.set_margin_top(8);
    l
}

/// The add / edit dialog for a service.
fn editor(ctx: &Ctx, existing: Option<Service>) {
    let editing = existing.is_some();
    let s = existing.unwrap_or_else(|| Service { id: new_id(), ..Default::default() });
    let (win, content, actions) = w::dialog(ctx, if editing { "Edit courier service" } else { "Add courier service" }, 680);
    win.set_default_height(760);
    let form = w::vbox(12);

    let entry = |text: &str, placeholder: &str| {
        let e = gtk::Entry::new();
        e.set_text(text);
        e.set_placeholder_text(Some(placeholder));
        e.set_hexpand(true);
        e
    };
    let name = entry(&s.name, "e.g. Home → Jita freight");
    let assignee = entry(&s.assignee, "Character or corporation the contract goes to");
    let description = entry(&s.description, "Shown on the contract (optional)");
    form.append(&labelled("Name", &name));
    form.append(&fields(vec![labelled("Assign contracts to", &assignee).upcast(), labelled("Description", &description).upcast()]));

    form.append(&section("Pricing"));
    let p = &s.pricing;
    let money = |v: f64, ph: &str| {
        let e = number_entry(&amount_text(v), 12);
        e.set_placeholder_text(Some(ph));
        e
    };
    let base = money(p.base, "0");
    let per_jump = money(p.per_jump, "0");
    let per_m3 = money(p.per_m3, "0");
    let per_m3_jump = money(p.per_m3_jump, "0");
    let coll = number_entry(&if p.collateral_rate > 0.0 { cr::percent(p.collateral_rate) } else { String::new() }, 8);
    coll.set_placeholder_text(Some("0"));
    let minimum = money(p.minimum, "none");
    let round_to = money(p.round_to, "1");
    form.append(&fields(vec![
        labelled("Base fee", &base).upcast(),
        labelled("Per jump", &per_jump).upcast(),
        labelled("Per m³", &per_m3).upcast(),
        labelled("Per m³ per jump", &per_m3_jump).upcast(),
        labelled("Collateral %", &coll).upcast(),
        labelled("Minimum reward", &minimum).upcast(),
        labelled("Round up to", &round_to).upcast(),
    ]));
    let hint = w::line("Amounts take k, m and b (\"1.5m\"). Every part adds up; the minimum applies to the total.", &["body-small", "dim"]);
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    form.append(&hint);

    form.append(&section("Limits"));
    let max_volume = number_entry(&opt_text(s.max_volume), 12);
    max_volume.set_placeholder_text(Some("no limit"));
    let max_collateral = number_entry(&opt_text(s.max_collateral), 12);
    max_collateral.set_placeholder_text(Some("no limit"));
    let max_jumps = number_entry(&s.max_jumps.map(|j| j.to_string()).unwrap_or_default(), 6);
    max_jumps.set_placeholder_text(Some("no limit"));
    let labels: Vec<&str> = RoutePref::ALL.iter().map(|p| p.label()).collect();
    let route = w::dropdown(&labels, RoutePref::ALL.iter().position(|r| *r == s.route).unwrap_or(0) as u32);
    let days = number_entry(&s.days_to_complete.to_string(), 4);
    let expiry = number_entry(&s.expiration_days.to_string(), 4);
    form.append(&fields(vec![
        labelled("Max volume (m³)", &max_volume).upcast(),
        labelled("Max collateral", &max_collateral).upcast(),
        labelled("Max jumps", &max_jumps).upcast(),
        labelled("Route", &route).upcast(),
        labelled("Days to complete", &days).upcast(),
        labelled("Expires in (days)", &expiry).upcast(),
    ]));
    let high_sec = Rc::new(Cell::new(s.high_sec_only));
    {
        let high_sec = high_sec.clone();
        form.append(&w::switch_row(
            "High-sec only",
            Some("Refuse routes that leave high-sec (on the route setting above)"),
            s.high_sec_only,
            move |on| high_sec.set(on),
        ));
    }
    let pickups = Rc::new(RefCell::new(s.pickups.clone()));
    let deliveries = Rc::new(RefCell::new(s.deliveries.clone()));
    form.append(&section("Pickup systems"));
    form.append(&w::line("Leave empty to pick up anywhere.", &["body-small", "dim"]));
    form.append(&system_list("Add a pickup system", pickups.clone()));
    form.append(&section("Delivery systems"));
    form.append(&w::line("Leave empty to deliver anywhere.", &["body-small", "dim"]));
    form.append(&system_list("Add a delivery system", deliveries.clone()));

    form.append(&section("Flat-rate routes"));
    let note = w::line("A fixed price for a route replaces the rates above, and the jump and system limits don't apply to it.", &["body-small", "dim"]);
    note.set_wrap(true);
    note.set_xalign(0.0);
    form.append(&note);
    let flats = Rc::new(RefCell::new(s.flat_routes.clone()));
    form.append(&flat_routes(ctx, flats.clone()));

    form.set_margin_end(12);
    let sw = w::scrolled(&form);
    sw.set_min_content_height(480);
    content.append(&sw);

    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button("Save", Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (win, ctx) = (win.clone(), ctx.clone());
        save.connect_clicked(move |_| {
            let mut svc = s.clone();
            svc.name = name.text().trim().to_string();
            if svc.name.is_empty() {
                svc.name = "Courier service".into();
            }
            svc.assignee = assignee.text().trim().to_string();
            svc.description = description.text().trim().to_string();
            svc.pricing = cr::Pricing {
                base: amount(&base),
                per_jump: amount(&per_jump),
                per_m3: amount(&per_m3),
                per_m3_jump: amount(&per_m3_jump),
                collateral_rate: amount(&coll) / 100.0,
                minimum: amount(&minimum),
                round_to: amount(&round_to),
            };
            svc.max_volume = opt_amount(&max_volume);
            svc.max_collateral = opt_amount(&max_collateral);
            svc.max_jumps = max_jumps.text().trim().parse::<usize>().ok();
            svc.route = RoutePref::ALL.get(route.selected() as usize).copied().unwrap_or_default();
            svc.high_sec_only = high_sec.get();
            svc.days_to_complete = days.text().trim().parse::<i64>().ok().filter(|d| *d > 0).unwrap_or(3);
            svc.expiration_days = expiry.text().trim().parse::<i64>().ok().filter(|d| *d > 0).unwrap_or(7);
            svc.pickups = pickups.borrow().clone();
            svc.deliveries = deliveries.borrow().clone();
            svc.flat_routes = flats.borrow().clone();
            let mut doc = cr::load_doc();
            match doc.services.iter_mut().find(|x| x.id == svc.id) {
                Some(x) => *x = svc.clone(),
                None => doc.services.push(svc.clone()),
            }
            cr::save_doc(ctx.core(), &doc);
            ctx.remember(SERVICE_KEY, svc.id.as_str());
            win.close();
            ctx.rebuild();
        });
    }
    actions.append(&cancel);
    actions.append(&save);
    win.present();
}
