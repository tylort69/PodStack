//! Courier → Active: what the characters are hauling now and how long they have left, what they've shipped and
//! where it is, couriers assigned to them, and each hauler's load.

use super::{countdown_cell, details_cell, header, jumps_cell, m3, min_sec_cell, party_name, place_cell, place_key, View};
use crate::ctx::{Ctx, Page};
use crate::pages::contracts::common::open_contract;
use crate::widgets::{self as w, Status, Table};
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::domains::contracts::CORP_SCOPE;
use podstack_core::features::contracts::{Mine, Party};
use podstack_core::features::courier::{self as cr, ActiveCourier, Load, Role, RouteSummary};
use podstack_core::fmt;

/// An open courier with its route worked out.
#[derive(Clone)]
struct Row {
    a: ActiveCourier,
    route: Option<RouteSummary>,
    who: String,
}

impl Row {
    fn isk_per_jump(&self) -> Option<f64> {
        self.route.map(|r| cr::per_jump(self.a.reward(), r.jumps))
    }
}

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.append(&header(ctx, View::Active));
    let chars = ctx.core().characters();
    if chars.is_empty() {
        body.append(&w::empty_state("local_shipping", "No characters", "Add a character to see the couriers it hauls and ships."));
        return Page::new("Contracts", body);
    }
    // a login without the contracts scope sees nothing here: say so rather than show an empty list
    let missing: Vec<String> = chars
        .iter()
        .filter(|c| c.refreshes.get("contracts").and_then(|r| r.error.as_deref()) == Some("scope"))
        .map(|c| c.name())
        .collect();
    if !missing.is_empty() {
        body.append(&w::scope_notice(&format!("esi-contracts.read_character_contracts.v1 ({})", missing.join(", "))));
    }
    let corp_missing = chars.iter().any(|c| c.refreshes.get("corp_contracts").and_then(|r| r.error.as_deref()) == Some("scope"));

    let mine = Mine::of(&chars);
    let now = Utc::now();
    let pref = cr::load_doc().route;
    let listed = cr::listed(&chars);
    let active = cr::active(&listed, &mine, now);
    let rows: Vec<Row> = active
        .iter()
        .map(|a| Row {
            route: match (a.from_system(), a.to_system()) {
                (Some(f), Some(t)) => cr::summary(f, t, pref),
                _ => None,
            },
            who: party_name(&mine, a.party),
            a: a.clone(),
        })
        .collect();
    let t = cr::totals(&active, now);

    body.append(&w::tile_row(tiles(&t)));

    if active.is_empty() {
        body.append(&w::empty_state(
            "local_shipping",
            "No couriers under way",
            "Couriers your characters accept or issue show up here with their deadlines, collateral and routes.",
        ));
        if corp_missing {
            body.append(&w::scope_notice(CORP_SCOPE));
        }
        return Page::new("Contracts", body);
    }

    let hauling: Vec<Row> = rows.iter().filter(|r| r.a.role == Role::Hauling).cloned().collect();
    let shipping: Vec<Row> = rows.iter().filter(|r| r.a.role == Role::Shipping).cloned().collect();
    let assigned: Vec<Row> = rows.iter().filter(|r| r.a.role == Role::Assigned).cloned().collect();

    if !hauling.is_empty() {
        let sub = format!("{} courier{} · deliver before the time runs out or lose the collateral", hauling.len(), plural(hauling.len()));
        body.append(&w::card_with("Hauling", Some(&sub), &[], &hauling_table(ctx, hauling)));
        let loads = cr::loads(&active, now);
        body.append(&w::card_with(
            "Per hauler",
            Some("What each character or corporation is carrying"),
            &[],
            &loads_table(&mine, loads),
        ));
    }
    if !shipping.is_empty() {
        let sub = format!("{} in transit · {} waiting for a hauler", t.in_transit.count, t.waiting.count);
        body.append(&w::card_with("Shipped", Some(&sub), &[], &shipping_table(ctx, shipping)));
    }
    if !assigned.is_empty() {
        body.append(&w::card_with(
            "Assigned to you",
            Some("Couriers others have offered to your characters or corporations, not accepted yet"),
            &[],
            &assigned_table(ctx, assigned),
        ));
    }
    if corp_missing {
        body.append(&w::scope_notice(CORP_SCOPE));
    }
    Page::new("Contracts", body)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn tiles(t: &cr::ActiveTotals) -> Vec<gtk::Box> {
    let at_risk = w::stat_tile(
        "Collateral at risk",
        &fmt::isk_short(t.hauling.collateral),
        Some(&format!("{} courier{} being hauled", t.hauling.count, plural(t.hauling.count))),
    );
    at_risk.set_tooltip_text(Some("Collateral your haulers put up: lost if they don't deliver in time"));
    let transit = w::stat_tile(
        "Cargo in transit",
        &m3(t.in_transit.volume),
        Some(&format!("{} shipment{} · {} collateral", t.in_transit.count, plural(t.in_transit.count), fmt::isk_short(t.in_transit.collateral))),
    );
    transit.set_tooltip_text(Some("Your couriers that a hauler has accepted"));
    let rewards = w::stat_tile(
        "Rewards on delivery",
        &fmt::isk_short(t.hauling.reward),
        Some(&format!("you pay {} on accepted shipments", fmt::isk_short(t.in_transit.reward))),
    );
    rewards.set_tooltip_text(Some("What your haulers get when they deliver, and what your accepted shipments will cost"));
    let soon_sub = if t.hauling.overdue > 0 { format!("{} overdue", t.hauling.overdue) } else { "nothing overdue".to_string() };
    let soon = w::stat_tile("Due within 24 h", &t.due_soon.to_string(), Some(&soon_sub));
    soon.set_tooltip_text(Some("Couriers being hauled that must be delivered in the next day"));
    vec![at_risk, transit, rewards, soon]
}

fn route_cols(t: Table<Row>) -> Table<Row> {
    t.widget_sorted("Jumps", |r| jumps_cell(r.route.as_ref()), |a, b| a.route.map(|r| r.jumps).cmp(&b.route.map(|r| r.jumps)))
        .width(64)
        .widget_sorted("Min sec", |r| min_sec_cell(r.route.as_ref()), |a, b| {
            a.route.map(|r| r.min_security).unwrap_or(-2.0).total_cmp(&b.route.map(|r| r.min_security).unwrap_or(-2.0))
        })
        .width(70)
}

fn money_cols(t: Table<Row>) -> Table<Row> {
    t.num("Volume", |r| m3(r.a.volume()), |r| r.a.volume())
        .num("Collateral", |r| fmt::isk_short(r.a.collateral()), |r| r.a.collateral())
        .num("Reward", |r| fmt::isk_short(r.a.reward()), |r| r.a.reward())
}

fn hauling_table(ctx: &Ctx, rows: Vec<Row>) -> gtk::Widget {
    let t = Table::new(rows)
        .text("Hauler", |r| r.who.clone())
        .widget_sorted("Pickup", |r| place_cell(&r.a.contract.start_location), |a, b| place_key(&a.a.contract.start_location).cmp(&place_key(&b.a.contract.start_location)))
        .expand()
        .widget_sorted("Delivery", |r| place_cell(&r.a.contract.end_location), |a, b| place_key(&a.a.contract.end_location).cmp(&place_key(&b.a.contract.end_location)))
        .expand();
    let t = money_cols(route_cols(t))
        .num("ISK/jump", |r| r.isk_per_jump().map(fmt::isk_short).unwrap_or_default(), |r| r.isk_per_jump().unwrap_or(0.0))
        .widget_sorted("Time left", |r| countdown_cell(r.a.due, "Overdue"), |a, b| a.a.due.cmp(&b.a.due))
        .width(110);
    finish(ctx, t)
}

fn shipping_table(ctx: &Ctx, rows: Vec<Row>) -> gtk::Widget {
    let t = Table::new(rows)
        .text("Issued by", |r| r.who.clone())
        .widget_sorted("Pickup", |r| place_cell(&r.a.contract.start_location), |a, b| place_key(&a.a.contract.start_location).cmp(&place_key(&b.a.contract.start_location)))
        .expand()
        .widget_sorted("Delivery", |r| place_cell(&r.a.contract.end_location), |a, b| place_key(&a.a.contract.end_location).cmp(&place_key(&b.a.contract.end_location)))
        .expand()
        .widget_sorted(
            "State",
            |r| if r.a.accepted() { w::status("In transit", Status::Info) } else { w::status("Waiting for a hauler", Status::Neutral) }.upcast(),
            |a, b| a.a.accepted().cmp(&b.a.accepted()),
        )
        .text("Hauler", |r| if r.a.accepted() { r.a.contract.acceptor_name.clone().unwrap_or_default() } else { availability(&r.a) });
    let t = money_cols(t)
        .widget_sorted(
            "Time left",
            |r| {
                let c = countdown_cell(r.a.due, if r.a.accepted() { "Overdue" } else { "Expired" });
                c.set_tooltip_text(Some(&format!("{} {}", if r.a.accepted() { "Due" } else { "Expires" }, fmt::datetime(r.a.due))));
                c
            },
            |a, b| a.a.due.cmp(&b.a.due),
        )
        .width(110);
    finish(ctx, t)
}

/// Who can take a courier nobody has accepted yet.
fn availability(a: &ActiveCourier) -> String {
    match (&a.contract.availability[..], &a.contract.assignee_name) {
        ("public", _) => "Public".into(),
        (_, Some(n)) => format!("Offered to {n}"),
        (k, None) => podstack_core::features::comms::title_case(k),
    }
}

fn assigned_table(ctx: &Ctx, rows: Vec<Row>) -> gtk::Widget {
    let t = Table::new(rows)
        .text("Assigned to", |r| r.who.clone())
        .text("Issuer", |r| if r.a.contract.for_corporation { r.a.contract.issuer_corporation_name.clone() } else { r.a.contract.issuer_name.clone() })
        .widget_sorted("Pickup", |r| place_cell(&r.a.contract.start_location), |a, b| place_key(&a.a.contract.start_location).cmp(&place_key(&b.a.contract.start_location)))
        .expand()
        .widget_sorted("Delivery", |r| place_cell(&r.a.contract.end_location), |a, b| place_key(&a.a.contract.end_location).cmp(&place_key(&b.a.contract.end_location)))
        .expand();
    let t = money_cols(route_cols(t))
        .num("ISK/jump", |r| r.isk_per_jump().map(fmt::isk_short).unwrap_or_default(), |r| r.isk_per_jump().unwrap_or(0.0))
        .num("Days", |r| r.a.contract.days_to_complete.unwrap_or(0).to_string(), |r| r.a.contract.days_to_complete.unwrap_or(0) as f64)
        .widget_sorted("Expires", |r| countdown_cell(r.a.due, "Expired"), |a, b| a.a.due.cmp(&b.a.due))
        .width(110);
    finish(ctx, t)
}

fn finish(ctx: &Ctx, t: Table<Row>) -> gtk::Widget {
    let (ctx, ctx2) = (ctx.clone(), ctx.clone());
    t.widget("", move |r| details_cell(&ctx2, &r.a.contract))
        .on_activate(move |r| open_contract(&ctx, &r.a.contract))
        .max_height(440)
        .build()
}

fn loads_table(mine: &Mine, loads: Vec<(Party, Load)>) -> gtk::Widget {
    let names: Vec<String> = loads.iter().map(|(p, _)| party_name(mine, *p)).collect();
    let rows: Vec<(String, Load)> = names.into_iter().zip(loads.into_iter().map(|l| l.1)).collect();
    Table::new(rows)
        .text("Hauler", |r| r.0.clone())
        .expand()
        .num("Couriers", |r| r.1.count.to_string(), |r| r.1.count as f64)
        .num("Cargo", |r| m3(r.1.volume), |r| r.1.volume)
        .num("Collateral", |r| fmt::isk_short(r.1.collateral), |r| r.1.collateral)
        .num("Rewards", |r| fmt::isk_short(r.1.reward), |r| r.1.reward)
        .widget_sorted(
            "Next due",
            |r| match r.1.next_due {
                Some(d) => countdown_cell(d, "Overdue"),
                None => w::line("", &[]).upcast(),
            },
            |a, b| a.1.next_due.cmp(&b.1.next_due),
        )
        .width(110)
        .widget_sorted(
            "Overdue",
            |r| if r.1.overdue > 0 { w::status(&r.1.overdue.to_string(), Status::Bad).upcast() } else { w::line("", &[]).upcast() },
            |a, b| a.1.overdue.cmp(&b.1.overdue),
        )
        .max_height(320)
        .build()
}
