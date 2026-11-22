//! Market → Profits: trade profits matched from wallet transactions, the item warehouse (bought and
//! unsold), and every character's transactions.

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use gtk::prelude::*;

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Profits));
    Page::new("Market", body)
}
