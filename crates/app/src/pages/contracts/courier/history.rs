//! Courier → History: what hauling paid and shipping cost over a period, ISK per jump and per m³, how often
//! couriers failed, per character and per route. Built from the contracts archive, so it goes past ESI's 30 days.

use super::{header, system_name, View};
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::security_label;
use crate::widgets::{self as w, Table};
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::features::contracts::{self as ct, Mine, Period};
use podstack_core::features::courier::{self as cr, HaulTotals, RouteStats};
use podstack_core::fmt;

const PERIOD_KEY: &str = "contracts.courier.period";

fn opt_isk(v: Option<f64>) -> String {
    v.map(fmt::isk_short).unwrap_or_else(|| "–".into())
}

fn rate(v: Option<f64>) -> String {
    v.map(|r| format!("{}%", fmt::decimal(r * 100.0, 1))).unwrap_or_else(|| "–".into())
}

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.append(&header(ctx, View::History));
    let period = Period::from_key(&ctx.recall(PERIOD_KEY).unwrap_or_else(|| Period::Quarter.key().into()));
    let labels: Vec<&str> = Period::ALL.iter().map(|p| p.label()).collect();
    let dd = w::dropdown(&labels, Period::ALL.iter().position(|p| *p == period).unwrap_or(0) as u32);
    {
        let ctx = ctx.clone();
        dd.connect_selected_notify(move |d| {
            ctx.remember(PERIOD_KEY, Period::ALL.get(d.selected() as usize).copied().unwrap_or_default().key());
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        });
    }
    let archive = ct::archive_snapshot();
    let settings = ct::settings();
    let bar = w::hbox(12);
    bar.append(&dd);
    let since = archive.since().map(|s| format!("PodStack has kept contracts since {}.", fmt::date(s))).unwrap_or_default();
    let note = w::line(
        &format!(
            "{since} {}",
            if settings.count_internal { "Couriers between your own characters are counted." } else { "Couriers between your own characters are left out (see the contracts settings)." }
        ),
        &["body-small", "dim"],
    );
    note.set_wrap(true);
    note.set_ellipsize(gtk::pango::EllipsizeMode::None);
    note.set_xalign(0.0);
    note.set_hexpand(true);
    bar.append(&note);
    body.append(&bar);

    let chars = ctx.core().characters();
    let mine = Mine::of(&chars);
    let now = Utc::now();
    let flows: Vec<ct::Flow> = ct::all_flows(&chars, &archive, settings.value_at_sell)
        .into_iter()
        .filter(|f| period.contains(f.date, now) && (settings.count_internal || !f.internal))
        .collect();
    let pref = cr::load_doc().route;
    let h = cr::history(&flows, |id| archive.get(id).map(|e| &e.contract), |a, b| cr::summary(a, b, pref).map(|s| s.jumps));

    if h.hauled.count + h.hauled.failed + h.shipped.count + h.shipped.failed == 0 {
        body.append(&w::empty_state(
            "history",
            "No couriers in this period",
            "Couriers your characters haul or ship are counted here once they're delivered (or fail). Try a longer period.",
        ));
        return Page::new("Contracts", body);
    }

    body.append(&w::tile_row(tiles(&h.hauled, &h.shipped)));

    let parties: Vec<(String, HaulTotals, HaulTotals)> = h.by_party.iter().map(|(p, a, b)| (mine.name(*p), *a, *b)).collect();
    body.append(&w::card_with("Per character", Some("Hauled and shipped by each character or corporation"), &[], &party_table(parties)));
    body.append(&w::card_with(
        "Top routes",
        Some("Pickup system to delivery system, most couriers first"),
        &[],
        &routes_table(h.routes.into_iter().take(200).collect()),
    ));
    Page::new("Contracts", body)
}

fn tiles(hauled: &HaulTotals, shipped: &HaulTotals) -> Vec<gtk::Box> {
    let mut v = Vec::new();
    if hauled.count + hauled.failed > 0 {
        v.push(w::stat_tile("Hauled", &fmt::int(hauled.count as i64), Some(&format!("{} earned · {}", fmt::isk_short(hauled.reward), super::m3(hauled.volume)))));
        let pj = w::stat_tile("ISK per jump", &opt_isk(hauled.isk_per_jump()), Some(&format!("over {} jumps", fmt::int(hauled.jumps as i64))));
        pj.set_tooltip_text(Some("Rewards of delivered couriers divided by their jumps (a courier within one system counts as one)"));
        v.push(pj);
        v.push(w::stat_tile("ISK per m³", &opt_isk(hauled.isk_per_m3()), Some("rewards over volume moved")));
        let f = w::stat_tile(
            "Failure rate",
            &rate(hauled.failure_rate()),
            Some(&format!("{} failed · {} collateral lost", hauled.failed, fmt::isk_short(hauled.failed_collateral))),
        );
        f.set_tooltip_text(Some("Couriers your haulers failed, out of all they finished or failed"));
        v.push(f);
    }
    if shipped.count + shipped.failed > 0 {
        v.push(w::stat_tile("Shipped", &fmt::int(shipped.count as i64), Some(&format!("{} paid · {}", fmt::isk_short(shipped.reward), super::m3(shipped.volume)))));
        v.push(w::stat_tile("Paid per jump", &opt_isk(shipped.isk_per_jump()), Some(&format!("{} per m³", opt_isk(shipped.isk_per_m3())))));
        if shipped.failed > 0 {
            v.push(w::stat_tile(
                "Shipments failed",
                &fmt::int(shipped.failed as i64),
                Some(&format!("{} collateral received", fmt::isk_short(shipped.failed_collateral))),
            ));
        }
    }
    v
}

fn party_table(rows: Vec<(String, HaulTotals, HaulTotals)>) -> gtk::Widget {
    Table::new(rows)
        .text("Character", |r| r.0.clone())
        .expand()
        .num("Hauled", |r| fmt::int(r.1.count as i64), |r| r.1.count as f64)
        .num("Earned", |r| fmt::isk_short(r.1.reward), |r| r.1.reward)
        .num("Cargo", |r| super::m3(r.1.volume), |r| r.1.volume)
        .num("ISK/jump", |r| opt_isk(r.1.isk_per_jump()), |r| r.1.isk_per_jump().unwrap_or(0.0))
        .num("Failed", |r| if r.1.failed > 0 { r.1.failed.to_string() } else { String::new() }, |r| r.1.failed as f64)
        .num("Lost", |r| if r.1.failed > 0 { fmt::isk_short(r.1.failed_collateral) } else { String::new() }, |r| r.1.failed_collateral)
        .num("Shipped", |r| fmt::int(r.2.count as i64), |r| r.2.count as f64)
        .num("Paid", |r| fmt::isk_short(r.2.reward), |r| r.2.reward)
        .max_height(360)
        .build()
}

fn routes_table(rows: Vec<RouteStats>) -> gtk::Widget {
    let sys = |id: i64| {
        let b = w::hbox(6);
        let s = security_label(id);
        s.set_width_chars(3);
        b.append(&s);
        b.append(&w::line(&system_name(Some(id)), &[]));
        b.upcast::<gtk::Widget>()
    };
    Table::new(rows)
        .widget_sorted("Pickup", move |r| sys(r.from), |a, b| system_name(Some(a.from)).cmp(&system_name(Some(b.from))))
        .expand()
        .widget_sorted("Delivery", move |r| sys(r.to), |a, b| system_name(Some(a.to)).cmp(&system_name(Some(b.to))))
        .expand()
        .num("Jumps", |r| r.jumps.map(|j| j.to_string()).unwrap_or_else(|| "–".into()), |r| r.jumps.map(|j| j as f64).unwrap_or(-1.0))
        .num("Couriers", |r| fmt::int(r.totals.count as i64), |r| r.totals.count as f64)
        .num("Failed", |r| if r.totals.failed > 0 { r.totals.failed.to_string() } else { String::new() }, |r| r.totals.failed as f64)
        .num("Total reward", |r| fmt::isk_short(r.totals.reward), |r| r.totals.reward)
        .num("Average", |r| fmt::isk_short(r.average_reward()), |r| r.average_reward())
        .num("ISK/jump", |r| opt_isk(r.totals.isk_per_jump()), |r| r.totals.isk_per_jump().unwrap_or(0.0))
        .num("Cargo", |r| super::m3(r.totals.volume), |r| r.totals.volume)
        .max_height(480)
        .empty_text("No route known: the couriers' locations weren't resolved.")
        .build()
}
