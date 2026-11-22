//! Planetary Industry → Finder: systems by the planet kinds they have. Tick the kinds you need (or let a product
//! pick them), narrow by security and region, and see each system's planets, their raw resources and what can be
//! made entirely there. The kinds and their resources are listed at the bottom for reference.

use super::common::{commodity_cell, tier_badge, CSS};
use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::{search_systems, security_label, suggest_entry};
use crate::widgets::{self as w, Table};
use gtk::prelude::*;
use podstack_core::features::planetary::{self as pl, SystemPlanets};
use podstack_core::pi::{planet_name, PI};
use podstack_core::sde::{SecurityClass, MAP};
use podstack_core::fmt;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

const KINDS_KEY: &str = "planets.finder.kinds";
const SEC_KEY: &str = "planets.finder.security";
const REGION_KEY: &str = "planets.finder.region";
const SYSTEM_KEY: &str = "planets.finder.system";

/// Systems listed at most (the table stays quick; narrowing finds the rest).
const LIMIT: usize = 500;

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    crate::theme::add_css(".pi-kind-chip { padding: 2px 10px; }");
    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&tab_bar(ctx, Tab::Finder));

    let kinds: Rc<RefCell<BTreeSet<String>>> =
        Rc::new(RefCell::new(ctx.recall(KINDS_KEY).map(|s| s.split(',').filter(|k| !k.is_empty()).map(String::from).collect()).unwrap_or_default()));

    // filters
    let sec_labels = ["All security", "High sec", "Low sec", "Null sec"];
    let sec = w::dropdown(&sec_labels, ctx.recall(SEC_KEY).and_then(|s| s.parse().ok()).unwrap_or(0));
    let regions: Vec<(i64, String)> = std::iter::once((0, "All regions".to_string())).chain(MAP.regions.iter().cloned()).collect();
    let region_labels: Vec<&str> = regions.iter().map(|r| r.1.as_str()).collect();
    let region_now = ctx.recall(REGION_KEY).and_then(|s| s.parse::<i64>().ok()).and_then(|id| regions.iter().position(|r| r.0 == id)).unwrap_or(0) as u32;
    let region = w::dropdown(&region_labels, region_now);
    let products: Vec<(i64, String)> = std::iter::once((0, "Planets for a product…".to_string()))
        .chain((1..=4).flat_map(|t| PI.of_tier(t).into_iter().map(|c| (c.id, format!("P{} {}", c.tier, c.name)))))
        .collect();
    let product_labels: Vec<&str> = products.iter().map(|p| p.1.as_str()).collect();
    let product = w::dropdown(&product_labels, 0);
    product.set_enable_search(true);

    let chips = w::hbox(6);
    chips.set_halign(gtk::Align::Start);
    let chip_buttons: Rc<RefCell<Vec<(String, gtk::ToggleButton)>>> = Rc::default();
    for k in &PI.kinds {
        let c = w::chip(k.label, kinds.borrow().contains(k.key));
        chips.append(&c);
        chip_buttons.borrow_mut().push((k.key.to_string(), c));
    }

    let bar = w::hbox(12);
    bar.append(&sec);
    bar.append(&region);
    bar.append(&product);
    let filters = w::vbox(8);
    filters.append(&bar);
    let chip_row = w::hbox(8);
    chip_row.append(&w::line("Must have", &["label-medium", "dim"]));
    let chip_scroll = gtk::ScrolledWindow::new();
    chip_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    chip_scroll.set_child(Some(&chips));
    chip_scroll.set_hexpand(true);
    chip_row.append(&chip_scroll);
    filters.append(&chip_row);
    body.append(&filters);

    let summary = w::line("", &["body-small", "dim"]);
    summary.set_xalign(0.0);
    body.append(&summary);

    // results (left) and the chosen system (below on narrow windows): a paned split keeps both usable
    let (results_sw, results) = w::async_holder();
    results_sw.set_vexpand(true);
    let detail = w::vbox(12);
    let detail_sw = w::scrolled(&detail);
    let paned = gtk::Paned::new(gtk::Orientation::Vertical);
    paned.set_start_child(Some(&results_sw));
    paned.set_end_child(Some(&detail_sw));
    paned.set_resize_start_child(true);
    paned.set_vexpand(true);
    paned.set_position(380);
    body.append(&paned);

    let show_system: Rc<dyn Fn(i64)> = {
        let (ctx, detail) = (ctx.clone(), detail.clone());
        Rc::new(move |id| {
            ctx.remember(SYSTEM_KEY, id.to_string());
            fill_detail(&detail, id);
        })
    };
    let render: Rc<dyn Fn()> = {
        let (ctx, kinds, sec, region, regions, results, summary, show_system) =
            (ctx.clone(), kinds.clone(), sec.clone(), region.clone(), regions.clone(), results.clone(), summary.clone(), show_system.clone());
        Rc::new(move || {
            let sec_class = match sec.selected() {
                1 => Some(SecurityClass::High),
                2 => Some(SecurityClass::Low),
                3 => Some(SecurityClass::Null),
                _ => None,
            };
            let region_id = regions.get(region.selected() as usize).map(|r| r.0).filter(|r| *r != 0);
            let want: Vec<String> = kinds.borrow().iter().cloned().collect();
            ctx.remember(KINDS_KEY, want.join(","));
            ctx.remember(SEC_KEY, sec.selected().to_string());
            ctx.remember(REGION_KEY, region_id.unwrap_or(0).to_string());
            let want_refs: Vec<&str> = want.iter().map(|s| s.as_str()).collect();
            let found = pl::find_systems(&want_refs, sec_class, region_id, LIMIT);
            summary.set_text(&if found.len() >= LIMIT {
                format!("Showing the first {LIMIT} systems: narrow by region or planet kinds to see others.")
            } else {
                format!("{} system{}", fmt::int(found.len() as i64), if found.len() == 1 { "" } else { "s" })
            });
            while let Some(c) = results.first_child() {
                results.remove(&c);
            }
            results.append(&table(found, show_system.clone()));
        })
    };
    for (key, chip) in chip_buttons.borrow().iter() {
        let (kinds, render, key) = (kinds.clone(), render.clone(), key.clone());
        chip.connect_toggled(move |c| {
            if c.is_active() {
                kinds.borrow_mut().insert(key.clone());
            } else {
                kinds.borrow_mut().remove(&key);
            }
            render();
        });
    }
    for d in [&sec, &region] {
        let render = render.clone();
        d.connect_selected_notify(move |_| render());
    }
    {
        // A product sets the chips to a small set of planet kinds that has all its raw resources.
        let chip_buttons = chip_buttons.clone();
        product.connect_selected_notify(move |d| {
            let Some((id, _)) = products.get(d.selected() as usize) else { return };
            if *id == 0 {
                return;
            }
            let needed = pl::planets_for(*id).unwrap_or_default();
            for (key, chip) in chip_buttons.borrow().iter() {
                // each toggle re-renders; the last one leaves the right result
                chip.set_active(needed.contains(&key.as_str()));
            }
        });
    }

    // jump straight to a system
    let jump = suggest_entry("Go to a system", search_systems, {
        let show = show_system.clone();
        move |id, _| show(id)
    });
    jump.set_hexpand(true);
    bar.prepend(&jump);

    render();
    if let Some(id) = ctx.recall(SYSTEM_KEY).and_then(|s| s.parse::<i64>().ok()) {
        fill_detail(&detail, id);
    } else {
        fill_reference(&detail);
    }
    Page::new("Planetary Industry", body).static_page().no_scroll()
}

fn table(rows: Vec<SystemPlanets>, show: Rc<dyn Fn(i64)>) -> gtk::Widget {
    let mut t = Table::new(rows)
        .text("System", |r| MAP.system(r.system).map(|s| s.name.clone()).unwrap_or_default())
        .expand()
        .width(130)
        .widget_sorted("Sec", |r| security_label(r.system).upcast(), |a, b| {
            let s = |x: &SystemPlanets| MAP.system(x.system).map(|s| s.security).unwrap_or(0.0);
            s(a).total_cmp(&s(b))
        })
        .width(50)
        .text("Region", |r| MAP.system(r.system).and_then(|s| MAP.region_name(s.region)).unwrap_or("").to_string())
        .width(130)
        .num("Kinds", |r| r.kinds().to_string(), |r| r.kinds() as f64)
        .width(60)
        .num("Planets", |r| r.total().to_string(), |r| r.total() as f64)
        .width(70);
    for k in &PI.kinds {
        let key = k.key;
        t = t.num(k.label, move |r| r.counts.get(key).map(|n| n.to_string()).unwrap_or_default(), move |r| *r.counts.get(key).unwrap_or(&0) as f64).width(80);
    }
    t.empty_text("No system has all of those planet kinds here").on_activate(move |r| show(r.system)).build()
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// One system: its planets with their raw resources, and what can be made entirely there.
fn fill_detail(b: &gtk::Box, system: i64) {
    clear(b);
    let Some(sys) = MAP.system(system) else { return };
    let header = w::hbox(12);
    header.append(&w::line(&sys.name, &["title-large"]));
    header.append(&security_label(system));
    header.append(&w::line(MAP.region_name(sys.region).unwrap_or(""), &["dim"]));
    b.append(&header);

    let planets = w::vbox(4);
    let mut kinds_here: BTreeSet<&'static str> = BTreeSet::new();
    for (t, index) in PI.planets_in(system) {
        let Some(k) = PI.kind_of_type(*t) else { continue };
        kinds_here.insert(k.key);
        let row = w::hbox(12);
        let name = w::line(&planet_name(&sys.name, *index), &["body-medium"]);
        name.set_width_chars(16);
        name.set_xalign(0.0);
        row.append(&name);
        let kind = w::line(k.label, &["body-medium", "primary-text"]);
        kind.set_width_chars(10);
        kind.set_xalign(0.0);
        row.append(&kind);
        let res = w::line(&k.resources.iter().map(|r| PI.name(*r)).collect::<Vec<_>>().join(", "), &["body-small", "dim"]);
        res.set_wrap(true);
        res.set_xalign(0.0);
        res.set_hexpand(true);
        row.append(&res);
        planets.append(&row);
    }
    if PI.planets_in(system).is_empty() {
        planets.append(&w::line("No planets you can build colonies on.", &["dim"]));
    }
    b.append(&w::card_with("Planets", None, &[], &planets));

    // Products whose raw resources all occur on this system's planets: no hauling of raw materials needed.
    let raw_here: BTreeSet<i64> = PI.kinds.iter().filter(|k| kinds_here.contains(k.key)).flat_map(|k| k.resources.iter().copied()).collect();
    let local = w::vbox(8);
    for tier in 1..=4u8 {
        let items: Vec<i64> = PI.of_tier(tier).into_iter().filter(|c| PI.raw_inputs(c.id).iter().all(|r| raw_here.contains(r))).map(|c| c.id).collect();
        if items.is_empty() {
            continue;
        }
        let row = w::hbox(8);
        row.append(&tier_badge(tier));
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_max_children_per_line(4);
        flow.set_hexpand(true);
        for i in items {
            flow.insert(&commodity_cell(i), -1);
        }
        row.append(&flow);
        local.append(&row);
    }
    if local.first_child().is_none() {
        local.append(&w::line("Nothing: every product needs a resource from elsewhere.", &["dim"]));
    }
    b.append(&w::card_with("Made entirely from this system", Some("Every raw resource is on one of its planets"), &[], &local));
}

/// The planet kinds and their raw resources.
fn fill_reference(b: &gtk::Box) {
    clear(b);
    let grid = w::vbox(6);
    for k in &PI.kinds {
        let row = w::hbox(12);
        let l = w::line(k.label, &["body-medium", "primary-text"]);
        l.set_width_chars(10);
        l.set_xalign(0.0);
        row.append(&l);
        let res = w::line(&k.resources.iter().map(|r| PI.name(*r)).collect::<Vec<_>>().join(", "), &["body-small"]);
        res.set_wrap(true);
        res.set_xalign(0.0);
        row.append(&res);
        grid.append(&row);
    }
    b.append(&w::card_with("Planet kinds and resources", Some("Pick a system above to see its planets"), &[], &grid));
}
