//! Character tab: the character's contracts, open and completed, with its contract slots.

use crate::ctx::{Ctx, Page};
use crate::pages::contracts::{contracts_section, CSS};
use crate::pages::market_orders::scope_failed;
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::fmt;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    crate::theme::add_css(CSS);
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Contracts", body) };
    let Some(data) = &c.contracts else {
        if scope_failed(&c, "contracts") {
            body.append(&w::scope_notice("esi-contracts.read_character_contracts.v1"));
        } else {
            body.append(&w::empty_state("assignment", "Contracts not loaded yet", "They appear after the next refresh."));
        }
        return Page::new("Contracts", body);
    };

    let (done, open): (Vec<_>, Vec<_>) = data.contracts.iter().map(|k| (id, k.clone())).partition(|(_, k)| k.is_completed());
    let couriers = open.iter().filter(|(_, k)| k.kind == "courier").count();
    let max = c.max_contracts();
    let slots = w::stat_tile("Contract slots", &format!("{} / {max}", data.slots_used), Some("Own open personal contracts"));
    if data.slots_used >= max {
        slots.add_css_class("accent");
    }
    let updated = c.refreshes.get("contracts").and_then(|r| r.last).map(|t| format!("Updated {}", fmt::relative(t)));
    body.append(&w::tile_row(vec![
        w::stat_tile("Open", &open.len().to_string(), updated.as_deref()),
        slots,
        w::stat_tile("Couriers in flight", &couriers.to_string(), None),
        w::stat_tile("Completed", &done.len().to_string(), Some("Last 30 days")),
    ]));
    body.append(&contracts_section(ctx, "char_contracts", open, done));
    Page::new("Contracts", body).no_scroll()
}
