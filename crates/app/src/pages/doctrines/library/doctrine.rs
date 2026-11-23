//! A doctrine's view: how many characters can fly it, its fits, and who can fly which fit (a click shows what's
//! missing and the skill plan).

use super::{dialogs, fit, hero, import, menu_button, pilot, plural, section, ship_icon, Checks, Lib, MenuItem};
use crate::widgets::{self as w, Btn, Status};
use gtk::prelude::*;
use podstack_core::features::doctrines::{self as dc, Doctrine, DoctrineFit, FitCheck, Readiness};
use podstack_core::{config, fmt, Character};
use std::rc::Rc;

pub(super) fn view(lib: &Rc<Lib>, path: &[&str], d: &Doctrine) {
    let checks = lib.checks();
    let crumbs = path.join(" \u{203a} ");

    let add = w::button_icon("add", "Add fits", Btn::Filled);
    add.set_tooltip_text(Some("Paste EFT fits (several at once), EVE's XML or DNA"));
    let (l, id) = (lib.clone(), d.id.clone());
    add.connect_clicked(move |_| import::paste_dialog(&l, Some(id.clone()), None));
    let mut actions: Vec<gtk::Widget> = vec![add.upcast()];
    if !d.fits.is_empty() {
        let copy = w::button_icon("content_copy", "Copy EFT", Btn::Tonal);
        copy.set_tooltip_text(Some("Every fit of the doctrine, for Pyfa or EVE"));
        let (l, fits) = (lib.clone(), d.fits.clone());
        copy.connect_clicked(move |_| super::copy_eft(&l.ctx, &fits));
        actions.push(copy.upcast());
    }
    actions.push(doctrine_menu(lib, d).upcast());
    lib.detail.append(&hero("shield", &crumbs, &d.name, &d.notes, &actions));

    let fits: Vec<&DoctrineFit> = d.fits.iter().collect();
    if fits.is_empty() {
        let empty = w::empty_state(
            "rocket_launch",
            "No fits yet",
            "Paste the fits this doctrine accepts: copy them from EVE's fitting window or Pyfa (several at once is fine), or open an EFT, EVE XML or PodStack file.",
        );
        empty.set_margin_top(24);
        let row = w::hbox(12);
        row.set_halign(gtk::Align::Center);
        row.set_margin_top(8);
        let paste = w::button_icon("content_paste", "Paste fits", Btn::Filled);
        let (l, id) = (lib.clone(), d.id.clone());
        paste.connect_clicked(move |_| import::paste_dialog(&l, Some(id.clone()), None));
        row.append(&paste);
        let open = w::button_icon("file_open", "Open a file", Btn::Tonal);
        let (l, id) = (lib.clone(), d.id.clone());
        open.connect_clicked(move |_| import::open_file(&l, Some(id.clone())));
        row.append(&open);
        empty.append(&row);
        lib.detail.append(&empty);
        return;
    }

    let t = checks.tally(&fits);
    let of = format!("of {}", plural(checks.ids.len(), "character", "characters"));
    lib.detail.append(&w::tile_row(vec![
        w::stat_tile("Fits", &fits.len().to_string(), None),
        w::stat_tile("Can fly", &checks.count(t.ready), Some(&of)),
        w::stat_tile("Hull only", &checks.count(t.hull_only), Some("missing some modules")),
        w::stat_tile("Can't fly", &checks.count(t.missing), Some("not the hull yet")),
    ]));
    super::group::pilots_notice(lib, &checks);

    lib.detail.append(&section("Fits", Some("Click a fit for its EFT, items and skills, and to edit its role and notes.")));
    lib.detail.append(&fits_list(lib, &checks, d));

    if !checks.pilots.is_empty() {
        lib.detail.append(&section("Who can fly what", Some("Click a character or a cell for what's missing and the skill plan to get there.")));
        lib.detail.append(&matrix(lib, &checks, d));
    }
}

fn doctrine_menu(lib: &Rc<Lib>, d: &Doctrine) -> gtk::MenuButton {
    let (up, down) = dialogs::can_shift(lib, &d.id);
    let l = lib.clone();
    let id = d.id.clone();
    let with = move |f: fn(&Rc<Lib>, &str)| {
        let (l, id) = (l.clone(), id.clone());
        move || f(&l, &id)
    };
    let (l, id) = (lib.clone(), d.id.clone());
    let paste = move || import::paste_dialog(&l, Some(id.clone()), None);
    let (l, id) = (lib.clone(), d.id.clone());
    let open = move || import::open_file(&l, Some(id.clone()));
    let (l, fits, name) = (lib.clone(), d.fits.clone(), d.name.clone());
    let save_eft = move || dialogs::save_eft(&l, &dialogs::file_name(&name, "txt"), &fits);
    menu_button(
        Some("more_vert"),
        None,
        "More for this doctrine",
        vec![
            MenuItem::new("content_paste", "Paste fits…", paste),
            MenuItem::new("file_open", "Open a file…", open),
            MenuItem::Separator,
            MenuItem::new("edit", "Edit name and notes…", with(dialogs::edit_doctrine)),
            MenuItem::new("drive_file_move", "Move to another group…", with(dialogs::move_doctrine)),
            MenuItem::new("arrow_upward", "Move up", with(|l, id| dialogs::shift(l, id, true))).enabled(up),
            MenuItem::new("arrow_downward", "Move down", with(|l, id| dialogs::shift(l, id, false))).enabled(down),
            MenuItem::Separator,
            MenuItem::new("description", "Save fits as EFT…", save_eft).enabled(!d.fits.is_empty()),
            MenuItem::new("save_alt", "Save as a PodStack file…", with(dialogs::save_doctrine_json)),
            MenuItem::Separator,
            MenuItem::danger("delete", "Delete doctrine…", with(dialogs::delete_doctrine)),
        ],
    )
}

/// A role as a small tertiary chip.
pub(crate) fn role_chip(role: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(role));
    l.add_css_class("doclib-role");
    l.set_valign(gtk::Align::Center);
    l
}

/// The ship's class and race ("Heavy Assault Cruiser · Gallente"), or its name when unknown.
pub(crate) fn ship_line(f: &DoctrineFit) -> String {
    match f.ship() {
        Some(s) => format!("{} \u{00b7} {}", f.ship_name, s.group),
        None => f.ship_name.clone(),
    }
}

fn fits_list(lib: &Rc<Lib>, checks: &Checks, d: &Doctrine) -> gtk::Widget {
    let list = w::vbox(2);
    for (i, f) in d.fits.iter().enumerate() {
        let row = w::hbox(4);
        let content = w::hbox(16);
        content.append(&ship_icon(f.ship_id, 40));
        let texts = w::vbox(2);
        texts.set_hexpand(true);
        texts.set_valign(gtk::Align::Center);
        texts.append(&w::line(&f.name, &["body-large"]));
        texts.append(&w::line(&ship_line(f), &["body-medium", "dim"]));
        content.append(&texts);
        if !f.role.trim().is_empty() {
            content.append(&role_chip(f.role.trim()));
        }
        if !f.unknown.is_empty() {
            let warn = w::icon_sized("help_outline", 20);
            warn.add_css_class("warning-text");
            warn.set_valign(gtk::Align::Center);
            warn.set_tooltip_text(Some(&format!("Not recognised when imported, left out of the checks: {}", f.unknown.join(", "))));
            content.append(&warn);
        }
        if !checks.ids.is_empty() {
            let n = checks.fit_ready(&f.id);
            content.append(&w::status(&format!("{n} can fly"), if n > 0 { Status::Good } else { Status::Neutral }));
        }
        let open = gtk::Button::new();
        open.add_css_class("doclib-fit");
        open.set_child(Some(&content));
        open.set_hexpand(true);
        open.set_tooltip_text(Some("Details"));
        let (l, id) = (lib.clone(), f.id.clone());
        open.connect_clicked(move |_| fit::dialog(&l, &id));
        row.append(&open);
        row.append(&fit_menu(lib, f, i, d.fits.len()));
        list.append(&row);
    }
    w::card(&list).upcast()
}

fn fit_menu(lib: &Rc<Lib>, f: &DoctrineFit, index: usize, count: usize) -> gtk::MenuButton {
    let (l, id) = (lib.clone(), f.id.clone());
    let open = move || fit::dialog(&l, &id);
    let (ctx, fit) = (lib.ctx.clone(), f.clone());
    let copy = move || super::copy_eft(&ctx, std::slice::from_ref(&fit));
    let (l, id) = (lib.clone(), f.id.clone());
    let up = move || dialogs::shift(&l, &id, true);
    let (l, id) = (lib.clone(), f.id.clone());
    let down = move || dialogs::shift(&l, &id, false);
    let (l, id) = (lib.clone(), f.id.clone());
    let delete = move || dialogs::delete_fit(&l, &id, || {});
    menu_button(
        Some("more_vert"),
        None,
        "More for this fit",
        vec![
            MenuItem::new("info", "Details…", open),
            MenuItem::new("content_copy", "Copy EFT", copy),
            MenuItem::new("arrow_upward", "Move up", up).enabled(index > 0),
            MenuItem::new("arrow_downward", "Move down", down).enabled(index + 1 < count),
            MenuItem::Separator,
            MenuItem::danger("delete", "Delete fit…", delete),
        ],
    )
}

/// A check as a pill: "Can fly", "Hull only" with the time to the full fit, or the time to train.
pub(crate) fn check_pill(c: &FitCheck) -> gtk::Label {
    match c.readiness {
        Readiness::Ready => w::status("Can fly", Status::Good),
        Readiness::HullOnly => w::status(&format!("Hull \u{00b7} {}", fmt::duration(c.time_to_fit, 2)), Status::Warn),
        Readiness::Missing => w::status(&fmt::duration(c.time_to_fit, 2), Status::Neutral),
    }
}

/// What a check means, for tooltips.
pub(crate) fn check_tooltip(c: &FitCheck) -> String {
    let mut lines = vec![match c.readiness {
        Readiness::Ready => "Can fly this fit as it is".to_string(),
        Readiness::HullOnly => format!("Can fly the hull; the full fit in {}", fmt::duration(c.time_to_fit, 3)),
        Readiness::Missing => format!("The hull in {}, the full fit in {}", fmt::duration(c.time_to_hull, 3), fmt::duration(c.time_to_fit, 3)),
    }];
    if !c.missing_items.is_empty() {
        let shown: Vec<&str> = c.missing_items.iter().take(6).map(String::as_str).collect();
        let more = c.missing_items.len().saturating_sub(shown.len());
        lines.push(format!("Can't use: {}{}", shown.join(", "), if more > 0 { format!(" +{more} more") } else { String::new() }));
    }
    lines.join("\n")
}

/// Characters × fits. It scrolls sideways when the doctrine has more fits than fit the window.
fn matrix(lib: &Rc<Lib>, checks: &Checks, d: &Doctrine) -> gtk::Widget {
    let fits: Vec<&DoctrineFit> = d.fits.iter().collect();
    let grid = gtk::Grid::new();
    grid.set_column_spacing(8);
    grid.set_row_spacing(4);

    let head = w::line("Character", &["doclib-head"]);
    head.set_valign(gtk::Align::End);
    head.set_margin_start(8);
    grid.attach(&head, 0, 0, 1, 1);
    for (j, f) in fits.iter().enumerate() {
        let h = w::vbox(4);
        h.set_halign(gtk::Align::Center);
        let icon = ship_icon(f.ship_id, 32);
        icon.set_halign(gtk::Align::Center);
        h.append(&icon);
        let name = w::line(&f.name, &["doclib-head"]);
        name.set_max_width_chars(14);
        name.set_xalign(0.5);
        h.append(&name);
        h.set_tooltip_text(Some(&format!("{}\n{}", f.name, ship_line(f))));
        grid.attach(&h, j as i32 + 1, 0, 1, 1);
    }

    // who can fly it first, then the closest
    let mut rows: Vec<(&Character, Option<Readiness>, chrono::Duration)> = checks
        .pilots
        .iter()
        .map(|c| {
            let best = dc::best_readiness(&checks.matrix, c.id, &fits);
            let time = dc::closest_fit(&checks.matrix, c.id, &fits).map(|(_, x)| x.time_to_fit).unwrap_or_else(chrono::Duration::zero);
            (c.as_ref(), best, time)
        })
        .collect();
    rows.sort_by_key(|(_, best, time)| (std::cmp::Reverse(*best), *time));

    for (i, (c, _, _)) in rows.iter().enumerate() {
        let r = i as i32 + 1;
        let ready = fits.iter().filter(|f| checks.check(c.id, &f.id).is_some_and(|x| x.readiness == Readiness::Ready)).count();
        let supporting = if ready > 0 {
            format!("Can fly {} of {}", ready, plural(fits.len(), "fit", "fits"))
        } else {
            match dc::closest_fit(&checks.matrix, c.id, &fits) {
                Some((f, x)) => format!("{} in {}", f.name, fmt::duration(x.time_to_fit, 2)),
                None => String::new(),
            }
        };
        let who = w::hbox(10);
        who.append(&w::avatar(&config::character_portrait(c.id, 64), 32));
        let texts = w::vbox(0);
        texts.set_valign(gtk::Align::Center);
        let name = w::line(&c.name(), &["body-medium"]);
        name.set_max_width_chars(20);
        texts.append(&name);
        let sub = w::line(&supporting, &["body-small", "dim"]);
        sub.set_max_width_chars(22);
        texts.append(&sub);
        who.append(&texts);
        let b = gtk::Button::new();
        b.add_css_class("doclib-pilot");
        b.set_child(Some(&who));
        b.set_tooltip_text(Some("What's missing, and the skill plan"));
        let (l, cid, did) = (lib.clone(), c.id, d.id.clone());
        b.connect_clicked(move |_| pilot::dialog(&l, cid, &did, None));
        grid.attach(&b, 0, r, 1, 1);

        for (j, f) in fits.iter().enumerate() {
            let Some(x) = checks.check(c.id, &f.id) else { continue };
            let cell = gtk::Button::new();
            cell.add_css_class("doclib-cell");
            let pill = check_pill(x);
            pill.set_halign(gtk::Align::Center);
            cell.set_child(Some(&pill));
            cell.set_tooltip_text(Some(&check_tooltip(x)));
            cell.set_valign(gtk::Align::Center);
            let (l, cid, did, fid) = (lib.clone(), c.id, d.id.clone(), f.id.clone());
            cell.connect_clicked(move |_| pilot::dialog(&l, cid, &did, Some(&fid)));
            grid.attach(&cell, j as i32 + 1, r, 1, 1);
        }
        grid.attach(&plan_menu(lib, c, d), fits.len() as i32 + 1, r, 1, 1);
    }

    let sw = gtk::ScrolledWindow::new();
    sw.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    sw.set_child(Some(&grid));
    w::card(&sw).upcast()
}

/// A character's skill plan actions for a doctrine.
fn plan_menu(lib: &Rc<Lib>, c: &Character, d: &Doctrine) -> gtk::MenuButton {
    let checks = lib.checks();
    let fits: Vec<&DoctrineFit> = d.fits.iter().collect();
    let closest = dc::closest_fit(&checks.matrix, c.id, &fits).map(|(f, x)| (f.clone(), x.readiness));
    let all_ready = fits.iter().all(|f| checks.check(c.id, &f.id).is_some_and(|x| x.readiness == Readiness::Ready));
    let mut items = vec![];
    match &closest {
        Some((f, r)) if *r != Readiness::Ready => {
            let (l, cid, fit) = (lib.clone(), c.id, f.clone());
            items.push(MenuItem::new("content_copy", &format!("Copy skill plan for {}", f.name), move || pilot::copy_plan(&l, cid, &[&fit])));
            let (l, cid, fit, dn) = (lib.clone(), c.id, f.clone(), d.name.clone());
            items.push(MenuItem::new("playlist_add", &format!("Add {} to a new skill plan", f.name), move || {
                pilot::add_plan(&l, cid, &format!("{dn} \u{2014} {}", fit.name), &[&fit])
            }));
        }
        _ => {}
    }
    if !all_ready {
        let (l, cid, all) = (lib.clone(), c.id, d.fits.clone());
        items.push(MenuItem::new("content_copy", "Copy skill plan for every fit", move || pilot::copy_plan(&l, cid, &all.iter().collect::<Vec<_>>())));
        let (l, cid, all, dn) = (lib.clone(), c.id, d.fits.clone(), d.name.clone());
        items.push(MenuItem::new("playlist_add", "Add every fit to a new skill plan", move || pilot::add_plan(&l, cid, &dn, &all.iter().collect::<Vec<_>>())));
        items.push(MenuItem::Separator);
    }
    let (l, cid, did) = (lib.clone(), c.id, d.id.clone());
    items.push(MenuItem::new("checklist", "What's missing…", move || pilot::dialog(&l, cid, &did, None)));
    let mb = menu_button(Some("school"), None, "Skill plan", items);
    if all_ready {
        mb.set_tooltip_text(Some("Can fly every fit"));
    }
    mb
}
