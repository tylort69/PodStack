//! A group's view (and the whole library's): its subgroups and doctrines as cards, with how many of the characters
//! can fly them.

use super::{dialogs, hero, import, menu_button, plural, section, ship_icon, Checks, Lib, MenuItem, Sel};
use crate::widgets::{self as w, BannerKind, Btn, Status};
use gtk::prelude::*;
use podstack_core::features::doctrines::{Doctrine, DoctrineDoc, DoctrineFit, Group};
use std::rc::Rc;

/// The whole library: totals and the top-level groups.
pub(super) fn library(lib: &Rc<Lib>, doc: &DoctrineDoc) {
    let checks = lib.checks();
    let new = w::button_icon("create_new_folder", "New group", Btn::Filled);
    let l = lib.clone();
    new.connect_clicked(move |_| dialogs::new_group(&l, None));
    let paste = w::button_icon("content_paste", "Paste fits", Btn::Tonal);
    let l = lib.clone();
    paste.connect_clicked(move |_| import::paste_dialog(&l, None, None));
    let open = w::button_icon("file_open", "Open a file", Btn::Tonal);
    open.set_tooltip_text(Some("Fits (EFT .txt, EVE .xml) or a PodStack doctrines file (.json) to merge"));
    let l = lib.clone();
    open.connect_clicked(move |_| import::open_file(&l, None));
    let save = w::button_icon("save_alt", "Save library", Btn::Text);
    save.set_tooltip_text(Some("Everything as a PodStack doctrines file, to share or keep a copy"));
    let l = lib.clone();
    save.connect_clicked(move |_| {
        let doc = l.doc();
        dialogs::save_json(&l, "podstack-doctrines.json", &doc.groups);
    });
    lib.detail.append(&hero(
        "library_books",
        "",
        "All doctrines",
        "Every group you fly with, its doctrines and the fits they accept. Pick a doctrine to see who can fly it and what the others are missing.",
        &[new.upcast(), paste.upcast(), open.upcast(), save.upcast()],
    ));

    let fits: Vec<&DoctrineFit> = doc.all_fits().into_iter().map(|(_, _, f)| f).collect();
    let t = checks.tally(&fits);
    let subgroups = doc.all_groups().len() - doc.groups.len();
    lib.detail.append(&w::tile_row(vec![
        w::stat_tile("Groups", &doc.groups.len().to_string(), (subgroups > 0).then(|| plural(subgroups, "subgroup", "subgroups")).as_deref()),
        w::stat_tile("Doctrines", &doc.all_doctrines().len().to_string(), None),
        w::stat_tile("Fits", &fits.len().to_string(), None),
        w::stat_tile("Pilots", &checks.count(t.ready), Some(&format!("of {} can fly a doctrine", checks.ids.len()))),
    ]));
    pilots_notice(lib, &checks);

    lib.detail.append(&section("Groups", None));
    lib.detail.append(&cards(doc.groups.iter().map(|g| group_card(lib, &checks, g)).collect()));
}

/// Banners for when the characters are still being checked, there's nobody to check, or skills are still loading.
pub(super) fn pilots_notice(lib: &Lib, checks: &Checks) {
    if !checks.ready {
        lib.detail.append(&w::banner("hourglass_empty", "Checking which of your characters can fly what\u{2026}", BannerKind::Notice));
    } else if checks.pilots.is_empty() && checks.loading == 0 {
        lib.detail.append(&w::banner("person_add", "Add a character to see who can fly what.", BannerKind::Info));
    } else if checks.loading > 0 {
        let msg = format!("Still loading the skills of {}: they're counted once they arrive.", plural(checks.loading, "character", "characters"));
        lib.detail.append(&w::banner("hourglass_empty", &msg, BannerKind::Notice));
    }
}

pub(super) fn view(lib: &Rc<Lib>, doc: &DoctrineDoc, g: &Group) {
    let checks = lib.checks();
    let path: Vec<String> = doc.all_groups().into_iter().find(|(_, x)| x.id == g.id).map(|(p, _)| p.iter().map(|s| s.to_string()).collect()).unwrap_or_default();
    let crumbs = path[..path.len().saturating_sub(1)].join(" \u{203a} ");
    let top = doc.parent_of(&g.id).is_none();

    let add = w::button_icon("add", "Add doctrine", Btn::Filled);
    let (l, id) = (lib.clone(), g.id.clone());
    add.connect_clicked(move |_| dialogs::new_doctrine(&l, &id));
    let sub = w::button_icon("create_new_folder", "Add subgroup", Btn::Tonal);
    sub.set_tooltip_text(Some("A SIG or squad inside this group, such as Capitals or Incursions"));
    let (l, id) = (lib.clone(), g.id.clone());
    sub.connect_clicked(move |_| dialogs::new_group(&l, Some(&id)));
    let more = group_menu(lib, g);
    lib.detail.append(&hero(if top { "groups" } else { "folder" }, &crumbs, &g.name, &g.notes, &[add.upcast(), sub.upcast(), more.upcast()]));

    let fits: Vec<&DoctrineFit> = g.all_doctrines().into_iter().flat_map(|d| d.fits.iter()).collect();
    let t = checks.tally(&fits);
    let subgroups = g.walk().len() - 1;
    let doctrines = g.all_doctrines().len();
    let mut tiles = vec![
        w::stat_tile("Doctrines", &doctrines.to_string(), (subgroups > 0 && doctrines > g.doctrines.len()).then_some("with its subgroups")),
        w::stat_tile("Fits", &fits.len().to_string(), None),
        w::stat_tile("Pilots", &checks.count(t.ready), Some(&format!("of {} can fly a doctrine", checks.ids.len()))),
    ];
    if subgroups > 0 {
        tiles.push(w::stat_tile("Subgroups", &subgroups.to_string(), None));
    }
    lib.detail.append(&w::tile_row(tiles));
    if !fits.is_empty() {
        pilots_notice(lib, &checks);
    }

    if g.groups.is_empty() && g.doctrines.is_empty() {
        let empty = w::empty_state(
            "add_circle_outline",
            "Nothing here yet",
            "Add a doctrine for each fleet composition this group flies, then paste the fits it accepts. Subgroups are optional: use them for SIGs with their own doctrines.",
        );
        empty.set_margin_top(24);
        let row = w::hbox(12);
        row.set_halign(gtk::Align::Center);
        row.set_margin_top(8);
        let a = w::button_icon("add", "Add doctrine", Btn::Filled);
        let (l, id) = (lib.clone(), g.id.clone());
        a.connect_clicked(move |_| dialogs::new_doctrine(&l, &id));
        row.append(&a);
        let b = w::button_icon("create_new_folder", "Add subgroup", Btn::Tonal);
        let (l, id) = (lib.clone(), g.id.clone());
        b.connect_clicked(move |_| dialogs::new_group(&l, Some(&id)));
        row.append(&b);
        empty.append(&row);
        lib.detail.append(&empty);
        return;
    }
    if !g.doctrines.is_empty() {
        lib.detail.append(&section("Doctrines", None));
        lib.detail.append(&cards(g.doctrines.iter().map(|d| doctrine_card(lib, &checks, d)).collect()));
    }
    if !g.groups.is_empty() {
        lib.detail.append(&section("Subgroups", None));
        lib.detail.append(&cards(g.groups.iter().map(|s| group_card(lib, &checks, s)).collect()));
    }
}

fn group_menu(lib: &Rc<Lib>, g: &Group) -> gtk::MenuButton {
    let (up, down) = dialogs::can_shift(lib, &g.id);
    let id = g.id.clone();
    let name = g.name.clone();
    let l = lib.clone();
    let with = move |f: fn(&Rc<Lib>, &str)| {
        let (l, id) = (l.clone(), id.clone());
        move || f(&l, &id)
    };
    let (l, id) = (lib.clone(), g.id.clone());
    let copy = move || {
        let doc = l.doc();
        let fits: Vec<DoctrineFit> = doc.group(&id).map(|g| g.all_doctrines().into_iter().flat_map(|d| d.fits.iter().cloned()).collect()).unwrap_or_default();
        super::copy_eft(&l.ctx, &fits);
    };
    let (l, id) = (lib.clone(), g.id.clone());
    let save = move || {
        let doc = l.doc();
        if let Some(g) = doc.group(&id) {
            dialogs::save_json(&l, &dialogs::file_name(&name, "json"), std::slice::from_ref(g));
        }
    };
    menu_button(
        Some("more_vert"),
        None,
        "More for this group",
        vec![
            MenuItem::new("edit", "Edit name and notes…", with(dialogs::edit_group)),
            MenuItem::new("drive_file_move", "Move to…", with(dialogs::move_group)),
            MenuItem::new("arrow_upward", "Move up", with(|l, id| dialogs::shift(l, id, true))).enabled(up),
            MenuItem::new("arrow_downward", "Move down", with(|l, id| dialogs::shift(l, id, false))).enabled(down),
            MenuItem::Separator,
            MenuItem::new("content_copy", "Copy every fit as EFT", copy),
            MenuItem::new("save_alt", "Save as a PodStack file…", save),
            MenuItem::Separator,
            MenuItem::danger("delete", "Delete group…", with(dialogs::delete_group)),
        ],
    )
}

/// Cards in a grid that wraps to one column in a narrow window.
fn cards(children: Vec<gtk::Widget>) -> gtk::FlowBox {
    let f = w::flow(children, 3);
    f.set_min_children_per_line(1);
    f
}

/// A clickable card.
fn card_button(lib: &Rc<Lib>, sel: Sel, content: &gtk::Box) -> gtk::Widget {
    let b = gtk::Button::new();
    b.add_css_class("doclib-card");
    b.set_child(Some(content));
    b.set_size_request(260, -1);
    let l = lib.clone();
    b.connect_clicked(move |_| l.select(sel.clone()));
    b.upcast()
}

fn card_head(icon: &str, title: &str, subtitle: &str) -> gtk::Box {
    let head = w::hbox(12);
    let i = w::icon_sized(icon, 20);
    i.add_css_class("doclib-card-icon");
    head.append(&i);
    let texts = w::vbox(2);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    texts.append(&w::line(title, &["title-medium"]));
    texts.append(&w::line(subtitle, &["body-medium", "dim"]));
    head.append(&texts);
    head
}

/// Up to `max` distinct hulls of some fits, as small icons.
fn hulls(fits: &[&DoctrineFit], max: usize) -> Option<gtk::Box> {
    let mut seen = Vec::new();
    for f in fits {
        if f.ship_id > 0 && !seen.contains(&f.ship_id) {
            seen.push(f.ship_id);
        }
    }
    if seen.is_empty() {
        return None;
    }
    let b = w::hbox(6);
    for id in seen.iter().take(max) {
        b.append(&ship_icon(*id, 28));
    }
    if seen.len() > max {
        b.append(&w::line(&format!("+{}", seen.len() - max), &["label-medium", "dim"]));
    }
    Some(b)
}

/// "3 can fly", "1 hull only" pills (or why there are none).
fn tally_pills(checks: &Checks, fits: &[&DoctrineFit]) -> gtk::Box {
    let b = w::hbox(6);
    if fits.is_empty() {
        b.append(&w::status("No fits yet", Status::Neutral));
        return b;
    }
    if checks.ids.is_empty() {
        return b;
    }
    let t = checks.tally(fits);
    b.append(&w::status(&format!("{} can fly", t.ready), if t.ready > 0 { Status::Good } else { Status::Neutral }));
    if t.hull_only > 0 {
        b.append(&w::status(&format!("{} hull only", t.hull_only), Status::Warn));
    }
    b
}

fn doctrine_card(lib: &Rc<Lib>, checks: &Checks, d: &Doctrine) -> gtk::Widget {
    let fits: Vec<&DoctrineFit> = d.fits.iter().collect();
    let content = w::vbox(12);
    content.append(&card_head("shield", &d.name, &plural(fits.len(), "fit", "fits")));
    if let Some(h) = hulls(&fits, 7) {
        content.append(&h);
    }
    content.append(&tally_pills(checks, &fits));
    card_button(lib, Sel::Doctrine(d.id.clone()), &content)
}

fn group_card(lib: &Rc<Lib>, checks: &Checks, g: &Group) -> gtk::Widget {
    let doctrines = g.all_doctrines();
    let fits: Vec<&DoctrineFit> = doctrines.iter().flat_map(|d| d.fits.iter()).collect();
    let subs = g.walk().len() - 1;
    let mut sub = vec![plural(doctrines.len(), "doctrine", "doctrines"), plural(fits.len(), "fit", "fits")];
    if subs > 0 {
        sub.push(plural(subs, "subgroup", "subgroups"));
    }
    let content = w::vbox(12);
    content.append(&card_head("groups", &g.name, &sub.join(" \u{00b7} ")));
    if let Some(h) = hulls(&fits, 7) {
        content.append(&h);
    }
    content.append(&tally_pills(checks, &fits));
    card_button(lib, Sel::Group(g.id.clone()), &content)
}
