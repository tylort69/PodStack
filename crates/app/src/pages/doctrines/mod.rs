//! The Doctrines destination, in tabs:
//!
//! - Doctrines (`library`): groups (and, when wanted, subgroups such as SIGs inside them) holding doctrines, each with the fits
//!   it accepts; import and export of fits and whole groups; who of the characters can fly each doctrine and fit,
//!   and the skill plan for those who can't yet;
//! - Pilots: every character with the doctrine fits it can fly, and what's closest;
//! - Ships: how many characters can fly each class of hull (dreadnoughts, battleships, …), by race and by hull.
//!
//! The data and checks live in `podstack_core::features::doctrines`; fits are read with `features::fitting`.

pub mod common;
pub mod library;
pub mod pilots;
pub mod ships;

use crate::ctx::{Ctx, Page, Route};
use crate::widgets as w;
use gtk::prelude::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Doctrines,
    Pilots,
    Ships,
}

impl Tab {
    pub const ALL: [Tab; 3] = [Tab::Doctrines, Tab::Pilots, Tab::Ships];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Doctrines => "Doctrines",
            Tab::Pilots => "Pilots",
            Tab::Ships => "Ships",
        }
    }
}

const TAB_KEY: &str = "doctrines.tab";

/// The open tab: the one last chosen, or `PODSTACK_DOCTRINES_TAB` (for UI testing), or Doctrines.
fn current(ctx: &Ctx) -> Tab {
    let name = ctx.recall(TAB_KEY).or_else(|| std::env::var("PODSTACK_DOCTRINES_TAB").ok()).unwrap_or_default();
    Tab::ALL.into_iter().find(|t| t.label().eq_ignore_ascii_case(name.trim())).unwrap_or(Tab::Doctrines)
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(common::CSS);
    match current(ctx) {
        Tab::Doctrines => library::build(ctx),
        Tab::Pilots => pilots::build(ctx),
        Tab::Ships => ships::build(ctx),
    }
}

/// Opens a tab of the page, from anywhere in the app.
#[allow(dead_code)] // for links between the tabs
pub fn open(ctx: &Ctx, tab: Tab) {
    ctx.remember(TAB_KEY, tab.label());
    ctx.navigate(Route::Doctrines);
    ctx.rebuild();
}

/// The tabs, for the top of each tab's page.
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
    row
}

/// A tab not built yet.
#[allow(dead_code)]
pub(crate) fn placeholder(ctx: &Ctx, tab: Tab) -> Page {
    let body = w::page_body();
    body.append(&tab_bar(ctx, tab));
    body.append(&w::empty_state("military_tech", tab.label(), "Coming soon."));
    Page::new("Doctrines", body)
}
