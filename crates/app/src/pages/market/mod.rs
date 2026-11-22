//! The Market destination, in tabs: Browser (any item's order book and history), Orders (every character's orders
//! and how they stand against the competition), Profits (trade profits, warehouse, transactions) and Tools (margin
//! calculator, appraisal). The calculations live in `podstack_core::features::trade` and `podstack_core::market`.

pub mod browser;
pub mod orders;
pub mod profits;
pub mod tools;

use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use podstack_core::features::comms;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Browser,
    Orders,
    Profits,
    Tools,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Browser, Tab::Orders, Tab::Profits, Tab::Tools];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Browser => "Browser",
            Tab::Orders => "Orders",
            Tab::Profits => "Profits",
            Tab::Tools => "Tools",
        }
    }
}

const TAB_KEY: &str = "market.tab";

/// The open tab: the one last chosen, or `PODSTACK_MARKET_TAB` (for UI testing), or Browser.
fn current(ctx: &Ctx) -> Tab {
    let name = ctx.recall(TAB_KEY).or_else(|| std::env::var("PODSTACK_MARKET_TAB").ok()).unwrap_or_default();
    Tab::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name.trim())).unwrap_or(Tab::Browser)
}

pub fn build(ctx: &Ctx) -> Page {
    match current(ctx) {
        Tab::Browser => browser::build(ctx),
        Tab::Orders => orders::build(ctx),
        Tab::Profits => profits::build(ctx),
        Tab::Tools => tools::build(ctx),
    }
}

/// Opens a tab of the Market page, from anywhere in the app.
#[allow(dead_code)] // for links into the market from other pages
pub fn open(ctx: &Ctx, tab: Tab) {
    ctx.remember(TAB_KEY, tab.label());
    ctx.navigate(crate::ctx::Route::Market);
    ctx.rebuild();
}

/// The tabs, for the top of each tab's page.
pub fn tab_bar(ctx: &Ctx, current: Tab) -> gtk::Box {
    let labels: Vec<&str> = Tab::ALL.iter().map(|t| t.label()).collect();
    let selected = Tab::ALL.iter().position(|t| *t == current).unwrap_or(0);
    let ctx = ctx.clone();
    w::segmented(&labels, selected, move |i| {
        ctx.remember(TAB_KEY, Tab::ALL[i].label());
        // once the toggle handler has returned: the rebuild replaces the button being clicked
        let ctx = ctx.clone();
        gtk::glib::idle_add_local_once(move || ctx.rebuild());
    })
}

/// Count shown on the navigation drawer item (None hides the badge).
pub fn nav_badge(core: &podstack_core::Core) -> Option<u32> {
    Some(comms::count_expiring_soon(&core.characters()) as u32)
}
