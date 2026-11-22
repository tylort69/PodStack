//! The market orders table and its Open/History filter bar (the Market page's Orders tab and the character Market
//! tab), and small cell helpers other pages share.

use crate::ctx::{CharTab, Ctx, Route};
use crate::widgets::{self as w, Status, Table};
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::domains::wallet::MarketOrder;
use podstack_core::features::comms;
use podstack_core::{config, fmt, Character};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

pub const CSS: &str = "
.comms-filter-bar { padding: 4px 0; }
.comms-type-cell image, .comms-type-cell picture { border-radius: 6px; }
.comms-expiring { color: @warning; font-weight: 500; }
";

pub(crate) fn scope_failed(c: &Character, key: &str) -> bool {
    c.refreshes.get(key).and_then(|r| r.error.as_deref()) == Some("scope")
}

/// Replaces a container's only child.
pub(crate) fn set_child(container: &gtk::Box, child: &impl IsA<gtk::Widget>) {
    while let Some(c) = container.first_child() {
        container.remove(&c);
    }
    container.append(child);
}

/// An item's icon and name.
pub(crate) fn type_cell(type_id: i64, name: &str) -> gtk::Widget {
    let row = w::hbox(10);
    row.add_css_class("comms-type-cell");
    row.append(&w::eve_image(&config::type_icon(type_id, 32), 24));
    row.append(&w::line(name, &[]));
    row.upcast()
}

/// A character's portrait and name.
pub(crate) fn character_cell(id: i64, name: &str) -> gtk::Widget {
    let row = w::hbox(10);
    row.append(&w::avatar(&config::character_portrait(id, 64), 24));
    row.append(&w::line(name, &[]));
    row.upcast()
}

/// A plain label cell (for widget columns sorted by something other than their text).
pub(crate) fn label_cell(t: &str, classes: &[&str]) -> gtk::Widget {
    let l = w::line(t, classes);
    l.upcast()
}

/// Open/History switch, search and the orders table. `names` (character id → name) adds a Character column.
/// `note` is shown beside the search (e.g. when the data was updated).
pub(crate) fn orders_section(
    ctx: &Ctx,
    key: &str,
    orders: Vec<(i64, MarketOrder)>,
    names: Option<Rc<HashMap<i64, String>>>,
    note: Option<String>,
) -> gtk::Box {
    let section = w::vbox(12);
    section.set_vexpand(true);
    let view_key = format!("{key}.view");
    let search_key = format!("{key}.search");
    let history = ctx.recall(&view_key).as_deref() == Some("history");
    let query = ctx.recall(&search_key).unwrap_or_default();

    let orders = Rc::new(orders);
    let open_count = orders.iter().filter(|(_, o)| o.is_active()).count();
    let ended_count = orders.len() - open_count;
    let state = Rc::new(RefCell::new((history, query.clone())));
    let holder = w::vbox(0);
    holder.set_vexpand(true);

    let refill = {
        let (ctx, holder, orders, state, names) = (ctx.clone(), holder.clone(), orders.clone(), state.clone(), names.clone());
        Rc::new(move || {
            let (history, query) = state.borrow().clone();
            let q = query.trim().to_lowercase();
            let rows: Vec<(i64, MarketOrder)> = orders
                .iter()
                .filter(|(_, o)| o.is_active() != history)
                .filter(|(cid, o)| {
                    q.is_empty()
                        || [Some(o.name.as_str()), Some(o.location.name.as_str()), Some(o.region.as_str()), names.as_ref().and_then(|n| n.get(cid)).map(|s| s.as_str())]
                            .into_iter()
                            .flatten()
                            .any(|t| t.to_lowercase().contains(&q))
                })
                .cloned()
                .collect();
            let empty = if !q.is_empty() {
                "Nothing matches."
            } else if history {
                "No orders ended in the last 90 days."
            } else {
                "No open orders."
            };
            set_child(&holder, &orders_table(&ctx, rows, history, names.clone(), empty));
        })
    };

    let bar = w::hbox(12);
    bar.add_css_class("comms-filter-bar");
    let open_label = format!("Open ({open_count})");
    let ended_label = format!("History ({ended_count})");
    {
        let (ctx, state, refill, view_key) = (ctx.clone(), state.clone(), refill.clone(), view_key.clone());
        bar.append(&w::segmented(&[&open_label, &ended_label], history as usize, move |i| {
            ctx.remember(&view_key, if i == 1 { "history" } else { "open" });
            state.borrow_mut().0 = i == 1;
            refill();
        }));
    }
    let search = w::search_entry(if names.is_some() { "Search items, characters, stations or regions" } else { "Search items, stations or regions" });
    search.set_text(&query);
    search.set_max_width_chars(40);
    search.set_hexpand(false);
    search.set_halign(gtk::Align::End);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    if let Some(n) = note {
        let l = w::line(&n, &["body-small", "dim"]);
        l.set_valign(gtk::Align::Center);
        bar.append(&l);
    }
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        search.connect_search_changed(move |s| {
            ctx.remember(&search_key, s.text().as_str());
            state.borrow_mut().1 = s.text().to_string();
            refill();
        });
    }
    bar.append(&search);
    section.append(&bar);
    section.append(&holder);
    refill();
    section
}

/// Market orders. `history`: ended orders (their state and issue date instead of the time left).
pub(crate) fn orders_table(ctx: &Ctx, mut rows: Vec<(i64, MarketOrder)>, history: bool, names: Option<Rc<HashMap<i64, String>>>, empty: &str) -> gtk::Widget {
    let now = Utc::now();
    // already in the default order, so the table's initial sort doesn't move rows (the view would follow the
    // first one down)
    if history {
        rows.sort_by_key(|(_, o)| std::cmp::Reverse(o.issued));
    } else {
        rows.sort_by_key(|(_, o)| comms::order_expires_at(o));
    }
    let mut t = Table::new(rows)
        .widget_sorted("Item", |(_, o)| type_cell(o.type_id, &o.name), |a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
        .expand()
        .widget_sorted(
            "Order",
            move |(_, o)| {
                let b = w::hbox(6);
                b.append(&if o.is_buy_order { w::status("Buy", Status::Warn) } else { w::status("Sell", Status::Info) });
                if history {
                    let (label, tone) = match o.state.as_str() {
                        "filled" => ("Filled", Status::Good),
                        "cancelled" => ("Cancelled", Status::Bad),
                        "expired" => ("Expired", Status::Neutral),
                        other => (other, Status::Neutral),
                    };
                    b.append(&w::status(&comms::title_case(label), tone));
                }
                b.upcast()
            },
            |a, b| a.1.is_buy_order.cmp(&b.1.is_buy_order).then(a.1.state.cmp(&b.1.state)),
        );
    let mut col = 2;
    if let Some(names) = names.clone() {
        let n2 = names.clone();
        t = t.widget_sorted(
            "Character",
            move |(cid, _)| character_cell(*cid, names.get(cid).map(|s| s.as_str()).unwrap_or("?")),
            move |a, b| n2.get(&a.0).cmp(&n2.get(&b.0)),
        );
        col += 1;
    }
    t = t
        .num("Price", |(_, o)| fmt::decimal(o.price, 2), |(_, o)| o.price)
        .num("Quantity", |(_, o)| format!("{} / {}", fmt::int(o.volume_remain), fmt::int(o.volume_total)), |(_, o)| o.volume_remain as f64)
        .num("Value", |(_, o)| fmt::isk_short(o.remaining_value()), |(_, o)| o.remaining_value())
        .text("Range", |(_, o)| if o.is_buy_order { comms::order_range_label(&o.range) } else { String::new() })
        .text("Where", |(_, o)| if !o.location.name.is_empty() { o.location.name.clone() } else { o.region.clone() })
        .width(260);
    col += 5;
    t = if history {
        t.widget_sorted("Issued", |(_, o)| label_cell(&fmt::datetime(o.issued), &["dim"]), |a, b| a.1.issued.cmp(&b.1.issued)).sort_by(col, true)
    } else {
        t.widget_sorted(
            "Expires",
            move |(_, o)| {
                let at = comms::order_expires_at(o);
                let l = w::line(&fmt::duration(at - now, 2), if comms::is_order_expiring_soon(o, now) { &["comms-expiring"] } else { &["dim"] });
                l.set_xalign(1.0);
                l.set_tooltip_text(Some(&fmt::datetime(at)));
                l.upcast()
            },
            |a, b| comms::order_expires_at(&a.1).cmp(&comms::order_expires_at(&b.1)),
        )
        .sort_by(col, false)
    };
    if names.is_some() {
        let ctx = ctx.clone();
        t = t.on_activate(move |(cid, _)| ctx.navigate(Route::Character(*cid, CharTab::Market)));
    }
    t.empty_text(empty).build()
}
