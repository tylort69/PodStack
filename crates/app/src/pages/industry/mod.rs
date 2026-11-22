//! The Industry destination, in tabs:
//!
//! - Jobs: every character's jobs and how many slots each has free;
//! - Blueprints: every character's blueprints, and any blueprint in the game;
//! - Calculator: one job worked out (materials, time, job cost with the system's cost index, invention, profit);
//! - Planner: projects that build anything (a list, a fit, a fleet of capitals) down to raw materials, with every
//!   reaction, component, invention and purchase, split into jobs across the characters' slots;
//! - Facilities: the stations and structures jobs run in (structure, rigs, tax), and the rules the math uses;
//! - Systems: every system's cost indices, to find a cheap place to build.
//!
//! The math lives in `podstack_core::features::manufacturing`; the static data in `podstack_core::blueprints`.

pub mod blueprints;
pub mod calculator;
pub mod common;
pub mod facilities;
pub mod jobs;
pub mod planner;
pub mod systems;

pub(crate) use jobs::{char_cell, icon_url, jobs_view, keep_focus, on_search, place_cell, place_sub, scope_failed, type_cell};

use crate::ctx::{Ctx, Page, Route};
use crate::widgets as w;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Jobs,
    Blueprints,
    Calculator,
    Planner,
    Facilities,
    Systems,
}

impl Tab {
    pub const ALL: [Tab; 6] = [Tab::Jobs, Tab::Blueprints, Tab::Calculator, Tab::Planner, Tab::Facilities, Tab::Systems];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Jobs => "Jobs",
            Tab::Blueprints => "Blueprints",
            Tab::Calculator => "Calculator",
            Tab::Planner => "Planner",
            Tab::Facilities => "Facilities",
            Tab::Systems => "Systems",
        }
    }
}

const TAB_KEY: &str = "industry.tab";

/// The open tab: the one last chosen, or `PODSTACK_INDUSTRY_TAB` (for UI testing), or Jobs.
fn current(ctx: &Ctx) -> Tab {
    let name = ctx.recall(TAB_KEY).or_else(|| std::env::var("PODSTACK_INDUSTRY_TAB").ok()).unwrap_or_default();
    Tab::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name.trim())).unwrap_or(Tab::Jobs)
}

pub fn build(ctx: &Ctx) -> Page {
    match current(ctx) {
        Tab::Jobs => jobs::build(ctx),
        Tab::Blueprints => blueprints::build(ctx),
        Tab::Calculator => calculator::build(ctx),
        Tab::Planner => planner::build(ctx),
        Tab::Facilities => facilities::build(ctx),
        Tab::Systems => systems::build(ctx),
    }
}

/// Opens a tab of the Industry page, from anywhere in the app.
#[allow(dead_code)] // used through common::open_in_*
pub fn open(ctx: &Ctx, tab: Tab) {
    ctx.remember(TAB_KEY, tab.label());
    ctx.navigate(Route::Industry);
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

/// Jobs ready to deliver, across all characters.
pub fn nav_badge(core: &podstack_core::Core) -> Option<u32> {
    jobs::nav_badge(core)
}
