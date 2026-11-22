//! Character tab: buy and sell orders, like the Orders tab of EVE's market window: what's still open, and what
//! ended in the last 90 days.

use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::{orders_section, scope_failed, CSS};
use crate::widgets as w;
use gtk::prelude::*;
use chrono::Utc;
use podstack_core::features::comms;
use podstack_core::fmt;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    crate::theme::add_css(CSS);
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Market", body) };
    let Some(data) = &c.market_orders else {
        if scope_failed(&c, "market_orders") {
            body.append(&w::scope_notice("esi-markets.read_character_orders.v1"));
        } else {
            body.append(&w::empty_state("storefront", "Market orders not loaded yet", "They appear after the next refresh."));
        }
        return Page::new("Market", body);
    };

    let now = Utc::now();
    let sells: Vec<_> = data.active().filter(|o| !o.is_buy_order).collect();
    let buys: Vec<_> = data.active().filter(|o| o.is_buy_order).collect();
    let sell_value: f64 = sells.iter().map(|o| o.remaining_value()).sum();
    let buy_value: f64 = buys.iter().map(|o| o.remaining_value()).sum();
    let escrow: f64 = buys.iter().map(|o| o.escrow.unwrap_or(0.0)).sum();
    let expiring = data.active().filter(|o| comms::is_order_expiring_soon(o, now)).count();

    let expiring_tile = w::stat_tile("Expiring today", &expiring.to_string(), Some("Under a day left"));
    if expiring > 0 {
        expiring_tile.add_css_class("accent");
    }
    body.append(&w::tile_row(vec![
        w::stat_tile("Sell orders", &sells.len().to_string(), Some(&format!("{} ISK listed", fmt::isk_short(sell_value)))),
        w::stat_tile("Buy orders", &buys.len().to_string(), Some(&format!("{} ISK to buy", fmt::isk_short(buy_value)))),
        w::stat_tile("In escrow", &format!("{} ISK", fmt::isk_short(escrow)), None),
        expiring_tile,
    ]));
    let updated = c.refreshes.get("market_orders").and_then(|r| r.last).map(|t| format!("Updated {}", fmt::relative(t)));
    let orders = data.orders.iter().map(|o| (id, o.clone())).collect();
    body.append(&orders_section(ctx, "char_market", orders, None, updated));
    Page::new("Market", body).no_scroll()
}
