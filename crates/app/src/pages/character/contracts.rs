//! Character tab: what the character spent and made on contracts (30 days and all time, from PodStack's archive,
//! contracts with the user's other characters included), then its contracts, open and completed, with its contract
//! slots.

use crate::ctx::{Ctx, Page};
use crate::pages::contracts::{contracts_section, CSS};
use crate::pages::market_orders::scope_failed;
use crate::widgets as w;
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::features::contracts::{self as ct, Party, Period, Totals};
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

    body.append(&money_tiles(ctx, id));
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

/// Spent, made and net over 30 days and all time.
fn money_tiles(ctx: &Ctx, id: i64) -> gtk::FlowBox {
    let chars = ctx.core().characters();
    let archive = ct::archive_snapshot();
    let all = ct::all_flows(&chars, &archive, ct::settings().value_at_sell);
    let now = Utc::now();
    let mine: Vec<&ct::Flow> = all.iter().filter(|f| f.party == Party::Character(id)).collect();
    let month = Totals::of(mine.iter().copied().filter(|f| Period::Month.contains(f.date, now)));
    let ever = Totals::of(mine.iter().copied());
    let since = archive.since().map(|t| format!("Since {}", fmt::date(t)));
    let tile = |label: &str, v: f64, sub: Option<&str>| {
        let t = w::stat_tile(label, &fmt::isk_short(v), sub);
        t.set_tooltip_text(Some(&fmt::isk(v)));
        t
    };
    // the net is signed and colored: green for a gain, red for a loss
    let net = |label: &str, t: &Totals| {
        let v = t.net();
        let tile = tile(label, v, Some(&format!("{} contracts", fmt::int(t.count as i64))));
        if let Some(l) = tile.first_child().and_then(|c| c.next_sibling()).and_downcast::<gtk::Label>() {
            if v > 0.0 {
                l.set_text(&format!("+{}", fmt::isk_short(v)));
            }
            if v != 0.0 {
                l.add_css_class(if v > 0.0 { "success-text" } else { "error-text" });
            }
        }
        tile
    };
    w::tile_row(vec![
        tile("Spent (30 days)", month.isk_out, None),
        tile("Made (30 days)", month.isk_in, None),
        net("Net (30 days)", &month),
        tile("Spent (all time)", ever.isk_out, since.as_deref()),
        tile("Made (all time)", ever.isk_in, since.as_deref()),
        net("Net (all time)", &ever),
    ])
}
