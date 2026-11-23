//! Courier → Finder: a region's public courier contracts with their pickup and delivery resolved and their routes
//! worked out, filtered to what the hauler can carry and wants to fly, best paying first.

use super::{amount_text, countdown_cell, header, jumps_cell, m3, min_sec_cell, opt_amount, place_cell, place_key, set_quietly, View};
use crate::ctx::{Ctx, Page};
use crate::pages::contracts::common::open_contract;
use crate::pages::industry::form::{fields, labelled, number_entry, search_systems, suggest_entry};
use crate::rt;
use crate::widgets::{self as w, BannerKind, Btn, Table};
use chrono::{DateTime, Utc};
use gtk::prelude::*;
use podstack_core::domains::contracts::Contract;
use podstack_core::features::courier::{self as cr, FinderPrefs, FinderProgress, FinderRow, FinderSort, Hull};
use podstack_core::fmt;
use podstack_core::sde::{RoutePref, MAP};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

/// Rows listed at most: the table stays quick, and the filters find the rest.
const LIMIT: usize = 1000;

/// What was loaded last, kept while the app runs so coming back to the tab doesn't load it all again.
struct Loaded {
    region: i64,
    rows: Rc<Vec<FinderRow>>,
    at: DateTime<Utc>,
}

thread_local! {
    static LOADED: RefCell<Option<Loaded>> = const { RefCell::new(None) };
    /// The region being loaded, if any.
    static LOADING: Cell<Option<i64>> = const { Cell::new(None) };
    /// Pages waiting for the load: the one that started it, and any built again meanwhile (switching views and back).
    static WAITING: RefCell<Vec<Weak<Ui>>> = const { RefCell::new(Vec::new()) };
}

/// The pages still shown that wait for the load.
fn waiting() -> Vec<Rc<Ui>> {
    WAITING.with(|w| {
        let mut w = w.borrow_mut();
        w.retain(|u| u.upgrade().is_some_and(|u| u.results.root().is_some()));
        w.iter().filter_map(|u| u.upgrade()).collect()
    })
}

fn region_name(id: i64) -> String {
    MAP.region_name(id).unwrap_or("this region").to_string()
}

/// The widgets the filters and results live in.
struct Ui {
    ctx: Ctx,
    prefs: Rc<RefCell<FinderPrefs>>,
    pref: RoutePref,
    status: gtk::Label,
    progress: gtk::ProgressBar,
    results: gtk::Box,
    refresh: gtk::Button,
}

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&header(ctx, View::Finder));
    let doc = cr::load_doc();
    let prefs = Rc::new(RefCell::new(doc.finder.clone()));

    let status = w::line("", &["body-small", "dim"]);
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_wrap(true);
    let progress = w::progress(0.0);
    progress.set_visible(false);
    progress.set_hexpand(true);
    let results = w::vbox(12);
    results.set_vexpand(true);
    let refresh = w::button_icon("refresh", "Refresh", Btn::Tonal);
    let ui = Rc::new(Ui { ctx: ctx.clone(), prefs: prefs.clone(), pref: doc.route, status, progress, results, refresh });

    body.append(&filters(&ui, &doc.hulls));
    let status_row = w::hbox(12);
    status_row.append(&ui.status);
    status_row.append(&ui.refresh);
    body.append(&status_row);
    body.append(&ui.progress);
    body.append(&ui.results);
    {
        let ui2 = ui.clone();
        ui.refresh.connect_clicked(move |_| load(&ui2, true));
    }

    let region = prefs.borrow().region;
    let have = LOADED.with(|l| l.borrow().as_ref().is_some_and(|l| l.region == region));
    if have {
        render(&ui);
    } else {
        load(&ui, false);
    }
    Page::new("Contracts", body).static_page().no_scroll()
}

fn filters(ui: &Rc<Ui>, hulls: &[Hull]) -> gtk::Box {
    let p = ui.prefs.borrow().clone();
    let changed: Rc<dyn Fn()> = {
        let ui = ui.clone();
        Rc::new(move || {
            cr::save_finder(&ui.prefs.borrow());
            render(&ui);
        })
    };

    // region
    let regions: Vec<(i64, String)> = MAP.market_regions().cloned().collect();
    let labels: Vec<&str> = regions.iter().map(|r| r.1.as_str()).collect();
    let region = w::dropdown(&labels, regions.iter().position(|r| r.0 == p.region).unwrap_or(0) as u32);
    region.set_enable_search(true);
    {
        let ui = ui.clone();
        region.connect_selected_notify(move |d| {
            let Some(r) = regions.get(d.selected() as usize) else { return };
            ui.prefs.borrow_mut().region = r.0;
            cr::save_finder(&ui.prefs.borrow());
            let have = LOADED.with(|l| l.borrow().as_ref().is_some_and(|l| l.region == r.0));
            if have {
                render(&ui);
            } else {
                load(&ui, false);
            }
        });
    }

    // where the hauler is
    let origin = suggest_entry("Hauler's system", search_systems, {
        let (ui, changed) = (ui.clone(), changed.clone());
        move |id, _| {
            ui.prefs.borrow_mut().origin = Some(id);
            changed();
        }
    });
    origin.set_width_chars(16);
    origin.set_hexpand(false);
    if let Some(id) = p.origin {
        set_quietly(&origin, &super::system_name(Some(id)));
    }
    {
        // clearing the entry forgets the system
        let (ui, changed) = (ui.clone(), changed.clone());
        origin.connect_search_changed(move |e| {
            if e.text().trim().is_empty() && ui.prefs.borrow().origin.is_some() {
                ui.prefs.borrow_mut().origin = None;
                changed();
            }
        });
    }
    let located: Vec<(String, i64)> = ui
        .ctx
        .core()
        .characters()
        .iter()
        .filter_map(|c| Some((c.name(), c.location.as_ref()?.system.system_id?)))
        .collect();
    let origin_box = w::hbox(4);
    origin_box.append(&origin);
    if !located.is_empty() {
        let mut labels: Vec<String> = vec!["Where a character is…".into()];
        labels.extend(located.iter().map(|(n, s)| format!("{n} ({})", super::system_name(Some(*s)))));
        let labels: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let who = w::dropdown(&labels, 0);
        who.set_tooltip_text(Some("Count jumps from one of your characters' current system"));
        let (ui, changed, origin) = (ui.clone(), changed.clone(), origin.clone());
        who.connect_selected_notify(move |d| {
            let Some((_, sys)) = (d.selected() as usize).checked_sub(1).and_then(|i| located.get(i)) else { return };
            ui.prefs.borrow_mut().origin = Some(*sys);
            set_quietly(&origin, &super::system_name(Some(*sys)));
            changed();
            // back to the prompt, so picking the same character again works
            let d = d.clone();
            gtk::glib::idle_add_local_once(move || d.set_selected(0));
        });
        origin_box.append(&who);
    }

    let entry = |text: String, width: i32, ph: &str| {
        let e = number_entry(&text, width);
        e.set_placeholder_text(Some(ph));
        e
    };
    let max_jumps = entry(p.max_pickup_jumps.map(|j| j.to_string()).unwrap_or_default(), 5, "any");
    let max_volume = entry(p.max_volume.map(amount_text).unwrap_or_default(), 10, "any");
    let max_collateral = entry(p.max_collateral.map(amount_text).unwrap_or_default(), 10, "any");
    let min_reward = entry(p.min_reward.map(amount_text).unwrap_or_default(), 10, "any");
    let min_per_jump = entry(p.min_isk_per_jump.map(amount_text).unwrap_or_default(), 10, "any");
    {
        let (ui, changed) = (ui.clone(), changed.clone());
        max_jumps.connect_changed(move |e| {
            ui.prefs.borrow_mut().max_pickup_jumps = e.text().trim().parse::<usize>().ok();
            changed();
        });
    }
    type Setter = fn(&mut FinderPrefs, Option<f64>);
    let setters: [(&gtk::Entry, Setter); 4] = [
        (&max_volume, |p, v| p.max_volume = v),
        (&max_collateral, |p, v| p.max_collateral = v),
        (&min_reward, |p, v| p.min_reward = v),
        (&min_per_jump, |p, v| p.min_isk_per_jump = v),
    ];
    for (e, set) in setters {
        let (ui, changed) = (ui.clone(), changed.clone());
        e.connect_changed(move |e| {
            set(&mut ui.prefs.borrow_mut(), opt_amount(e));
            changed();
        });
    }

    let sort_labels: Vec<&str> = FinderSort::ALL.iter().map(|s| s.label()).collect();
    let sort = w::dropdown(&sort_labels, FinderSort::ALL.iter().position(|s| *s == p.sort).unwrap_or(0) as u32);
    {
        let (ui, changed) = (ui.clone(), changed.clone());
        sort.connect_selected_notify(move |d| {
            ui.prefs.borrow_mut().sort = FinderSort::ALL.get(d.selected() as usize).copied().unwrap_or_default();
            changed();
        });
    }
    let high_sec = w::chip("High-sec route only", p.high_sec_only);
    high_sec.set_tooltip_text(Some("Only couriers that can be flown without leaving high-sec"));
    {
        let (ui, changed) = (ui.clone(), changed.clone());
        high_sec.connect_toggled(move |c| {
            ui.prefs.borrow_mut().high_sec_only = c.is_active();
            changed();
        });
    }
    let travel = w::chip("Count jumps to pickup", p.count_travel);
    travel.set_tooltip_text(Some("ISK per jump includes flying from the hauler's system to the pickup"));
    {
        let (ui, changed) = (ui.clone(), changed.clone());
        travel.connect_toggled(move |c| {
            ui.prefs.borrow_mut().count_travel = c.is_active();
            changed();
        });
    }

    let box_ = w::vbox(8);
    box_.append(&fields(vec![
        labelled("Region", &region).upcast(),
        labelled("Hauler in", &origin_box).upcast(),
        labelled("Pickup within (jumps)", &max_jumps).upcast(),
        labelled("Max volume (m³)", &max_volume).upcast(),
        labelled("Max collateral", &max_collateral).upcast(),
        labelled("Min reward", &min_reward).upcast(),
        labelled("Min ISK per jump", &min_per_jump).upcast(),
        labelled("Sort by", &sort).upcast(),
    ]));

    // hull presets fill the volume limit
    let presets = w::hbox(6);
    presets.append(&w::line("Cargo of", &["label-medium", "dim"]));
    for h in hulls {
        let b = w::button(&format!("{} · {}", h.name, short_m3(h.capacity)), Btn::Outlined);
        b.add_css_class("courier-preset");
        b.set_tooltip_text(Some(&format!("Max volume {}", m3(h.capacity))));
        let (e, cap) = (max_volume.clone(), h.capacity);
        b.connect_clicked(move |_| e.set_text(&amount_text(cap)));
        presets.append(&b);
    }
    let any = w::button("Any", Btn::Text);
    {
        let e = max_volume.clone();
        any.connect_clicked(move |_| e.set_text(""));
    }
    presets.append(&any);
    let edit = w::icon_button("tune", "Edit the cargo presets");
    {
        let ctx = ui.ctx.clone();
        edit.connect_clicked(move |_| presets_dialog(&ctx));
    }
    presets.append(&edit);
    let chips = w::hbox(8);
    chips.append(&high_sec);
    chips.append(&travel);
    let row = w::hbox(16);
    let presets_scroll = gtk::ScrolledWindow::new();
    presets_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    presets_scroll.set_child(Some(&presets));
    presets_scroll.set_hexpand(true);
    row.append(&presets_scroll);
    row.append(&chips);
    box_.append(&row);
    box_
}

fn short_m3(v: f64) -> String {
    format!("{} m³", fmt::isk_short(v))
}

/// Loads the region off the GTK thread, showing how far it has got.
fn load(ui: &Rc<Ui>, force: bool) {
    let region = ui.prefs.borrow().region;
    ui.progress.set_visible(true);
    ui.progress.set_fraction(0.02);
    ui.refresh.set_sensitive(false);
    ui.status.set_text(&format!("Loading public couriers in {}…", region_name(region)));
    WAITING.with(|w| w.borrow_mut().push(Rc::downgrade(ui)));
    if LOADING.with(|l| l.get()) == Some(region) {
        return;
    }
    LOADING.with(|l| l.set(Some(region)));
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<FinderProgress>();
    gtk::glib::spawn_future_local(async move {
        use futures::StreamExt;
        while let Some(p) = rx.next().await {
            for ui in waiting() {
                if ui.prefs.borrow().region == region {
                    ui.progress.set_fraction(p.fraction());
                    ui.status.set_text(&p.label());
                }
            }
        }
    });
    let core = ui.ctx.core().clone();
    rt::run(
        async move {
            let r = cr::load_finder(&core, region, force, move |p| {
                let _ = tx.unbounded_send(p);
            })
            .await;
            let fetched = core.public_contracts.fetched(region);
            (r, fetched)
        },
        move |(r, fetched)| {
            // another region may have started loading meanwhile
            LOADING.with(|l| if l.get() == Some(region) { l.set(None) });
            let uis = waiting();
            WAITING.with(|w| w.borrow_mut().clear());
            let error = match r {
                Ok(rows) => {
                    // the listing may have come from memory: its age is when ESI was asked
                    let at = fetched.map(|i| Utc::now() - chrono::Duration::from_std(i.elapsed()).unwrap_or_default()).unwrap_or_else(Utc::now);
                    LOADED.with(|l| *l.borrow_mut() = Some(Loaded { region, rows: Rc::new(rows), at }));
                    None
                }
                Err(e) => Some(format!("Couldn't load {}'s public contracts: {e}", region_name(region))),
            };
            for ui in uis {
                ui.progress.set_visible(false);
                ui.refresh.set_sensitive(true);
                if ui.prefs.borrow().region != region {
                    // the region changed while this one loaded
                    load(&ui, false);
                } else if let Some(e) = &error {
                    super::clear(&ui.results);
                    ui.status.set_text("");
                    ui.results.append(&w::banner("cloud_off", e, BannerKind::Error));
                } else {
                    render(&ui);
                }
            }
        },
    );
}

/// Filters, sorts and shows what's loaded.
fn render(ui: &Rc<Ui>) {
    let p = ui.prefs.borrow().clone();
    let loaded = LOADED.with(|l| l.borrow().as_ref().filter(|l| l.region == p.region).map(|l| (l.rows.clone(), l.at)));
    let Some((rows, at)) = loaded else { return };
    let origin_tree = p.origin.and_then(|o| cr::tree(o, ui.pref));
    let travel = |r: &FinderRow| -> Option<usize> { origin_tree.as_ref().and_then(|t| t.jumps(r.pickup_system()?)) };
    let mut shown: Vec<&FinderRow> = rows.iter().filter(|r| cr::passes(r, &p, ui.pref, travel(r))).collect();
    cr::sort_rows(&mut shown, &p, ui.pref, travel);
    let matching = shown.len();
    let unknown = rows.iter().filter(|r| r.route(ui.pref).is_none()).count();

    let mut status = format!(
        "{} public courier{} in {} · {} match · updated {}",
        fmt::int(rows.len() as i64),
        if rows.len() == 1 { "" } else { "s" },
        region_name(p.region),
        fmt::int(matching as i64),
        fmt::relative(at)
    );
    if matching > LIMIT {
        status.push_str(&format!(" · showing the best {LIMIT}"));
    }
    if unknown > 0 {
        status.push_str(&format!(" · {unknown} without a known route (structures nobody here can look up)"));
    }
    if p.max_pickup_jumps.is_some() && p.origin.is_none() {
        status.push_str(" · pick the hauler's system to limit jumps to the pickup");
    }
    ui.status.set_text(&status);

    let table_rows: Vec<Shown> = shown
        .into_iter()
        .take(LIMIT)
        .map(|r| Shown { travel: travel(r), per_jump: r.isk_per_jump(ui.pref, if p.count_travel { travel(r) } else { None }), row: r.clone() })
        .collect();
    super::clear(&ui.results);
    if rows.is_empty() {
        ui.results.append(&w::empty_state("local_shipping", "No public couriers", &format!("Nobody has put up a public courier contract in {} right now.", region_name(p.region))));
        return;
    }
    ui.results.append(&table(&ui.ctx, table_rows, ui.pref, p.origin.is_some()));
}

/// A row as shown, with the jumps from the hauler's system.
struct Shown {
    row: FinderRow,
    travel: Option<usize>,
    per_jump: Option<f64>,
}

fn table(ctx: &Ctx, rows: Vec<Shown>, pref: RoutePref, with_travel: bool) -> gtk::Widget {
    let route = move |s: &Shown| s.row.route(pref).copied();
    let mut t = Table::new(rows)
        .widget_sorted("Pickup", |s| place_cell(&s.row.pickup), |a, b| place_key(&a.row.pickup).cmp(&place_key(&b.row.pickup)))
        .expand()
        .widget_sorted("Delivery", |s| place_cell(&s.row.delivery), |a, b| place_key(&a.row.delivery).cmp(&place_key(&b.row.delivery)))
        .expand()
        .widget_sorted("Jumps", move |s| jumps_cell(route(s).as_ref()), move |a, b| route(a).map(|r| r.jumps).cmp(&route(b).map(|r| r.jumps)))
        .width(64)
        .widget_sorted("Min sec", move |s| min_sec_cell(route(s).as_ref()), move |a, b| {
            route(a).map(|r| r.min_security).unwrap_or(-2.0).total_cmp(&route(b).map(|r| r.min_security).unwrap_or(-2.0))
        })
        .width(70);
    if with_travel {
        t = t.num("To pickup", |s| s.travel.map(|j| j.to_string()).unwrap_or_else(|| "–".into()), |s| s.travel.map(|j| j as f64).unwrap_or(f64::MAX)).width(80);
    }
    let t = t
        .num("Volume", |s| m3(s.row.volume()), |s| s.row.volume())
        .num("Collateral", |s| fmt::isk_short(s.row.collateral()), |s| s.row.collateral())
        .num("Reward", |s| fmt::isk_short(s.row.reward()), |s| s.row.reward())
        .num("ISK/jump", |s| s.per_jump.map(fmt::isk_short).unwrap_or_default(), |s| s.per_jump.unwrap_or(0.0))
        .num(
            "ISK/m³",
            |s| if s.row.volume() > 0.0 { fmt::isk_short(s.row.reward() / s.row.volume()) } else { String::new() },
            |s| if s.row.volume() > 0.0 { s.row.reward() / s.row.volume() } else { 0.0 },
        )
        .num("Days", |s| s.row.contract.days_to_complete.unwrap_or(0).to_string(), |s| s.row.contract.days_to_complete.unwrap_or(0) as f64)
        .width(56)
        .widget_sorted("Expires", |s| countdown_cell(s.row.contract.date_expired, "Expired"), |a, b| a.row.contract.date_expired.cmp(&b.row.contract.date_expired))
        .width(110)
        .text("Issuer", |s| s.row.issuer.clone());
    let ctx = ctx.clone();
    t.on_activate(move |s| open_contract(&ctx, &as_contract(&s.row))).empty_text("Nothing matches these filters.").build()
}

/// A public courier as a contract, for the contract dialog.
fn as_contract(r: &FinderRow) -> Contract {
    let c = &r.contract;
    Contract {
        contract_id: c.contract_id,
        kind: "courier".into(),
        status: "outstanding".into(),
        title: c.title.clone(),
        for_corporation: c.for_corporation,
        availability: "public".into(),
        issuer_id: c.issuer_id,
        issuer_corporation_id: c.issuer_corporation_id,
        issuer_name: if c.for_corporation { String::new() } else { r.issuer.clone() },
        issuer_corporation_name: if c.for_corporation { r.issuer.clone() } else { String::new() },
        date_issued: c.date_issued,
        date_expired: c.date_expired,
        days_to_complete: c.days_to_complete,
        reward: c.reward,
        collateral: c.collateral,
        volume: c.volume,
        start_location: r.pickup.clone(),
        end_location: r.delivery.clone(),
        ..Default::default()
    }
}

/// Edits the cargo presets: names and capacities the user's haulers really carry.
fn presets_dialog(ctx: &Ctx) {
    let (win, content, actions) = w::dialog(ctx, "Cargo presets", 480);
    let note = w::line(
        "Set each to what your fitted, skilled hauler carries: cargo depends on fittings, rigs and skills.",
        &["body-medium", "dim"],
    );
    note.set_wrap(true);
    note.set_xalign(0.0);
    content.append(&note);
    let list = w::vbox(8);
    let rows: Rc<RefCell<Vec<(gtk::Entry, gtk::Entry, gtk::Box)>>> = Rc::default();
    let add_row = {
        let (list, rows) = (list.clone(), rows.clone());
        Rc::new(move |h: &Hull| {
            let row = w::hbox(8);
            let name = gtk::Entry::new();
            name.set_text(&h.name);
            name.set_placeholder_text(Some("Hull"));
            name.set_hexpand(true);
            let cap = number_entry(&amount_text(h.capacity), 12);
            cap.set_placeholder_text(Some("m³"));
            let del = w::icon_button("delete", "Remove");
            row.append(&name);
            row.append(&cap);
            row.append(&w::line("m³", &["dim"]));
            row.append(&del);
            {
                let (list, rows, row) = (list.clone(), rows.clone(), row.clone());
                del.connect_clicked(move |_| {
                    rows.borrow_mut().retain(|r| r.2 != row);
                    list.remove(&row);
                });
            }
            list.append(&row);
            rows.borrow_mut().push((name, cap, row));
        })
    };
    for h in &cr::load_doc().hulls {
        add_row(h);
    }
    content.append(&list);
    let tools = w::hbox(8);
    let add = w::button_icon("add", "Add preset", Btn::Text);
    {
        let add_row = add_row.clone();
        add.connect_clicked(move |_| add_row(&Hull::default()));
    }
    let reset = w::button("Restore defaults", Btn::Text);
    {
        let (add_row, rows, list) = (add_row.clone(), rows.clone(), list.clone());
        reset.connect_clicked(move |_| {
            for r in rows.borrow_mut().drain(..) {
                list.remove(&r.2);
            }
            for h in cr::default_hulls() {
                add_row(&h);
            }
        });
    }
    tools.append(&add);
    tools.append(&reset);
    content.append(&tools);

    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button("Save", Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (win, ctx) = (win.clone(), ctx.clone());
        save.connect_clicked(move |_| {
            let hulls: Vec<Hull> = rows
                .borrow()
                .iter()
                .filter_map(|(n, c, _)| {
                    let capacity = opt_amount(c)?;
                    let name = n.text().trim().to_string();
                    Some(Hull { name: if name.is_empty() { "Hauler".into() } else { name }, capacity })
                })
                .collect();
            let mut doc = cr::load_doc();
            doc.hulls = hulls;
            cr::save_doc(ctx.core(), &doc);
            win.close();
            ctx.rebuild();
        });
    }
    actions.append(&cancel);
    actions.append(&save);
    win.present();
}
