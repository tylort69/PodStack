//! Buyback → Queue: item exchanges waiting on the user's characters and corporations, checked against a program at
//! live hub prices, and the user's own open sales to other buybacks with what they're worth.

use super::{colored, load_prices, loading, pct, program_dropdown, section, selected, signed_class, signed_pct, status_of};
use crate::ctx::Ctx;
use crate::pages::contracts::common::open_contract;
use crate::pages::market_orders::{set_child, type_cell};
use crate::widgets::{self as w, Table};
use chrono::{DateTime, Utc};
use gtk::prelude::*;
use podstack_core::domains::contracts::Contract;
use podstack_core::features::buyback::{self as bb, Check, Item, Offer, Program, Verdict};
use podstack_core::features::contracts::{self as ct, Mine};
use podstack_core::fmt;
use podstack_core::market::BestPrices;
use std::collections::HashMap;

const ONLY_KEY: &str = "contracts.buyback.queue.only";

#[derive(Clone)]
struct Row {
    c: Contract,
    offer: Offer,
    check: Check,
}

fn items_of(c: &Contract) -> Vec<Item> {
    c.items.iter().filter(|i| i.is_included).map(|i| Item { type_id: i.type_id, quantity: i.quantity, copy: i.is_blueprint_copy() }).collect()
}

/// "Tritanium × 10,000 + 3 more".
fn headline(c: &Contract) -> String {
    if !c.items_loaded {
        return "Items not loaded yet".into();
    }
    let inc: Vec<_> = c.items.iter().filter(|i| i.is_included).collect();
    match inc.split_first() {
        None => "No items".into(),
        Some((first, rest)) => {
            let mut s = if first.name.is_empty() { format!("Type #{}", first.type_id) } else { first.name.clone() };
            if first.quantity > 1 {
                s = format!("{s} × {}", fmt::int(first.quantity));
            }
            if !rest.is_empty() {
                s.push_str(&format!(" + {} more", rest.len()));
            }
            s
        }
    }
}

fn first_type(c: &Contract) -> i64 {
    c.items.iter().find(|i| i.is_included).map(|i| i.type_id).unwrap_or(0)
}

fn from_name(c: &Contract) -> String {
    if c.for_corporation {
        c.issuer_corporation_name.clone()
    } else {
        c.issuer_name.clone()
    }
}

fn row(p: &Program, c: Contract, prices: &HashMap<i64, BestPrices>) -> Row {
    let offer = bb::appraise(p, &items_of(&c), prices);
    let asks_items = c.items.iter().any(|i| !i.is_included);
    let mut check = bb::check(c.price.unwrap_or(0.0), &offer, p.tolerance, asks_items);
    if !c.items_loaded {
        check.verdict = Verdict::Review;
        check.reasons = vec!["Items not loaded yet: they load with the next contracts refresh".into()];
    }
    Row { c, offer, check }
}

fn expires_cell(t: DateTime<Utc>) -> gtk::Widget {
    let now = Utc::now();
    let l = if t > now { w::line(&fmt::duration(t - now, 2), &["dim"]) } else { w::line("Expired", &["error-text"]) };
    l.set_tooltip_text(Some(&fmt::datetime(t)));
    l.upcast()
}

fn items_cell(c: &Contract) -> gtk::Widget {
    let cell = type_cell(first_type(c), &headline(c));
    cell.set_tooltip_text(Some(
        &c.items.iter().filter(|i| i.is_included).take(20).map(|i| format!("{} × {}", i.name, fmt::int(i.quantity))).collect::<Vec<_>>().join("\n"),
    ));
    cell
}

fn open_button(ctx: &Ctx, c: &Contract) -> gtk::Widget {
    let b = w::icon_button("chevron_right", "Contract details");
    let (ctx, c) = (ctx.clone(), c.clone());
    b.connect_clicked(move |_| open_contract(&ctx, &c));
    b.upcast()
}

pub(super) fn view(ctx: &Ctx) -> gtk::Box {
    let col = w::vbox(16);
    let doc = bb::load_doc();
    let Some(p) = selected(ctx, &doc) else {
        col.append(&super::no_programs(ctx));
        return col;
    };
    let chars = ctx.core().characters();
    let mine = Mine::of(&chars);
    let archive = ct::archive_snapshot();
    let now = Utc::now();
    let open = |c: &Contract| c.kind == "item_exchange" && c.status == "outstanding" && c.date_expired > now;
    let only = ctx.recall(ONLY_KEY).as_deref() != Some("no") && p.receiver.is_set();
    let mut incoming = Vec::new();
    let mut selling = Vec::new();
    for e in ct::all_known(&chars, &archive) {
        let c = e.contract;
        if !open(&c) {
            continue;
        }
        let (issuer, _) = ct::sides(&c, &mine);
        let assignee = c.assignee_id.filter(|a| *a > 0);
        let to_program = !only || p.receiver.is_assignee(assignee, c.assignee_name.as_deref());
        if issuer.is_none() && assignee.is_some_and(|a| mine.is_mine(a)) {
            if to_program {
                incoming.push(c);
            }
        } else if issuer.is_some() && assignee.is_some_and(|a| !mine.is_mine(a)) && c.price.unwrap_or(0.0) > 0.0 && c.items.iter().all(|i| i.is_included) && to_program {
            selling.push(c);
        }
    }

    // controls
    let bar = w::hbox(12);
    bar.append(&w::line("Check against", &["body-medium", "dim"]));
    {
        let ctx2 = ctx.clone();
        bar.append(&program_dropdown(ctx, &doc, move |_| {
            // once the dropdown's handler has returned: the rebuild replaces it
            let ctx = ctx2.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        }));
    }
    let terms = w::line(&format!("{} · review up to {} over", p.summary(), pct(p.tolerance)), &["body-small", "dim"]);
    terms.set_hexpand(true);
    bar.append(&terms);
    if p.receiver.is_set() {
        let sw = gtk::Switch::new();
        sw.set_active(only);
        sw.set_valign(gtk::Align::Center);
        let ctx2 = ctx.clone();
        sw.connect_active_notify(move |s| {
            ctx2.remember(ONLY_KEY, if s.is_active() { "yes" } else { "no" });
            let ctx = ctx2.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        });
        bar.append(&w::line(&format!("Only to {}", p.receiver_label()), &["body-medium"]));
        bar.append(&sw);
    }
    col.append(&bar);

    if incoming.is_empty() && selling.is_empty() {
        let who = if only { format!(" to {}", p.receiver_label()) } else { String::new() };
        col.append(&w::empty_state(
            "inbox",
            "Nothing waiting",
            &format!(
                "No open item exchanges{who}. Contracts assigned to your characters or corporations show here to be checked, and your own sales to other buybacks to see what they're worth. Corporation contracts need a director's login with the corporation contracts scope."
            ),
        ));
        return col;
    }

    let holder = w::vbox(16);
    holder.append(&loading(&format!("Pricing {} contracts at {}…", incoming.len() + selling.len(), p.hub().0)));
    col.append(&holder);
    let mut types: Vec<i64> = incoming.iter().chain(selling.iter()).flat_map(|c| bb::types_of(&items_of(c))).collect();
    types.sort_unstable();
    types.dedup();
    let ctx2 = ctx.clone();
    load_prices(ctx, &p.clone(), types, move |prices| {
        let content = w::vbox(16);
        let inc: Vec<Row> = incoming.into_iter().map(|c| row(&p, c, &prices)).collect();
        let out: Vec<Row> = selling.into_iter().map(|c| row(&p, c, &prices)).collect();
        if !inc.is_empty() {
            content.append(&incoming_section(&ctx2, &p, inc));
        }
        if !out.is_empty() {
            content.append(&selling_section(&ctx2, &p, out));
        }
        set_child(&holder, &content);
    });
    col
}

fn incoming_section(ctx: &Ctx, p: &Program, rows: Vec<Row>) -> gtk::Box {
    let v = w::vbox(12);
    v.append(&section(
        "Incoming",
        Some(&format!("Item exchanges assigned to your characters and corporations, against \"{}\" at {} prices right now.", p.name, p.hub().0)),
    ));
    let of = |k: Verdict| rows.iter().filter(move |r| r.check.verdict == k);
    let accept_isk: f64 = of(Verdict::Accept).map(|r| r.check.asked).sum();
    let profit: f64 = rows.iter().filter(|r| r.check.verdict != Verdict::OverAsking).map(|r| r.check.profit_at_sell).sum();
    let accept = w::stat_tile("Accept", &of(Verdict::Accept).count().to_string(), Some(&format!("{} ISK asked", fmt::isk_short(accept_isk))));
    accept.add_css_class("accent");
    v.append(&w::tile_row(vec![
        w::stat_tile("Waiting", &rows.len().to_string(), Some(&format!("{} ISK asked", fmt::isk_short(rows.iter().map(|r| r.check.asked).sum())))),
        accept,
        w::stat_tile("Review", &of(Verdict::Review).count().to_string(), Some("Within tolerance, or with lines to check")),
        w::stat_tile("Over-asking", &of(Verdict::OverAsking).count().to_string(), Some("More than the program pays")),
        w::stat_tile("Expected profit", &format!("{} ISK", fmt::isk_short(profit)), Some("Accept and review, at hub sell")),
    ]));
    let mut rows = rows;
    rows.sort_by(|a, b| a.check.verdict.cmp(&b.check.verdict).then(a.c.date_expired.cmp(&b.c.date_expired)));
    let ctx2 = ctx.clone();
    let table = Table::new(rows)
        .widget_sorted("Verdict", |r: &Row| w::status(r.check.verdict.label(), status_of(r.check.verdict)).upcast(), |a, b| a.check.verdict.cmp(&b.check.verdict))
        .text("From", |r| from_name(&r.c))
        .text("To", |r| r.c.assignee_name.clone().unwrap_or_default())
        .widget_sorted("Items", |r| items_cell(&r.c), |a, b| headline(&a.c).cmp(&headline(&b.c)))
        .width(220)
        .num("Asked", |r| fmt::isk_short(r.check.asked), |r| r.check.asked)
        .num("Offer", |r| fmt::isk_short(r.check.offer), |r| r.check.offer)
        .widget_sorted(
            "Over offer",
            |r| match r.check.over_share {
                Some(s) => colored(&signed_pct(s), signed_class(-s)),
                None => colored("—", "dim"),
            },
            |a, b| a.check.over_share.unwrap_or(f64::MAX).total_cmp(&b.check.over_share.unwrap_or(f64::MAX)),
        )
        .widget_sorted(
            "Profit at sell",
            |r| colored(&fmt::isk_short(r.check.profit_at_sell), signed_class(r.check.profit_at_sell)),
            |a, b| a.check.profit_at_sell.total_cmp(&b.check.profit_at_sell),
        )
        .widget_sorted("Expires", |r| expires_cell(r.c.date_expired), |a, b| a.c.date_expired.cmp(&b.c.date_expired))
        .widget("Notes", |r| {
            let l = w::line(&r.check.reasons.join(" · "), &["body-small", "dim"]);
            l.set_tooltip_text(Some(&r.check.reasons.join("\n")));
            l.upcast()
        })
        .expand()
        .widget("", move |r| open_button(&ctx2, &r.c));
    let ctx = ctx.clone();
    v.append(&table.on_activate(move |r| open_contract(&ctx, &r.c)).natural_height().build());
    v
}

fn selling_section(ctx: &Ctx, p: &Program, rows: Vec<Row>) -> gtk::Box {
    let v = w::vbox(12);
    v.append(&section(
        "Your open sales",
        Some(&format!(
            "Item exchanges your characters and corporations made to others (other buybacks), with what the items fetch at {} and what \"{}\" would pay for them.",
            p.hub().0,
            p.name
        )),
    ));
    let asked: f64 = rows.iter().map(|r| r.check.asked).sum();
    let buy: f64 = rows.iter().map(|r| r.offer.hub_buy).sum();
    let sell: f64 = rows.iter().map(|r| r.offer.hub_sell).sum();
    v.append(&w::tile_row(vec![
        w::stat_tile("Open sales", &rows.len().to_string(), Some(&format!("{} ISK asked", fmt::isk_short(asked)))),
        w::stat_tile("Hub buy value", &format!("{} ISK", fmt::isk_short(buy)), Some(&if buy > 0.0 { format!("Asking {} of it", pct(asked / buy)) } else { "—".into() })),
        w::stat_tile("Hub sell value", &format!("{} ISK", fmt::isk_short(sell)), Some(&if sell > 0.0 { format!("Asking {} of it", pct(asked / sell)) } else { "—".into() })),
    ]));
    let ctx2 = ctx.clone();
    let share = |a: f64, of: f64| if of > 0.0 { pct(a / of) } else { "—".into() };
    let table = Table::new(rows)
        .text("To", |r: &Row| r.c.assignee_name.clone().unwrap_or_default())
        .text("From", |r| from_name(&r.c))
        .widget_sorted("Items", |r| items_cell(&r.c), |a, b| headline(&a.c).cmp(&headline(&b.c)))
        .expand()
        .num("Asked", |r| fmt::isk_short(r.check.asked), |r| r.check.asked)
        .num("Hub buy", |r| fmt::isk_short(r.offer.hub_buy), |r| r.offer.hub_buy)
        .num("Of buy", move |r| share(r.check.asked, r.offer.hub_buy), |r| if r.offer.hub_buy > 0.0 { r.check.asked / r.offer.hub_buy } else { 0.0 })
        .num("Hub sell", |r| fmt::isk_short(r.offer.hub_sell), |r| r.offer.hub_sell)
        .num("Program pays", |r| fmt::isk_short(r.offer.total), |r| r.offer.total)
        .widget_sorted(
            "vs program",
            |r| colored(&fmt::isk_short(r.check.over), signed_class(r.check.over)),
            |a, b| a.check.over.total_cmp(&b.check.over),
        )
        .widget_sorted("Expires", |r| expires_cell(r.c.date_expired), |a, b| a.c.date_expired.cmp(&b.c.date_expired))
        .widget("", move |r| open_button(&ctx2, &r.c));
    let ctx = ctx.clone();
    v.append(&table.on_activate(move |r| open_contract(&ctx, &r.c)).natural_height().build());
    let note = w::line("\"vs program\" is what you ask over (green) or under (red) the program's terms: under means the buyback gets a better deal than its terms.", &["body-small", "dim"]);
    note.set_wrap(true);
    v.append(&note);
    v
}
