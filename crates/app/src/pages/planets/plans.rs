//! Planetary Industry → Plans: production plans for a product. Pick what to make, how many final factories and
//! (optionally) a system; PodStack lays out extraction planets for the raw resources and factory planets for each
//! step, assigns them to characters with free planet slots, and then tracks how far the real colonies match. Plans
//! are saved; characters and planet kinds can be changed per planet.

use super::common::{self, commodity_cell, tier_badge, CSS};
use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::{fields, labelled, number_entry, search_systems, suggest_entry, system_name};
use crate::widgets::{self as w, Btn};
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::features::planetary::{self as pl, PiPlan, PlanetRole};
use podstack_core::fmt;
use podstack_core::pi::PI;
use std::cell::Cell;
use std::rc::Rc;

const PLAN_KEY: &str = "planets.plans.open";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Plans));
    let doc = pl::load();
    body.append(&new_plan_card(ctx));
    if doc.plans.is_empty() {
        body.append(&w::empty_state(
            "assignment",
            "No plans yet",
            "Pick a product above: PodStack lays out the planets it takes, shares them out over your characters' free planet slots, and tracks your colonies against it.",
        ));
    } else {
        let open = ctx.recall(PLAN_KEY).and_then(|id| doc.plans.iter().position(|p| p.id == id)).unwrap_or(0);
        body.append(&plan_picker(ctx, &doc.plans, open));
        body.append(&plan_view(ctx, &doc.plans[open]));
    }
    let page = Page::new("Planetary Industry", body).static_page();
    // colonies refreshing changes the progress
    let ctx2 = ctx.clone();
    ctx.on_event(&page.content, move |e| {
        if matches!(e, podstack_core::Event::CharacterUpdated(_)) {
            ctx2.rebuild();
        }
    });
    page
}

fn new_plan_card(ctx: &Ctx) -> gtk::Box {
    let products: Vec<(i64, String)> = (1..=4).flat_map(|t| PI.of_tier(t).into_iter().map(|c| (c.id, format!("P{} {}", c.tier, c.name)))).collect();
    let labels: Vec<&str> = products.iter().map(|p| p.1.as_str()).collect();
    let product = w::dropdown(&labels, 0);
    product.set_enable_search(true);
    let factories = number_entry("1", 5);
    let system: Rc<Cell<Option<i64>>> = Rc::default();
    let sys_entry = suggest_entry("Any system", search_systems, {
        let system = system.clone();
        move |id, _| system.set(Some(id))
    });
    {
        let system = system.clone();
        sys_entry.connect_search_changed(move |e| {
            if e.text().trim().is_empty() {
                system.set(None);
            }
        });
    }
    let create = w::button_icon("auto_awesome", "Make a plan", Btn::Filled);
    {
        let (ctx, product, factories) = (ctx.clone(), product.clone(), factories.clone());
        create.connect_clicked(move |_| {
            let Some((id, _)) = products.get(product.selected() as usize) else { return };
            let n = factories.text().trim().parse::<usize>().unwrap_or(1).clamp(1, 500);
            let plan = pl::suggest_plan(*id, n, &ctx.core().characters(), system.get());
            let mut doc = pl::load();
            ctx.remember(PLAN_KEY, plan.id.clone());
            doc.plans.push(plan);
            pl::save(ctx.core(), &doc);
            ctx.rebuild();
        });
    }
    let b = w::vbox(8);
    b.append(&fields(vec![
        labelled("Product", &product).upcast(),
        labelled("Final factories", &factories).upcast(),
        labelled("In system (optional)", &sys_entry).upcast(),
        create.upcast(),
    ]));
    let hint = w::line(
        "With a system, planets are picked from the kinds it has. Raw resources are paired on planets that have both, and characters with free planet slots get the planets.",
        &["body-small", "dim"],
    );
    hint.set_wrap(true);
    hint.set_xalign(0.0);
    b.append(&hint);
    w::card_with("New plan", None, &[], &b)
}

fn plan_picker(ctx: &Ctx, plans: &[PiPlan], open: usize) -> gtk::Box {
    let row = w::hbox(8);
    let labels: Vec<String> = plans.iter().map(|p| p.name.clone()).collect();
    let labels: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let dd = w::dropdown(&labels, open as u32);
    dd.set_hexpand(true);
    {
        let ctx = ctx.clone();
        let ids: Vec<String> = plans.iter().map(|p| p.id.clone()).collect();
        dd.connect_selected_notify(move |d| {
            if let Some(id) = ids.get(d.selected() as usize) {
                ctx.remember(PLAN_KEY, id.clone());
                let ctx = ctx.clone();
                gtk::glib::idle_add_local_once(move || ctx.rebuild());
            }
        });
    }
    let del = w::icon_button("delete", "Delete plan");
    {
        let ctx = ctx.clone();
        let (id, name) = (plans[open].id.clone(), plans[open].name.clone());
        del.connect_clicked(move |_| {
            let (ctx2, id) = (ctx.clone(), id.clone());
            w::confirm(&ctx, "Delete plan?", &format!("{name} will be removed. Your colonies aren't touched."), "Delete", true, move || {
                let mut doc = pl::load();
                doc.plans.retain(|p| p.id != id);
                pl::save(ctx2.core(), &doc);
                ctx2.rebuild();
            });
        });
    }
    row.append(&w::line("Plan", &["label-large"]));
    row.append(&dd);
    row.append(&del);
    row
}

fn plan_view(ctx: &Ctx, plan: &PiPlan) -> gtk::Box {
    let v = w::vbox(16);
    let chars = ctx.core().characters();
    let progress = pl::plan_progress(plan, &chars, Utc::now());
    let built = progress.iter().filter(|b| **b).count();
    let rate = PI.schematic_for(plan.product).map(|s| s.per_hour() * plan.factories as f64).unwrap_or(0.0);
    let assigned = plan.planets.iter().filter(|p| p.character_id.is_some()).count();
    let mut chars_used: Vec<i64> = plan.planets.iter().filter_map(|p| p.character_id).collect();
    chars_used.sort();
    chars_used.dedup();

    let value_tile = w::stat_tile("Output value", "…", Some(&format!("{} {} / h", fmt::decimal(rate, 1), PI.name(plan.product))));
    let tiles = w::tile_row(vec![
        w::stat_tile("Planets", &fmt::int(plan.planets.len() as i64), Some(&format!("{} on {} character(s)", assigned, chars_used.len()))),
        w::stat_tile("Built", &format!("{built} / {}", plan.planets.len()), Some(&format!("{}% matched by your colonies", if plan.planets.is_empty() { 0 } else { built * 100 / plan.planets.len() }))),
        value_tile.clone(),
    ]);
    v.append(&tiles);
    if assigned < plan.planets.len() {
        v.append(&w::banner(
            "person_off",
            &format!("{} planet(s) have no character: your characters' planet slots are full. Train Interplanetary Consolidation or add characters.", plan.planets.len() - assigned),
            w::BannerKind::Notice,
        ));
    }
    {
        let product = plan.product;
        let value_tile = value_tile.clone();
        common::load_prices(ctx, vec![product], false, move |p| {
            let price = p.sell().get(&product).copied();
            // stat_tile's second child is its value label
            if let Some(label) = value_tile.first_child().and_then(|c| c.next_sibling()).and_then(|c| c.downcast::<gtk::Label>().ok()) {
                label.set_text(&price.map(|pr| format!("{} / h", fmt::isk_short(pr * rate))).unwrap_or_else(|| "—".into()));
            }
        });
    }

    // the planets
    let list = w::vbox(0);
    let char_options: Vec<(Option<i64>, String)> = std::iter::once((None, "Nobody".to_string())).chain(chars.iter().map(|c| (Some(c.id), c.name()))).collect();
    let char_labels: Vec<&str> = char_options.iter().map(|c| c.1.as_str()).collect();
    let kind_labels: Vec<&str> = PI.kinds.iter().map(|k| k.label).collect();
    for (i, (pp, done)) in plan.planets.iter().zip(&progress).enumerate() {
        let row = w::hbox(12);
        row.add_css_class("list-item");
        let status = if *done { w::status("Built", w::Status::Good) } else { w::status("To build", w::Status::Neutral) };
        status.set_width_chars(9);
        row.append(&status);
        let what = w::vbox(2);
        what.set_hexpand(true);
        match &pp.role {
            PlanetRole::Extract { resources } => {
                let names: Vec<String> = resources.iter().map(|r| PI.name(*r)).collect();
                let line = w::hbox(6);
                line.append(&tier_badge(0));
                line.append(&w::line(&format!("Extract {}", names.join(" + ")), &["body-medium"]));
                what.append(&line);
                let p1: Vec<String> = resources.iter().filter_map(|r| PI.used_by(*r).into_iter().find(|s| PI.tier(s.output.0) == Some(1)).map(|s| PI.name(s.output.0))).collect();
                what.append(&w::line(&format!("Refine to {} on the spot", p1.join(" and ")), &["body-small", "dim"]));
            }
            PlanetRole::Factory { schematic, count } => {
                if let Some(s) = PI.schematics.get(schematic) {
                    let line = w::hbox(6);
                    line.append(&commodity_cell(s.output.0));
                    line.append(&w::line(&format!("× {count} factories"), &["body-medium"]));
                    what.append(&line);
                    what.append(&w::line(&format!("{} / h from {}", fmt::decimal(s.per_hour() * *count as f64, 1), s.inputs.iter().map(|i| PI.name(i.0)).collect::<Vec<_>>().join(", ")), &["body-small", "dim"]));
                }
            }
        }
        row.append(&what);
        let kind = w::dropdown(&kind_labels, PI.kinds.iter().position(|k| k.key == pp.kind).unwrap_or(0) as u32);
        let who = w::dropdown(&char_labels, char_options.iter().position(|c| c.0 == pp.character_id).unwrap_or(0) as u32);
        let place = w::line(&pp.system_id.map(system_name).unwrap_or_else(|| "Any system".into()), &["body-small", "dim"]);
        place.set_width_chars(12);
        row.append(&place);
        row.append(&kind);
        row.append(&who);
        let remove = w::icon_button("close", "Remove from plan");
        row.append(&remove);
        {
            let (ctx, id) = (ctx.clone(), plan.id.clone());
            kind.connect_selected_notify(move |d| {
                let key = PI.kinds.get(d.selected() as usize).map(|k| k.key.to_string());
                edit(&ctx, &id, |p| {
                    if let (Some(pp), Some(k)) = (p.planets.get_mut(i), key) {
                        pp.kind = k;
                    }
                });
            });
        }
        {
            let (ctx, id) = (ctx.clone(), plan.id.clone());
            let opts: Vec<Option<i64>> = char_options.iter().map(|c| c.0).collect();
            who.connect_selected_notify(move |d| {
                let c = opts.get(d.selected() as usize).copied().flatten();
                edit(&ctx, &id, |p| {
                    if let Some(pp) = p.planets.get_mut(i) {
                        pp.character_id = c;
                    }
                });
            });
        }
        {
            let (ctx, id) = (ctx.clone(), plan.id.clone());
            remove.connect_clicked(move |_| {
                edit(&ctx, &id, |p| {
                    if i < p.planets.len() {
                        p.planets.remove(i);
                    }
                });
                ctx.rebuild();
            });
        }
        list.append(&row);
    }
    let sw = gtk::ScrolledWindow::new();
    sw.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    sw.set_child(Some(&list));
    v.append(&w::card_with(
        &plan.name,
        Some("A planet counts as built when one of the character's colonies of that kind runs it (extractors for the resources, or at least that many factories)"),
        &[],
        &sw,
    ));

    // what each character needs to do
    let per_char = w::vbox(6);
    for c in &chars {
        let mine: Vec<usize> = plan.planets.iter().enumerate().filter(|(_, p)| p.character_id == Some(c.id)).map(|(i, _)| i).collect();
        if mine.is_empty() {
            continue;
        }
        let todo = mine.iter().filter(|i| !progress[**i]).count();
        let info = pl::character(c);
        per_char.append(&w::list_row(
            None,
            &c.name(),
            Some(&format!(
                "{} planet(s) in this plan, {} still to build · {} of {} planet slots used",
                mine.len(),
                todo,
                info.planets,
                info.max_planets
            )),
            None,
        ));
    }
    if per_char.first_child().is_some() {
        v.append(&w::card_with("By character", None, &[], &per_char));
    }
    v
}

/// Changes one plan and saves it.
fn edit(ctx: &Ctx, id: &str, f: impl FnOnce(&mut PiPlan)) {
    let mut doc = pl::load();
    if let Some(p) = doc.plans.iter_mut().find(|p| p.id == id) {
        f(p);
        pl::save(ctx.core(), &doc);
    }
}
