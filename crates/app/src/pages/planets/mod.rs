//! The Planetary Industry destination, in tabs:
//!
//! - Colonies: every character's colonies, how each is doing (extraction, factories, storage, value per hour) and
//!   each character's PI skills and free planets;
//! - Maintenance: everything to do this week across all characters, most urgent first (restart extractors, pick
//!   up full storage, deliver inputs between planets), checked off as you go;
//! - Chains: every commodity from planet to P4 (pick one to see what it needs and what it feeds), and a calculator
//!   for any product and amount, with factories, raw resources and planet kinds needed;
//! - Profit: what each product earns per factory hour at hub prices after customs taxes;
//! - Finder: systems by the planet kinds they have;
//! - Plans: production plans with planets assigned to characters, and how far the colonies match them.
//!
//! The math lives in `podstack_core::features::planetary`; the static data in `podstack_core::pi`.

pub mod chains;
pub mod colonies;
pub mod common;
pub mod finder;
pub mod maintenance;
pub mod plans;
pub mod profit;

pub(crate) use colonies::colonies_view;

use crate::ctx::{Ctx, Page, Route};
use crate::widgets as w;
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::features::planetary as pl;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Colonies,
    Maintenance,
    Chains,
    Profit,
    Finder,
    Plans,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Colonies, Tab::Maintenance, Tab::Chains, Tab::Profit, Tab::Finder, Tab::Plans];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Colonies => "Colonies",
            Tab::Maintenance => "Maintenance",
            Tab::Chains => "Chains",
            Tab::Profit => "Profit",
            Tab::Finder => "Finder",
            Tab::Plans => "Plans",
        }
    }
}

const TAB_KEY: &str = "planets.tab";

/// The open tab: the one last chosen, or `PODSTACK_PI_TAB` (for UI testing), or Colonies.
fn current(ctx: &Ctx) -> Tab {
    let name = ctx.recall(TAB_KEY).or_else(|| std::env::var("PODSTACK_PI_TAB").ok()).unwrap_or_default();
    Tab::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name.trim())).unwrap_or(Tab::Colonies)
}

pub fn build(ctx: &Ctx) -> Page {
    match current(ctx) {
        Tab::Colonies => colonies::build(ctx),
        Tab::Maintenance => maintenance::build(ctx),
        Tab::Chains => chains::build(ctx),
        Tab::Profit => profit::build(ctx),
        Tab::Finder => finder::build(ctx),
        Tab::Plans => plans::build(ctx),
    }
}

/// Opens a tab of the page, from anywhere in the app.
#[allow(dead_code)] // for links between the tabs
pub fn open(ctx: &Ctx, tab: Tab) {
    ctx.remember(TAB_KEY, tab.label());
    ctx.navigate(Route::Planets);
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

/// Colonies needing attention now (expired extractors or critical storage), across all characters.
pub fn nav_badge(core: &podstack_core::Core) -> Option<u32> {
    let s = pl::load().settings;
    let now = Utc::now();
    let n = core
        .characters()
        .iter()
        .flat_map(|c| c.planets.iter().flat_map(|p| p.colonies.clone()))
        .filter(|col| pl::report(col, now, &s).health <= pl::Health::Critical)
        .count();
    Some(n as u32)
}
