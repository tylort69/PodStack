//! Planetary Industry → Finder.

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use gtk::prelude::*;

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Finder));
    body.append(&w::empty_state("construction", "Finder", "Coming soon."));
    Page::new("Planetary Industry", body).static_page()
}
