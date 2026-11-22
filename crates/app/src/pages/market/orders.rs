//! Market → Orders: every character's buy and sell orders on one page, what's open (and running out) and what ended
//! in the last 90 days, and a price check of the open ones against their region's order book: who is outbid and the
//! price to set to lead again (one tick better, copied to the clipboard for the in-game market window).

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::{character_cell, orders_section, scope_failed, set_child, type_cell, CSS};
use crate::widgets::{self as w, Status, Table};
use chrono::{Local, Utc};
use gtk::prelude::*;
use podstack_core::domains::wallet::MarketOrder;
use podstack_core::features::comms;
use podstack_core::features::trade::{self, Competition};
use podstack_core::market::BookOrder;
use podstack_core::{fmt, Character};
use std::collections::{HashMap, HashSet};
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

    let names: Rc<HashMap<i64, String>> = Rc::new(chars.iter().map(|c| (c.id, c.name())).collect());
    // PODSTACK_ORDERS_VIEW=check opens the price check (for UI testing)
    let price_check = ctx.recall(VIEW_KEY).or_else(|| std::env::var("PODSTACK_ORDERS_VIEW").ok()).as_deref() == Some("check");
    let bar = w::hbox(12);
    {
        let ctx = ctx.clone();
        bar.append(&w::segmented(&["All orders", "Price check"], price_check as usize, move |i| {
            ctx.remember(VIEW_KEY, if i == 1 { "check" } else { "all" });
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        }));
    }
    body.append(&bar);
    if price_check {
        let active: Vec<(i64, MarketOrder)> = all.into_iter().filter(|(_, o)| o.is_active()).collect();
        body.append(&price_check_section(ctx, &bar, active, names));
    } else {
        body.append(&orders_section(ctx, "market_orders", all, Some(names), None));
    }
    Page::new("Market", body).no_scroll()
}

const VIEW_KEY: &str = "market.orders.view";

struct Checked {
    character: i64,
    order: MarketOrder,
    /// None when the region's order book didn't load.
    competition: Option<Competition>,
}

/// Loads the order book of every open order's item and region, and shows how each order stands. `bar` gets a refresh
/// button.
fn price_check_section(ctx: &Ctx, bar: &gtk::Box, orders: Vec<(i64, MarketOrder)>, names: Rc<HashMap<i64, String>>) -> gtk::Box {
    let section = w::vbox(12);
    section.set_vexpand(true);
    if orders.is_empty() {
        section.append(&w::empty_state("price_check", "No open orders", "Open buy and sell orders are checked against the market here."));
        return section;
    }
    let (scroller, holder) = w::async_holder();
    section.append(&scroller);

    let load: Rc<dyn Fn(bool)> = {
        let (ctx, holder) = (ctx.clone(), holder.clone());
        Rc::new(move |force: bool| {
            let spinner = w::vbox(12);
            spinner.set_valign(gtk::Align::Center);
            spinner.set_vexpand(true);
            let sp = gtk::Spinner::new();
            sp.start();
            sp.set_size_request(32, 32);
            spinner.append(&sp);
            spinner.append(&w::text(&format!("Checking {} orders against the market…", orders.len()), &["dim"]));
            set_child(&holder, &spinner);
            let core = ctx.core().clone();
            let orders = orders.clone();
            let (ctx, holder, names) = (ctx.clone(), holder.clone(), names.clone());
            crate::rt::run(
                async move {
                    let mut pairs: Vec<(i64, i64)> = orders.iter().map(|(_, o)| (o.region_id, o.type_id)).collect();
                    pairs.sort();
                    pairs.dedup();
                    let mut books: HashMap<(i64, i64), Arc<Vec<BookOrder>>> = HashMap::new();
                    for chunk in pairs.chunks(8) {
                        let futs = chunk.iter().map(|&(r, t)| core.market.orders(&core.esi, r, t, force));
                        for (pair, result) in chunk.iter().zip(futures::future::join_all(futs).await) {
                            match result {
                                Ok(b) => {
                                    books.insert(*pair, b);
                                }
                                Err(err) => tracing::warn!("price check {pair:?}: {err}"),
                            }
                        }
                    }
                    let own: HashSet<i64> = orders.iter().map(|(_, o)| o.order_id).collect();
                    orders
                        .into_iter()
                        .map(|(character, order)| {
                            let competition = books.get(&(order.region_id, order.type_id)).map(|b| trade::competition(&order, b, &own));
                            Checked { character, order, competition }
                        })
                        .collect::<Vec<_>>()
                },
                move |rows| set_child(&holder, &checked_view(&ctx, rows, names)),
            );
        })
    };
    let refresh = w::button_icon("refresh", "Check again", w::Btn::Tonal);
    {
        let load = load.clone();
        refresh.connect_clicked(move |_| load(true));
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    bar.append(&refresh);
    load(false);
    section
}

fn checked_view(ctx: &Ctx, mut rows: Vec<Checked>, names: Rc<HashMap<i64, String>>) -> gtk::Box {
    let v = w::vbox(12);
    v.set_vexpand(true);
    let outbid = rows.iter().filter(|r| r.competition.as_ref().is_some_and(|c| c.outbid)).count();
    let leading = rows.iter().filter(|r| r.competition.as_ref().is_some_and(|c| !c.outbid)).count();
    let failed = rows.iter().filter(|r| r.competition.is_none()).count();
    let outbid_value: f64 = rows.iter().filter(|r| r.competition.as_ref().is_some_and(|c| c.outbid)).map(|r| r.order.remaining_value()).sum();
    let outbid_tile = w::stat_tile("Outbid", &outbid.to_string(), Some(&format!("{} ISK of orders", fmt::isk_short(outbid_value))));
    if outbid > 0 {
        outbid_tile.add_css_class("accent");
    }
    v.append(&w::tile_row(vec![
        outbid_tile,
        w::stat_tile("Leading", &leading.to_string(), Some("Best price where they trade")),
        w::stat_tile("Checked", &Local::now().format("%H:%M").to_string(), Some(&if failed > 0 { format!("{failed} couldn't be checked") } else { "Order books refresh every 5 minutes".into() })),
    ]));
    v.append(&w::line(
        "Sell orders compete with sell orders at the same station; buy orders with any buy order that can buy there. Your other characters' orders don't count. Activate a row to copy its suggested price.",
        &["body-small", "dim"],
    ));
    // outbid first, then by value at stake
    rows.sort_by(|a, b| {
        let ob = |r: &Checked| r.competition.as_ref().is_some_and(|c| c.outbid);
        ob(b).cmp(&ob(a)).then(b.order.remaining_value().total_cmp(&a.order.remaining_value()))
    });
    let ctx2 = ctx.clone();
    let n2 = names.clone();
    let table = Table::new(rows)
        .widget_sorted("Item", |r| type_cell(r.order.type_id, &r.order.name), |a, b| a.order.name.to_lowercase().cmp(&b.order.name.to_lowercase()))
        .expand()
        .width(240)
        .widget_sorted(
            "Status",
            |r| {
                let b = w::hbox(6);
                b.append(&if r.order.is_buy_order { w::status("Buy", Status::Warn) } else { w::status("Sell", Status::Info) });
                b.append(&match &r.competition {
                    None => w::status("Unknown", Status::Neutral),
                    Some(c) if c.outbid => w::status(&format!("Outbid · #{}", c.rank), Status::Bad),
                    Some(c) if c.competitors == 0 => w::status("Alone", Status::Good),
                    Some(_) => w::status("Leading", Status::Good),
                });
                b.upcast()
            },
            |a, b| {
                let k = |r: &Checked| r.competition.as_ref().map(|c| (c.outbid, c.rank));
                k(a).cmp(&k(b))
            },
        )
        .width(180)
        .widget_sorted(
            "Character",
            move |r| character_cell(r.character, names.get(&r.character).map(|s| s.as_str()).unwrap_or("?")),
            move |a, b| n2.get(&a.character).cmp(&n2.get(&b.character)),
        )
        .width(170)
        .num("Your price", |r| fmt::decimal(r.order.price, 2), |r| r.order.price)
        .num("Best rival", |r| r.competition.as_ref().and_then(|c| c.best).map(|p| fmt::decimal(p, 2)).unwrap_or_else(|| "—".into()), |r| {
            r.competition.as_ref().and_then(|c| c.best).unwrap_or(0.0)
        })
        .num("Difference", |r| difference(r).map(|d| format!("{:+.2}%", d * 100.0)).unwrap_or_default(), |r| difference(r).unwrap_or(0.0))
        .widget_sorted(
            "Suggested",
            move |r| suggested_cell(&ctx2, r),
            |a, b| a.competition.as_ref().map(|c| c.suggested).unwrap_or(0.0).total_cmp(&b.competition.as_ref().map(|c| c.suggested).unwrap_or(0.0)),
        )
        .num("Remaining", |r| fmt::int(r.order.volume_remain), |r| r.order.volume_remain as f64)
        .num("Rivals", |r| r.competition.as_ref().map(|c| c.competitors.to_string()).unwrap_or_default(), |r| {
            r.competition.as_ref().map(|c| c.competitors as f64).unwrap_or(0.0)
        })
        .text("Where", |r| if !r.order.location.name.is_empty() { r.order.location.name.clone() } else { r.order.region.clone() })
        .width(240);
    let ctx3 = ctx.clone();
    let table = table
        .on_activate(move |r| {
            if let Some(c) = &r.competition {
                w::copy_to_clipboard(&ctx3, &price_text(c.suggested));
            }
        })
        .empty_text("No open orders.")
        .build();
    v.append(&table);
    v
}

/// How far the best rival is from your price, relative to it: positive means you're worse.
fn difference(r: &Checked) -> Option<f64> {
    let best = r.competition.as_ref()?.best?;
    if r.order.price <= 0.0 {
        return None;
    }
    Some(if r.order.is_buy_order { (best - r.order.price) / r.order.price } else { (r.order.price - best) / r.order.price })
}

/// The price as the in-game market window accepts it when pasted.
fn price_text(p: f64) -> String {
    format!("{p:.2}")
}

fn suggested_cell(ctx: &Ctx, r: &Checked) -> gtk::Widget {
    let row = w::hbox(4);
    row.set_halign(gtk::Align::End);
    let Some(c) = &r.competition else { return row.upcast() };
    let changed = c.suggested != r.order.price;
    row.append(&w::line(&fmt::decimal(c.suggested, 2), if changed { &["primary-text"] } else { &["dim"] }));
    let copy = w::icon_button("content_copy", "Copy this price");
    let (ctx, p) = (ctx.clone(), c.suggested);
    copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &price_text(p)));
    row.append(&copy);
    row.upcast()
}
