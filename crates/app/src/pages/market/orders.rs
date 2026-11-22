//! Market → Orders: every character's buy and sell orders on one page, what's open (and running out) and what ended
//! in the last 90 days.

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::{orders_section, scope_failed, CSS};
use crate::widgets as w;
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::domains::wallet::MarketOrder;
use podstack_core::features::comms;
use podstack_core::{fmt, Character};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let chars = ctx.core().characters();
    let all = comms::all_orders(&chars);
    let now = Utc::now();
    let active: Vec<&(i64, MarketOrder)> = all.iter().filter(|(_, o)| o.is_active()).collect();
    let sells: Vec<_> = active.iter().filter(|(_, o)| !o.is_buy_order).collect();
    let buys: Vec<_> = active.iter().filter(|(_, o)| o.is_buy_order).collect();
    let sell_value: f64 = sells.iter().map(|(_, o)| o.remaining_value()).sum();
    let escrow: f64 = buys.iter().map(|(_, o)| o.escrow.unwrap_or(0.0)).sum();
    let expiring = active.iter().filter(|(_, o)| comms::is_order_expiring_soon(o, now)).count();
    let missing: Vec<&Arc<Character>> = chars.iter().filter(|c| c.market_orders.is_none()).collect();

    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&tab_bar(ctx, Tab::Orders));
    let expiring_tile = w::stat_tile("Expiring today", &expiring.to_string(), Some("Open orders with under a day left"));
    if expiring > 0 {
        expiring_tile.add_css_class("accent");
    }
    body.append(&w::tile_row(vec![
        w::stat_tile("Sell orders", &sells.len().to_string(), Some(&format!("{} ISK listed", fmt::isk_short(sell_value)))),
        w::stat_tile("Buy orders", &buys.len().to_string(), Some(&format!("{} ISK in escrow", fmt::isk_short(escrow)))),
        expiring_tile,
    ]));

    if !missing.is_empty() {
        let names: Vec<String> = missing.iter().map(|c| c.name()).collect();
        let scope_missing = missing.iter().any(|c| scope_failed(c, "market_orders"));
        let msg = if scope_missing {
            format!("No market orders for {}: log in again with \"Add character\" to grant Read Market Orders.", names.join(", "))
        } else {
            format!("Market orders for {} haven't loaded yet.", names.join(", "))
        };
        body.append(&w::banner(if scope_missing { "lock" } else { "hourglass_empty" }, &msg, w::BannerKind::Notice));
    }

    if chars.is_empty() {
        body.append(&w::empty_state("storefront", "No characters", "Add a character to see its market orders."));
        return Page::new("Market", body).no_scroll();
    }

    let names: HashMap<i64, String> = chars.iter().map(|c| (c.id, c.name())).collect();
    body.append(&orders_section(ctx, "market_orders", all, Some(Rc::new(names)), None));
    Page::new("Market", body).no_scroll()
}
