//! Industry → Facilities: the stations and structures jobs run in (structure type, rigs, system, tax), which one
//! each activity uses, whose skills and which hub's prices calculations use, the rules from CCP's patch notes that
//! the math needs, and the state of the game data (CCP's static data build).

use super::common::{self, CSS};
use super::form::{self, fields, labelled, number_entry, percent_text, security_label, suggest_entry, system_name};
use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::rt;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::blueprints::{Activity, BonusKind, REFINERY_GROUP, INDUSTRY};
use podstack_core::features::manufacturing::{self as mf, Facility, Rules, RULES_KEY};
use podstack_core::sde::{SecurityClass, TRADE_HUBS};
use podstack_core::{fmt, static_data};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Facilities));
    body.append(&facilities_card(ctx));
    body.append(&defaults_card(ctx));
    body.append(&calculation_card(ctx));
    body.append(&rules_card(ctx));
    body.append(&game_data_card(ctx));
    let page = Page::new("Industry", body).static_page();
    // a new CCP build changes structures and rigs; saving here changes the lists
    let ctx2 = ctx.clone();
    ctx.on_event(&page.content, move |e| {
        if matches!(e, podstack_core::Event::Doc(d) if d == "sde") {
            ctx2.rebuild();
        }
    });
    page
}

fn structure_name(f: &Facility) -> String {
    f.structure.map(|s| INDUSTRY.type_name(s)).unwrap_or_else(|| "NPC station".into())
}

fn facilities_card(ctx: &Ctx) -> gtk::Box {
    let doc = mf::load(ctx.core());
    let list = w::vbox(0);
    if doc.facilities.is_empty() {
        list.append(&w::empty_state(
            "factory",
            "No facilities yet",
            "Until you add one, jobs are worked out in an NPC station in Jita. Add the stations and structures you build in, with their rigs and tax, to get exact numbers.",
        ));
    }
    for f in &doc.facilities {
        let sub = {
            let rigs = f.rigs.len();
            let tax = format!("{}% tax", percent_text(f.tax(&mf::rules(ctx.core()))));
            let rigs = if rigs == 0 { "no rigs".to_string() } else { format!("{rigs} rig{}", if rigs == 1 { "" } else { "s" }) };
            format!("{} · {} · {} · {}", system_name(f.system_id), structure_name(f), rigs, tax)
        };
        let lead = security_label(f.system_id);
        lead.set_width_chars(4);
        let actions = w::hbox(4);
        let edit = w::icon_button("edit", "Edit");
        let del = w::icon_button("delete", "Delete");
        {
            let (ctx, f) = (ctx.clone(), f.clone());
            edit.connect_clicked(move |_| editor(&ctx, Some(f.clone())));
        }
        {
            let (ctx, f) = (ctx.clone(), f.clone());
            del.connect_clicked(move |_| {
                let ctx2 = ctx.clone();
                let id = f.id.clone();
                w::confirm(&ctx, "Delete facility?", &format!("{} will be removed; activities using it go back to the NPC station default.", f.name), "Delete", true, move || {
                    let mut doc = mf::load(ctx2.core());
                    doc.facilities.retain(|x| x.id != id);
                    doc.defaults.retain(|_, v| *v != id);
                    mf::save(ctx2.core(), &doc);
                    ctx2.rebuild();
                });
            });
        }
        actions.append(&edit);
        actions.append(&del);
        list.append(&w::list_row(Some(lead.upcast_ref()), &f.name, Some(&sub), Some(actions.upcast_ref())));
    }
    let add = w::button_icon("add", "Add facility", Btn::Tonal);
    {
        let ctx = ctx.clone();
        add.connect_clicked(move |_| editor(&ctx, None));
    }
    w::card_with("Facilities", Some("Where your jobs run"), &[add.upcast()], &list)
}

/// Opens the add-facility dialog with `system` filled in (from the Systems tab).
pub(super) fn new_facility_in(ctx: &Ctx, system: i64) {
    editor_with(ctx, None, Some(system));
}

fn editor(ctx: &Ctx, existing: Option<Facility>) {
    editor_with(ctx, existing, None);
}

/// The add / edit dialog, with a live preview of the bonuses and the system's cost indices.
fn editor_with(ctx: &Ctx, existing: Option<Facility>, system: Option<i64>) {
    let editing = existing.is_some();
    let f = Rc::new(RefCell::new(existing.unwrap_or_else(|| Facility {
        id: format!("f{}", chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()),
        name: String::new(),
        system_id: system.unwrap_or(TRADE_HUBS[0].2),
        ..Default::default()
    })));
    let (win, content, actions) = w::dialog(ctx, if editing { "Edit facility" } else { "Add facility" }, 560);

    let name = gtk::Entry::new();
    name.set_text(&f.borrow().name);
    name.set_placeholder_text(Some("e.g. Home Raitaru"));
    name.set_hexpand(true);

    let preview = w::vbox(6);
    let indices = w::vbox(4);
    let refresh: Rc<dyn Fn()> = {
        let (f, preview) = (f.clone(), preview.clone());
        Rc::new(move || fill_preview(&preview, &f.borrow()))
    };

    let system = suggest_entry("Solar system", form::search_systems, {
        let (f, refresh, indices, ctx) = (f.clone(), refresh.clone(), indices.clone(), ctx.clone());
        move |id, _| {
            f.borrow_mut().system_id = id;
            refresh();
            load_indices(&ctx, &indices, id);
        }
    });
    system.set_text(&system_name(f.borrow().system_id));

    // Structures: "NPC station" first, then every structure type in the static data.
    let structures: Vec<(Option<i64>, String)> =
        std::iter::once((None, "NPC station".to_string())).chain(INDUSTRY.structures.iter().map(|s| (Some(s.id), s.name.clone()))).collect();
    let labels: Vec<&str> = structures.iter().map(|s| s.1.as_str()).collect();
    let current = structures.iter().position(|s| s.0 == f.borrow().structure).unwrap_or(0) as u32;
    let structure = w::dropdown(&labels, current);

    let rigs_box = w::vbox(6);
    let fill_rigs: Rc<dyn Fn()> = {
        let (f, rigs_box, refresh) = (f.clone(), rigs_box.clone(), refresh.clone());
        Rc::new(move || {
            while let Some(c) = rigs_box.first_child() {
                rigs_box.remove(&c);
            }
            let size = f.borrow().structure.and_then(|s| INDUSTRY.structure(s)).map(|s| s.rig_size);
            let Some(size) = size else {
                rigs_box.append(&w::line("NPC stations have no rigs.", &["dim"]));
                return;
            };
            // Rigs that fit this structure size; refineries take reactor rigs, the others engineering rigs.
            let refinery = f.borrow().structure.and_then(|s| INDUSTRY.structure(s)).is_some_and(|s| s.group == REFINERY_GROUP);
            let options: Vec<(i64, String)> = INDUSTRY
                .rigs
                .iter()
                .filter(|r| r.size == size && r.bonuses.iter().any(|b| (b.0 == Activity::Reaction) == refinery))
                .map(|r| (r.id, r.name.clone()))
                .collect();
            let options = Rc::new(options);
            let labels: Vec<String> = std::iter::once("No rig".to_string()).chain(options.iter().map(|o| o.1.clone())).collect();
            let labels: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
            f.borrow_mut().rigs.retain(|r| options.iter().any(|o| o.0 == *r));
            for slot in 0..3 {
                let chosen = f.borrow().rigs.get(slot).copied();
                let sel = chosen.and_then(|c| options.iter().position(|o| o.0 == c)).map(|i| i as u32 + 1).unwrap_or(0);
                let dd = w::dropdown(&labels, sel);
                dd.set_hexpand(true);
                let (f, options, refresh) = (f.clone(), options.clone(), refresh.clone());
                dd.connect_selected_notify(move |d| {
                    let mut fb = f.borrow_mut();
                    let picked = (d.selected() as usize).checked_sub(1).and_then(|i| options.get(i)).map(|o| o.0);
                    // rigs are kept in slot order; an emptied slot drops out
                    let mut slots: Vec<Option<i64>> = (0..3).map(|i| fb.rigs.get(i).copied()).collect();
                    slots[slot] = picked;
                    fb.rigs = slots.into_iter().flatten().collect();
                    drop(fb);
                    refresh();
                });
                rigs_box.append(&dd);
            }
            refresh();
        })
    };
    {
        let (f, fill_rigs) = (f.clone(), fill_rigs.clone());
        let structures = structures.clone();
        structure.connect_selected_notify(move |d| {
            f.borrow_mut().structure = structures.get(d.selected() as usize).and_then(|s| s.0);
            fill_rigs();
        });
    }

    let rules = mf::rules(ctx.core());
    let tax = number_entry(&f.borrow().tax.map(percent_text).unwrap_or_default(), 8);
    tax.set_placeholder_text(Some(&format!("{} (default)", percent_text(rules.npc_facility_tax))));

    content.append(&labelled("Name", &name));
    content.append(&fields(vec![labelled("System", &system).upcast(), labelled("Structure", &structure).upcast(), labelled("Facility tax %", &tax).upcast()]));
    content.append(&labelled("Rigs", &rigs_box));
    content.append(&w::line("Bonuses here", &["title-small"]));
    content.append(&preview);
    content.append(&w::line("System cost indices (live)", &["title-small"]));
    content.append(&indices);
    fill_rigs();
    load_indices(ctx, &indices, f.borrow().system_id);

    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button("Save", Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (win, ctx) = (win.clone(), ctx.clone());
        save.connect_clicked(move |_| {
            let mut fac = f.borrow().clone();
            fac.name = name.text().trim().to_string();
            if fac.name.is_empty() {
                fac.name = format!("{} {}", system_name(fac.system_id), structure_name(&fac));
            }
            let t = tax.text();
            fac.tax = if t.trim().is_empty() { None } else { form::parse_amount(&t).map(|p| p / 100.0) };
            let mut doc = mf::load(ctx.core());
            match doc.facilities.iter_mut().find(|x| x.id == fac.id) {
                Some(x) => *x = fac,
                None => doc.facilities.push(fac),
            }
            mf::save(ctx.core(), &doc);
            win.close();
            ctx.rebuild();
        });
    }
    actions.append(&cancel);
    actions.append(&save);
    win.present();
}

/// What the structure and rigs do, in words, at the facility's security level.
fn fill_preview(b: &gtk::Box, f: &Facility) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
    let mut lines: Vec<String> = Vec::new();
    if let Some(s) = f.structure.and_then(|s| INDUSTRY.structure(s)) {
        for a in Activity::ALL {
            let parts: Vec<String> = [(BonusKind::Material, "materials"), (BonusKind::Time, "time"), (BonusKind::Cost, "job cost")]
                .iter()
                .filter_map(|(k, label)| {
                    let m = s.multiplier(a, *k);
                    (m != 1.0).then(|| format!("{}% {label}", signed_percent(m - 1.0)))
                })
                .collect();
            if !parts.is_empty() {
                lines.push(format!("{} (structure): {}", a.label(), parts.join(", ")));
            }
        }
    }
    let sec = match f.security() {
        SecurityClass::High => 0,
        SecurityClass::Low => 1,
        SecurityClass::Null => 2,
    };
    for r in f.rigs.iter().filter_map(|r| INDUSTRY.rig(*r)) {
        match r.security[sec] {
            None => lines.push(format!("{}: no effect at this security level", r.name)),
            Some(scale) => {
                let parts: Vec<String> = r
                    .bonuses
                    .iter()
                    .map(|(a, k, p, filter)| {
                        let what = match k {
                            BonusKind::Material => "materials",
                            BonusKind::Time => "time",
                            BonusKind::Cost => "job cost",
                        };
                        let target = filter.and_then(|id| INDUSTRY.filters.get(&id)).map(|f| format!(" for {}", f.name)).unwrap_or_default();
                        format!("{} {}% {what}{target}", a.label(), signed_percent(p * scale / 100.0))
                    })
                    .collect();
                lines.push(format!("{}: {}", short_rig_name(&r.name), parts.join(", ")));
            }
        }
    }
    if f.structure.is_some() && !f.supports(&INDUSTRY, Activity::Reaction) && f.structure.and_then(|s| INDUSTRY.structure(s)).is_some_and(|s| s.group == REFINERY_GROUP) {
        lines.push("Reactions aren't possible in high security space.".into());
    }
    if lines.is_empty() {
        lines.push("No bonuses: materials and times as on the blueprint.".into());
    }
    for l in lines {
        let label = w::line(&l, &["body-small"]);
        label.set_wrap(true);
        label.set_xalign(0.0);
        b.append(&label);
    }
}

fn signed_percent(fraction: f64) -> String {
    let p = fraction * 100.0;
    let s = format!("{:.2}", p.abs());
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if p < 0.0 {
        format!("−{s}")
    } else {
        format!("+{s}")
    }
}

/// "Standup M-Set Equipment Manufacturing Material Efficiency I" → "M-Set Equipment Manufacturing ME I".
fn short_rig_name(n: &str) -> String {
    n.trim_start_matches("Standup ").replace("Material Efficiency", "ME").replace("Time Efficiency", "TE")
}

fn load_indices(ctx: &Ctx, b: &gtk::Box, system: i64) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
    b.append(&w::line("Loading…", &["dim"]));
    let core = ctx.core().clone();
    let b = b.clone();
    rt::run(async move { core.market.cost_indices(&core.esi, false).await }, move |r| {
        while let Some(c) = b.first_child() {
            b.remove(&c);
        }
        match r {
            Ok(all) => {
                let rows: Vec<(&str, String)> = Activity::ALL
                    .iter()
                    .map(|a| (a.label(), mf::cost_index(Some(&all), system, *a)))
                    .map(|(l, v)| (l, format!("{}%", percent_text(v))))
                    .collect();
                b.append(&w::key_values(&rows));
            }
            Err(err) => b.append(&w::line(&format!("Couldn't load cost indices: {err}"), &["error-text"])),
        }
    });
}

fn defaults_card(ctx: &Ctx) -> gtk::Box {
    let doc = mf::load(ctx.core());
    let grid = w::vbox(8);
    let options: Vec<(Option<String>, String)> =
        std::iter::once((None, "NPC station in Jita".to_string())).chain(doc.facilities.iter().map(|f| (Some(f.id.clone()), f.label()))).collect();
    let options = Rc::new(options);
    let labels: Vec<&str> = options.iter().map(|o| o.1.as_str()).collect();
    for a in Activity::ALL {
        let current = doc.defaults.get(a.key()).and_then(|id| options.iter().position(|o| o.0.as_deref() == Some(id))).unwrap_or(0) as u32;
        let dd = w::dropdown(&labels, current);
        dd.set_hexpand(true);
        let (ctx, options) = (ctx.clone(), options.clone());
        dd.connect_selected_notify(move |d| {
            let mut doc = mf::load(ctx.core());
            match options.get(d.selected() as usize).and_then(|o| o.0.clone()) {
                Some(id) => {
                    doc.defaults.insert(a.key().into(), id);
                }
                None => {
                    doc.defaults.remove(a.key());
                }
            }
            mf::save(ctx.core(), &doc);
        });
        let row = w::hbox(12);
        let l = w::line(a.label(), &["body-medium"]);
        l.set_width_chars(14);
        l.set_xalign(0.0);
        row.append(&l);
        row.append(&dd);
        grid.append(&row);
    }
    w::card_with("Default facility per activity", Some("The calculator and planner use these unless you pick another"), &[], &grid)
}

fn calculation_card(ctx: &Ctx) -> gtk::Box {
    let doc = mf::load(ctx.core());
    let chars = ctx.core().characters();
    let b = w::vbox(8);
    // Skills: the best of all characters (the default), all V, or one character. `skills_of` 0 means all V.
    let mut options: Vec<(Option<i64>, String)> = vec![(None, "Best of all characters".into()), (Some(0), "All skills at V".into())];
    options.extend(chars.iter().map(|c| (Some(c.id), c.name())));
    let options = Rc::new(options);
    let labels: Vec<&str> = options.iter().map(|o| o.1.as_str()).collect();
    let current = options.iter().position(|o| o.0 == doc.skills_of).unwrap_or(0) as u32;
    let skills = w::dropdown(&labels, current);
    {
        let (ctx, options) = (ctx.clone(), options.clone());
        skills.connect_selected_notify(move |d| {
            let mut doc = mf::load(ctx.core());
            doc.skills_of = options.get(d.selected() as usize).and_then(|o| o.0);
            mf::save(ctx.core(), &doc);
        });
    }
    let hubs: Vec<&str> = TRADE_HUBS.iter().map(|h| h.0).collect();
    let hub_now = common::price_hub(&doc).0;
    let hub = w::dropdown(&hubs, hubs.iter().position(|h| *h == hub_now).unwrap_or(0) as u32);
    hub.connect_selected_notify({
        let ctx = ctx.clone();
        move |d| {
            let mut doc = mf::load(ctx.core());
            doc.price_hub = TRADE_HUBS.get(d.selected() as usize).map(|h| h.0.to_string());
            mf::save(ctx.core(), &doc);
        }
    });
    b.append(&fields(vec![labelled("Skills for times and invention", &skills).upcast(), labelled("Prices from", &hub).upcast()]));
    let note = w::line("Materials are priced at the hub's lowest sell order, products at its sell and buy orders.", &["body-small", "dim"]);
    note.set_wrap(true);
    note.set_xalign(0.0);
    b.append(&note);
    w::card_with("Calculations", None, &[], &b)
}

fn rules_card(ctx: &Ctx) -> gtk::Box {
    let rules = mf::rules(ctx.core());
    let b = w::vbox(8);
    let intro = w::line(
        "Everything else (blueprints, structure and rig bonuses, skill bonuses, cost indices, adjusted prices) comes from CCP's data and updates by itself. These few rules are only published in patch notes: change them here if CCP does.",
        &["body-small", "dim"],
    );
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    b.append(&intro);
    let pct = |v: f64| number_entry(&percent_text(v), 7);
    let num = |v: f64| number_entry(&format!("{v}"), 5);
    let scc = pct(rules.scc_surcharge);
    let npc = pct(rules.npc_facility_tax);
    let inv_share = pct(rules.invention_eiv_share);
    let copy_share = pct(rules.copying_eiv_share);
    let inv_me = num(rules.invented_me as f64);
    let inv_te = num(rules.invented_te as f64);
    let me = num(rules.assumed_me as f64);
    let te = num(rules.assumed_te as f64);
    let sci = num(rules.invention_science_divisor);
    let enc = num(rules.invention_encryption_divisor);
    b.append(&fields(vec![
        labelled("SCC surcharge %", &scc).upcast(),
        labelled("NPC facility tax %", &npc).upcast(),
        labelled("Invention cost % of EIV", &inv_share).upcast(),
        labelled("Copying cost % of EIV", &copy_share).upcast(),
    ]));
    b.append(&fields(vec![
        labelled("Invented ME", &inv_me).upcast(),
        labelled("Invented TE", &inv_te).upcast(),
        labelled("ME of unowned blueprints", &me).upcast(),
        labelled("TE of unowned blueprints", &te).upcast(),
        labelled("Science skill divisor", &sci).upcast(),
        labelled("Encryption skill divisor", &enc).upcast(),
    ]));
    let save = w::button("Save rules", Btn::Tonal);
    let reset = w::button("Reset to defaults", Btn::Text);
    {
        let ctx = ctx.clone();
        save.connect_clicked(move |_| {
            let p = |e: &gtk::Entry, old: f64| form::parse_amount(&e.text()).map(|v| v / 100.0).unwrap_or(old);
            let n = |e: &gtk::Entry, old: f64| form::parse_amount(&e.text()).unwrap_or(old);
            let old = mf::rules(ctx.core());
            let r = Rules {
                scc_surcharge: p(&scc, old.scc_surcharge),
                npc_facility_tax: p(&npc, old.npc_facility_tax),
                invention_eiv_share: p(&inv_share, old.invention_eiv_share),
                copying_eiv_share: p(&copy_share, old.copying_eiv_share),
                invented_me: n(&inv_me, old.invented_me as f64) as i64,
                invented_te: n(&inv_te, old.invented_te as f64) as i64,
                assumed_me: n(&me, old.assumed_me as f64).clamp(0.0, 10.0) as i64,
                assumed_te: n(&te, old.assumed_te as f64).clamp(0.0, 20.0) as i64,
                invention_science_divisor: n(&sci, old.invention_science_divisor).max(1.0),
                invention_encryption_divisor: n(&enc, old.invention_encryption_divisor).max(1.0),
            };
            ctx.core().set_setting(RULES_KEY, &r);
            ctx.toast("Rules saved");
        });
    }
    {
        let ctx = ctx.clone();
        reset.connect_clicked(move |_| {
            ctx.core().set_setting(RULES_KEY, &Rules::default());
            ctx.rebuild();
        });
    }
    let actions = w::hbox(8);
    actions.append(&save);
    actions.append(&reset);
    b.append(&actions);
    w::card_with("Rules", None, &[], &b)
}

fn game_data_card(ctx: &Ctx) -> gtk::Box {
    let s = static_data::status();
    let b = w::vbox(8);
    let rows = vec![
        ("CCP build", s.build.to_string()),
        ("Released", s.release.map(fmt::datetime).unwrap_or_else(|| "—".into())),
        ("Source", if s.bundled { "Bundled with PodStack".into() } else { "Downloaded from CCP".into() }),
        ("Last checked", s.checked.map(fmt::relative).unwrap_or_else(|| "Not yet".into())),
        ("Blueprints and formulas", fmt::int(INDUSTRY.blueprints.len() as i64)),
    ];
    b.append(&w::key_values(&rows));
    let note = w::line("PodStack checks CCP's static data every few hours and switches to a new build while running.", &["body-small", "dim"]);
    note.set_wrap(true);
    note.set_xalign(0.0);
    b.append(&note);
    let check = w::button_icon("sync", "Check now", Btn::Tonal);
    let busy = Rc::new(Cell::new(false));
    {
        let ctx = ctx.clone();
        check.connect_clicked(move |btn| {
            if busy.replace(true) {
                return;
            }
            btn.set_sensitive(false);
            ctx.toast("Checking CCP's game data… (a new build is a ~100 MB download)");
            let core = ctx.core().clone();
            let (ctx, btn, busy) = (ctx.clone(), btn.clone(), busy.clone());
            rt::run(async move { core.update_static_data(true).await }, move |_| {
                busy.set(false);
                btn.set_sensitive(true);
                ctx.rebuild();
            });
        });
    }
    w::card_with("Game data", None, &[check.upcast()], &b)
}
