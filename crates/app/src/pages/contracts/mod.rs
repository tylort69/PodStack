//! The Contracts destination, in tabs:
//!
//! - Overview: what the characters spent and made on contracts over a period, per character and overall, by kind
//!   (bought, sold, hauled, shipped), by counterparty, by item, and over time; ISK tied up in open contracts;
//! - Contracts: every contract ever seen (PodStack keeps them past ESI's 30 days) with filters, tags, notes and
//!   CSV export;
//! - Courier: couriers being hauled and shipped with deadlines and collateral, hauling history, a quote calculator
//!   for courier services and a finder for public courier contracts;
//! - Buyback: buyback programs (rates per item group), appraisals with the offer to contract for, incoming
//!   contracts checked against the program, and what the buyback has made;
//! - Deals: public item exchanges and auctions in a region compared with the market.
//!
//! History, flows and totals live in `podstack_core::features::contracts`; public contracts in
//! `podstack_core::public_contracts`.

pub mod browse;
pub mod buyback;
pub mod common;
pub mod courier;
pub mod deals;
pub mod overview;

pub(crate) use common::{contracts_section, CSS};

use crate::ctx::{Ctx, Page, Route};
use crate::widgets as w;
use gtk::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Overview,
    Contracts,
    Courier,
    Buyback,
    Deals,
}

impl Tab {
    pub const ALL: [Tab; 5] = [Tab::Overview, Tab::Contracts, Tab::Courier, Tab::Buyback, Tab::Deals];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Contracts => "Contracts",
            Tab::Courier => "Courier",
            Tab::Buyback => "Buyback",
            Tab::Deals => "Deals",
        }
    }
}

const TAB_KEY: &str = "contracts.tab";

/// The open tab: the one last chosen, or `PODSTACK_CONTRACTS_TAB` (for UI testing), or Overview.
fn current(ctx: &Ctx) -> Tab {
    let name = ctx.recall(TAB_KEY).or_else(|| std::env::var("PODSTACK_CONTRACTS_TAB").ok()).unwrap_or_default();
    Tab::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name.trim())).unwrap_or(Tab::Overview)
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    match current(ctx) {
        Tab::Overview => overview::build(ctx),
        Tab::Contracts => browse::build(ctx),
        Tab::Courier => courier::build(ctx),
        Tab::Buyback => buyback::build(ctx),
        Tab::Deals => deals::build(ctx),
    }
}

/// Opens a tab of the page, from anywhere in the app.
#[allow(dead_code)] // for links between the tabs
pub fn open(ctx: &Ctx, tab: Tab) {
    ctx.remember(TAB_KEY, tab.label());
    ctx.navigate(Route::Contracts);
    ctx.rebuild();
}

/// The tabs (and the settings button), for the top of each tab's page.
pub fn tab_bar(ctx: &Ctx, current: Tab) -> gtk::Box {
    let labels: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
    let selected = Tab::ALL.iter().position(|t| *t == current).unwrap_or(0);
    let row = w::hbox(12);
    let ctx2 = ctx.clone();
    row.append(&w::segmented(&labels, selected, move |i| {
        ctx2.remember(TAB_KEY, Tab::ALL[i].label());
        // once the toggle handler has returned: the rebuild replaces the button being clicked
        let ctx = ctx2.clone();
        gtk::glib::idle_add_local_once(move || ctx.rebuild());
    }));
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    row.append(&spacer);
    row.append(&common::settings_button(ctx));
    row
}

/// A tab not built yet.
#[allow(dead_code)]
pub(crate) fn placeholder(ctx: &Ctx, tab: Tab) -> Page {
    let body = w::page_body();
    body.append(&tab_bar(ctx, tab));
    body.append(&w::empty_state("assignment", tab.label(), "Coming soon."));
    Page::new("Contracts", body)
}
