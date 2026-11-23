//! The Buyback tab, for both ends of a buyback: checking what one should pay for loot, and running one.
//!
//! - Programs: the buyback programs the user defines (hub, price basis, rates, rules per market group or type,
//!   hauling, who gets the contracts);
//! - Appraise: pasted items against a program, with the contract to make;
//! - Queue: item exchanges waiting on the user's characters and corporations checked against a program, and the
//!   user's own open sales to other buybacks;
//! - History: what was bought as a buyback and sold to buybacks, from the contract flows.
//!
//! The math is in `podstack_core::features::buyback`.

mod appraise;
mod history;
mod programs;
mod queue;

use super::Tab;
use crate::ctx::{Ctx, Page};
use crate::rt;
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::features::buyback::{self as bb, BuybackDoc, Program};
use podstack_core::market::BestPrices;
use std::collections::HashMap;

const CSS: &str = "
.bb-paste { font-family: monospace; padding: 12px; border-radius: 12px; background-color: @surface_container_highest; }
.bb-paste text { background-color: transparent; }
.bb-instructions { font-family: monospace; padding: 12px 16px; border-radius: 12px; background-color: @surface_container_highest; }
.bb-rule-row { padding: 4px 0; }
.bb-legend-swatch { min-width: 12px; min-height: 12px; border-radius: 3px; }
.bb-legend-paid { background-color: @primary; }
.bb-legend-received { background-color: @tertiary; }
.bb-program-card .card-header { padding-bottom: 0; }
";

const VIEWS: [&str; 4] = ["Programs", "Appraise", "Queue", "History"];
const VIEW_KEY: &str = "contracts.buyback.view";
const PROGRAM_KEY: &str = "contracts.buyback.program";

/// The open view: the one last chosen, or `PODSTACK_BUYBACK_VIEW` (for UI testing), or Programs.
fn current_view(ctx: &Ctx) -> usize {
    ctx.recall(VIEW_KEY)
        .or_else(|| std::env::var("PODSTACK_BUYBACK_VIEW").ok())
        .and_then(|v| VIEWS.iter().position(|l| l.eq_ignore_ascii_case(v.trim())))
        .unwrap_or(0)
}

/// Switches to a view (from a button inside another view).
fn show_view(ctx: &Ctx, view: usize) {
    ctx.remember(VIEW_KEY, VIEWS[view]);
    let ctx = ctx.clone();
    gtk::glib::idle_add_local_once(move || ctx.rebuild());
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&super::tab_bar(ctx, Tab::Buyback));
    let view = current_view(ctx);
    {
        let ctx = ctx.clone();
        body.append(&w::segmented(&VIEWS, view, move |i| show_view(&ctx, i)));
    }
    match view {
        1 => {
            body.append(&appraise::view(ctx));
            Page::new("Contracts", body).static_page()
        }
        2 => {
            body.append(&queue::view(ctx));
            Page::new("Contracts", body)
        }
        3 => {
            body.append(&history::view(ctx));
            Page::new("Contracts", body)
        }
        _ => {
            body.append(&programs::view(ctx));
            Page::new("Contracts", body)
        }
    }
}

// ---------- shared by the views ----------

/// The program the views work with: the one last picked, else the first.
fn selected(ctx: &Ctx, doc: &BuybackDoc) -> Option<Program> {
    let id = ctx.recall(PROGRAM_KEY).and_then(|v| v.parse::<u64>().ok());
    id.and_then(|id| doc.get(id)).or_else(|| doc.programs.first()).cloned()
}

fn select(ctx: &Ctx, id: u64) {
    ctx.remember(PROGRAM_KEY, id.to_string());
}

/// A dropdown of the programs; `on_pick` gets the one chosen.
fn program_dropdown(ctx: &Ctx, doc: &BuybackDoc, on_pick: impl Fn(Program) + 'static) -> gtk::DropDown {
    let names: Vec<&str> = doc.programs.iter().map(|p| p.name.as_str()).collect();
    let current = selected(ctx, doc).and_then(|p| doc.programs.iter().position(|x| x.id == p.id)).unwrap_or(0);
    let dd = w::dropdown(&names, current as u32);
    let (ctx, programs) = (ctx.clone(), doc.programs.clone());
    dd.connect_selected_notify(move |dd| {
        if let Some(p) = programs.get(dd.selected() as usize) {
            select(&ctx, p.id);
            on_pick(p.clone());
        }
    });
    dd
}

/// Shown when there's no program to work with.
fn no_programs(ctx: &Ctx) -> gtk::Box {
    let b = w::empty_state("price_check", "No buyback programs", "Set up a program (the rates of a buyback you sell to, or of one you run) to appraise items and check contracts.");
    let add = w::button_icon("add", "New program", w::Btn::Filled);
    add.set_halign(gtk::Align::Center);
    let ctx = ctx.clone();
    add.connect_clicked(move |_| {
        ctx.remember(VIEW_KEY, VIEWS[0]);
        programs::edit(&ctx, Program::default());
    });
    b.append(&add);
    b
}

/// Best orders at the program's hub for these types, off the GTK thread.
fn load_prices(ctx: &Ctx, p: &Program, types: Vec<i64>, done: impl FnOnce(HashMap<i64, BestPrices>) + 'static) {
    let core = ctx.core().clone();
    let (_, station, region) = p.hub();
    rt::run(
        async move {
            if types.is_empty() {
                HashMap::new()
            } else {
                core.market.best_prices(&core.esi, region, Some(station), &types, false).await
            }
        },
        done,
    );
}

/// A centered spinner with a line of text, for loading.
fn loading(text: &str) -> gtk::Box {
    let b = w::vbox(12);
    b.set_valign(gtk::Align::Center);
    b.set_halign(gtk::Align::Center);
    b.set_margin_top(32);
    b.set_margin_bottom(32);
    let sp = gtk::Spinner::new();
    sp.start();
    sp.set_size_request(32, 32);
    b.append(&sp);
    b.append(&w::text(text, &["body-medium", "dim"]));
    b
}

/// "90%" from 0.9.
fn pct(fraction: f64) -> String {
    format!("{}%", bb::percent(fraction))
}

/// "+4.2%" / "−3.0%" from a fraction, one decimal.
fn signed_pct(fraction: f64) -> String {
    let v = fraction * 100.0;
    if v.abs() < 0.05 {
        "0.0%".into()
    } else if v > 0.0 {
        format!("+{v:.1}%")
    } else {
        format!("−{:.1}%", v.abs())
    }
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

/// A right-aligned colored number cell.
fn colored(t: &str, class: &str) -> gtk::Widget {
    let l = w::line(t, &["numeric", class]);
    l.set_xalign(1.0);
    l.upcast()
}

fn status_of(v: bb::Verdict) -> w::Status {
    match v {
        bb::Verdict::Accept => w::Status::Good,
        bb::Verdict::Review => w::Status::Warn,
        bb::Verdict::OverAsking => w::Status::Bad,
    }
}

fn line_status(s: bb::LineStatus) -> w::Status {
    match s {
        bb::LineStatus::Accepted => w::Status::Good,
        bb::LineStatus::Refused | bb::LineStatus::NotOnMarket => w::Status::Bad,
        bb::LineStatus::Unpriced | bb::LineStatus::BlueprintCopy => w::Status::Warn,
    }
}

/// A heading with an explanation, between the sections of a view.
fn section(title: &str, sub: Option<&str>) -> gtk::Box {
    let b = w::vbox(2);
    b.set_margin_top(8);
    b.append(&w::text(title, &["title-medium"]));
    if let Some(s) = sub {
        b.append(&w::text(s, &["body-medium", "dim"]));
    }
    b
}
