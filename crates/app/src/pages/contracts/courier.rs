//! The Courier tab, in four views:
//!
//! - Active: couriers being hauled (time left, collateral, route), couriers shipped and their state, couriers
//!   assigned to the user, and what each hauler carries;
//! - History: what hauling and shipping paid over a period, per character and per route (from the contracts
//!   archive, so it reaches past ESI's 30 days);
//! - Quote: courier services' rate cards and a calculator that prices a contract and writes out what to create;
//! - Finder: a region's public couriers, filtered to what the hauler can carry and wants to fly.
//!
//! The math lives in `podstack_core::features::courier`.

mod active;
mod finder;
mod history;
mod quote;

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::security_label;
use crate::widgets as w;
use chrono::{DateTime, Utc};
use gtk::prelude::*;
use podstack_core::domains::contracts::Contract;
use podstack_core::features::contracts::{Mine, Party};
use podstack_core::features::courier::{self as cr, RouteSummary};
use podstack_core::fmt;
use podstack_core::sde::{RoutePref, MAP};
use podstack_core::universe::Place;

const CSS: &str = "
.courier-reward { font-size: 28px; font-weight: 600; }
.courier-text { font-family: monospace; font-size: 12px; padding: 10px 12px; }
.courier-text text { background-color: transparent; }
.courier-chip { padding: 2px 4px 2px 10px; }
.courier-problem { padding: 2px 0; }
.courier-preset { padding: 2px 10px; min-height: 28px; }
";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Active,
    History,
    Quote,
    Finder,
}

impl View {
    const ALL: [View; 4] = [View::Active, View::History, View::Quote, View::Finder];
    fn label(self) -> &'static str {
        match self {
            View::Active => "Active",
            View::History => "History",
            View::Quote => "Quote",
            View::Finder => "Finder",
        }
    }
}

const VIEW_KEY: &str = "contracts.courier.view";

/// The open view: the one last chosen, or `PODSTACK_COURIER_VIEW` (for UI testing), or Active.
fn current(ctx: &Ctx) -> View {
    let name = ctx.recall(VIEW_KEY).or_else(|| std::env::var("PODSTACK_COURIER_VIEW").ok()).unwrap_or_default();
    View::ALL.into_iter().find(|v| v.label().eq_ignore_ascii_case(name.trim())).unwrap_or(View::Active)
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    match current(ctx) {
        View::Active => active::build(ctx),
        View::History => history::build(ctx),
        View::Quote => quote::build(ctx),
        View::Finder => finder::build(ctx),
    }
}

/// The Contracts tabs, then the view switch and (where routes are shown) the route preference.
fn header(ctx: &Ctx, view: View) -> gtk::Box {
    let b = w::vbox(12);
    b.append(&tab_bar(ctx, Tab::Courier));
    let row = w::hbox(12);
    let labels: Vec<&str> = View::ALL.iter().map(|v| v.label()).collect();
    let selected = View::ALL.iter().position(|v| *v == view).unwrap_or(0);
    {
        let ctx = ctx.clone();
        row.append(&w::segmented(&labels, selected, move |i| {
            ctx.remember(VIEW_KEY, View::ALL[i].label());
            // once the toggle handler has returned: the rebuild replaces the button being clicked
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        }));
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    // the quote follows each service's own preference
    if view != View::Quote {
        row.append(&w::line("Routes", &["label-medium", "dim"]));
        let labels: Vec<&str> = RoutePref::ALL.iter().map(|p| p.label()).collect();
        let now = cr::load_doc().route;
        let dd = w::dropdown(&labels, RoutePref::ALL.iter().position(|p| *p == now).unwrap_or(0) as u32);
        dd.set_tooltip_text(Some("How jumps and security are worked out, like the autopilot's route setting"));
        let ctx = ctx.clone();
        dd.connect_selected_notify(move |d| {
            let mut doc = cr::load_doc();
            doc.route = RoutePref::ALL.get(d.selected() as usize).copied().unwrap_or_default();
            cr::save_doc(ctx.core(), &doc);
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        });
        row.append(&dd);
    }
    b.append(&row);
    b
}

// ---------- cells shared by the views ----------

fn place_name(p: &Option<Place>) -> String {
    p.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "Unknown".into())
}

fn system_of(p: &Option<Place>) -> Option<i64> {
    p.as_ref().and_then(|p| p.system_id)
}

fn system_name(id: Option<i64>) -> String {
    id.and_then(|id| MAP.system(id)).map(|s| s.name.clone()).unwrap_or_else(|| "Unknown".into())
}

/// A location's system with its security, the full name as tooltip.
fn place_cell(p: &Option<Place>) -> gtk::Widget {
    let b = w::hbox(6);
    match system_of(p) {
        Some(sys) => {
            let sec = security_label(sys);
            sec.set_width_chars(3);
            b.append(&sec);
            b.append(&w::line(&system_name(Some(sys)), &[]));
        }
        None => {
            b.append(&w::line(if p.is_some() { "Unknown structure" } else { "Unknown" }, &["dim"]));
        }
    }
    b.set_tooltip_text(Some(&place_name(p)));
    b.upcast()
}

/// Sorts places by system name.
fn place_key(p: &Option<Place>) -> String {
    system_name(system_of(p))
}

/// Jumps, with the route in words as tooltip.
fn jumps_cell(r: Option<&RouteSummary>) -> gtk::Widget {
    match r {
        Some(r) => {
            let l = w::line(&r.jumps.to_string(), &["numeric"]);
            l.set_xalign(1.0);
            l.set_tooltip_text(Some(&r.describe()));
            l.upcast()
        }
        None => {
            let l = w::line("–", &["dim"]);
            l.set_xalign(1.0);
            l.set_tooltip_text(Some("No known route: a structure nobody can look up, or no stargates"));
            l.upcast()
        }
    }
}

/// The lowest security on the route, colored like EVE.
fn min_sec_cell(r: Option<&RouteSummary>) -> gtk::Widget {
    let Some(r) = r else { return w::line("", &[]).upcast() };
    let class = if r.min_security >= 0.5 {
        "success-text"
    } else if r.min_security > 0.0 {
        "warning-text"
    } else {
        "error-text"
    };
    let l = w::line(&fmt::security(r.min_security), &[class, "numeric"]);
    l.set_tooltip_text(Some(&r.describe()));
    l.upcast()
}

fn time_left_text(due: DateTime<Utc>, now: DateTime<Utc>) -> (String, &'static str) {
    let left = due - now;
    if left <= chrono::Duration::zero() {
        ("Overdue".into(), "error-text")
    } else if left < chrono::Duration::hours(6) {
        (fmt::duration(left, 2), "error-text")
    } else if left < chrono::Duration::hours(24) {
        (fmt::duration(left, 2), "warning-text")
    } else {
        (fmt::duration(left, 2), "")
    }
}

/// A countdown to `due` that keeps counting while shown; red under 6 hours, amber under a day. `over` is what an
/// expired one says ("Overdue" for deliveries, "Expired" for offers).
fn countdown_cell(due: DateTime<Utc>, over: &'static str) -> gtk::Widget {
    let l = w::line("", &["numeric"]);
    l.set_xalign(1.0);
    l.set_tooltip_text(Some(&format!("{} (EVE time {})", fmt::datetime(due), fmt::eve_time(due))));
    let set = {
        let l = l.clone();
        move || {
            let (t, class) = time_left_text(due, Utc::now());
            for c in ["error-text", "warning-text"] {
                l.remove_css_class(c);
            }
            if !class.is_empty() {
                l.add_css_class(class);
            }
            l.set_text(if t == "Overdue" { over } else { &t });
        }
    };
    set();
    w::ticker(&l, 30, set);
    l.upcast()
}

fn m3(v: f64) -> String {
    format!("{} m³", fmt::int(v.round() as i64))
}

fn party_name(mine: &Mine, p: Party) -> String {
    mine.name(p)
}

/// The contract dialog's chevron, for the last column.
fn details_cell(ctx: &Ctx, c: &Contract) -> gtk::Widget {
    let b = w::icon_button("chevron_right", "Contract details");
    let (ctx, c) = (ctx.clone(), c.clone());
    b.connect_clicked(move |_| super::common::open_contract(&ctx, &c));
    b.upcast()
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// Sets a suggestion entry's text without its suggestions popping up: the entry reports a change only after its
/// search delay, so for this one change the delay is dropped and the popover closed right after.
fn set_quietly(e: &gtk::SearchEntry, text: &str) {
    let delay = e.search_delay();
    e.set_search_delay(0);
    e.set_text(text);
    let e = e.clone();
    gtk::glib::idle_add_local_once(move || {
        e.set_search_delay(delay);
        let mut child = e.first_child();
        while let Some(c) = child {
            if let Some(p) = c.downcast_ref::<gtk::Popover>() {
                p.popdown();
            }
            child = c.next_sibling();
        }
    });
}

/// Amounts as typed back into an entry: 1500000 → "1,500,000"; 0 → empty.
fn amount_text(v: f64) -> String {
    if v == 0.0 {
        String::new()
    } else if v.fract() == 0.0 {
        fmt::decimal(v, 0)
    } else {
        fmt::decimal(v, 2)
    }
}

/// An entry's amount ("1.5m", "250k"); None when empty or unreadable, which means "no limit".
fn opt_amount(e: &gtk::Entry) -> Option<f64> {
    let t = e.text();
    if t.trim().is_empty() {
        None
    } else {
        crate::pages::industry::form::parse_amount(&t).filter(|v| *v > 0.0)
    }
}
