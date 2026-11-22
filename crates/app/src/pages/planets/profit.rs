//! Planetary Industry → Profit.

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use gtk::prelude::*;

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Profit));
    body.append(&w::empty_state("construction", "Profit", "Coming soon."));
    Page::new("Planetary Industry", body).static_page()
}
