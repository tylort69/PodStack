//! Market → Browser: any item's order book across regions, its price history, and EVE Tycoon-style
//! statistics.

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use gtk::prelude::*;

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Browser));
    Page::new("Market", body)
}
