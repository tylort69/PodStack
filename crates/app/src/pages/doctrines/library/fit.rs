//! A fit's details: its name, role, doctrine and notes (editable), its EFT text, items and skills, and who of the
//! characters can fly it.

use super::dialogs::{self, buffer_text, labelled, notes_field};
use super::doctrine::{check_tooltip, ship_line};
use super::{plural, Lib, Sel};
use crate::pages::skill_browser::shared;
use crate::widgets::{self as w, BannerKind, Btn, Status};
use gtk::prelude::*;
use podstack_core::features::doctrines::{DoctrineFit, Readiness};
use podstack_core::{config, fmt, training::roman};
use std::rc::Rc;

/// Roles offered as one-click choices (the field takes any text).
const ROLES: [&str; 9] = ["DPS", "Logistics", "Tackle", "EWAR", "Command", "Support", "Scout", "Interdiction", "Capital"];

pub(super) fn dialog(lib: &Rc<Lib>, fit_id: &str) {
    let doc = lib.doc();
    let Some((_, doctrine, f)) = doc.fit(fit_id) else { return };
    let f = f.clone();
    let doctrine_id = doctrine.id.clone();
    let checks = lib.checks();

    let (win, content, actions) = w::dialog(&lib.ctx, "Fit", 680);
    win.set_default_height(680);
    let inner = w::vbox(16);
    let scroll = w::scrolled(&inner);
    scroll.set_min_content_height(420);
    content.append(&scroll);

    // the hull and who can fly it
    let hero = w::hbox(20);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("doclib-ship-large");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_valign(gtk::Align::Center);
    frame.append(&w::eve_image(&config::type_render(f.ship_id, 128), 96));
    hero.append(&frame);
    let texts = w::vbox(4);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    texts.append(&w::text(&f.name, &["headline-small"]));
    let mut sub = vec![ship_line(&f)];
    if let Some(r) = f.ship().map(podstack_core::features::doctrines::race_name) {
        sub.push(r);
    }
    texts.append(&w::text(&sub.join(" \u{00b7} "), &["body-medium", "dim"]));
    if let Some(t) = f.added {
        texts.append(&w::line(&format!("Added {}", fmt::date(t)), &["body-small", "dim"]));
    }
    if !checks.ids.is_empty() {
        let n = checks.fit_ready(&f.id);
        let pill = w::status(&format!("{n} of {} can fly it", checks.ids.len()), if n > 0 { Status::Good } else { Status::Neutral });
        pill.set_margin_top(4);
        texts.append(&pill);
    }
    hero.append(&texts);
    inner.append(&hero);
    if !f.unknown.is_empty() {
        inner.append(&w::banner(
            "help_outline",
            &format!("Not recognised when imported, so left out of the checks: {}", f.unknown.join(", ")),
            BannerKind::Notice,
        ));
    }

    // what can be edited
    let name = gtk::Entry::new();
    name.set_text(&f.name);
    name.set_hexpand(true);
    let role = gtk::Entry::new();
    role.set_text(&f.role);
    role.set_placeholder_text(Some("What it does in the fleet"));
    role.set_hexpand(true);
    let chips = w::hbox(6);
    for r in ROLES {
        let b = w::button(r, Btn::Outlined);
        b.add_css_class("chip");
        let e = role.clone();
        b.connect_clicked(move |_| e.set_text(r));
        chips.append(&b);
    }
    let chip_scroll = gtk::ScrolledWindow::new();
    chip_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    chip_scroll.set_child(Some(&chips));
    let role_box = w::vbox(8);
    role_box.append(&role);
    role_box.append(&chip_scroll);

    let doctrines: Vec<(String, String)> = doc.all_doctrines().into_iter().map(|(p, d)| (d.id.clone(), format!("{} \u{203a} {}", p.join(" \u{203a} "), d.name))).collect();
    let labels: Vec<String> = doctrines.iter().map(|d| d.1.clone()).collect();
    let current = doctrines.iter().position(|d| d.0 == doctrine_id).unwrap_or(0);
    let target = shared::dropdown(&labels, current as u32);
    target.set_hexpand(true);
    let (notes_frame, notes) = notes_field(&f.notes, "Fitting notes, alternatives, implants, what to bring…", 64);

    let form = w::vbox(12);
    form.append(&labelled("Name", &name));
    form.append(&labelled("Role", &role_box));
    form.append(&labelled("Doctrine", &target));
    form.append(&labelled("Notes", &notes_frame));
    inner.append(&form);

    // EFT, items, skills
    let stack = gtk::Stack::new();
    stack.set_vhomogeneous(false);
    stack.add_named(&eft_view(lib, &f), Some("eft"));
    stack.add_named(&items_view(&f), Some("items"));
    stack.add_named(&skills_view(&f), Some("skills"));
    stack.add_named(&pilots_view(lib, &f), Some("pilots"));
    let pages = ["eft", "items", "skills", "pilots"];
    let st = stack.clone();
    let items_label = format!("Items ({})", f.items.len().saturating_sub(1));
    let skills_label = format!("Skills ({})", f.all_skills().len());
    let tabs = w::segmented(&["EFT", &items_label, &skills_label, "Pilots"], 0, move |i| st.set_visible_child_name(pages[i]));
    tabs.set_margin_top(4);
    inner.append(&tabs);
    inner.append(&stack);

    // actions
    actions.set_halign(gtk::Align::Fill);
    let delete = w::button_icon("delete", "Delete", Btn::Danger);
    let (l, id, wc) = (lib.clone(), f.id.clone(), win.clone());
    delete.connect_clicked(move |_| {
        let wc = wc.clone();
        dialogs::delete_fit(&l, &id, move || wc.close());
    });
    actions.append(&delete);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    actions.append(&spacer);
    let cancel = w::button("Cancel", Btn::Text);
    let wc = win.clone();
    cancel.connect_clicked(move |_| wc.close());
    actions.append(&cancel);
    let save = w::button("Save", Btn::Filled);
    let s = save.clone();
    name.connect_changed(move |e| s.set_sensitive(!e.text().trim().is_empty()));
    let (l, id, wc) = (lib.clone(), f.id.clone(), win.clone());
    save.connect_clicked(move |_| {
        let new_name = name.text().trim().to_string();
        if new_name.is_empty() {
            return;
        }
        let to = doctrines.get(target.selected() as usize).map(|d| d.0.clone()).filter(|t| *t != doctrine_id);
        let (role, notes) = (role.text().trim().to_string(), buffer_text(&notes).trim().to_string());
        let moved = l.edit(|d| {
            if let Some(f) = d.fit_mut(&id) {
                if f.name != new_name {
                    f.rename(&new_name);
                }
                f.role = role.clone();
                f.notes = notes.clone();
            }
            to.as_ref().is_some_and(|t| d.move_fit(&id, t))
        });
        if let (true, Some(t)) = (moved, to.clone()) {
            l.ctx.toast("Fit moved");
            l.select(Sel::Doctrine(t));
        } else {
            l.ctx.toast("Fit saved");
        }
        wc.close();
    });
    actions.append(&save);
    win.set_title(Some(&f.name));
    win.present();
}

fn eft_view(lib: &Rc<Lib>, f: &DoctrineFit) -> gtk::Widget {
    let b = w::vbox(8);
    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(&f.eft);
    let view = gtk::TextView::with_buffer(&buffer);
    view.set_monospace(true);
    view.set_editable(false);
    view.set_wrap_mode(gtk::WrapMode::None);
    let sw = gtk::ScrolledWindow::new();
    sw.set_child(Some(&view));
    sw.set_min_content_height(160);
    sw.set_max_content_height(360);
    sw.set_propagate_natural_height(true);
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("doclib-input-frame");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.append(&sw);
    b.append(&frame);
    let copy = w::button_icon("content_copy", "Copy EFT", Btn::Tonal);
    copy.set_halign(gtk::Align::Start);
    copy.set_tooltip_text(Some("For EVE's fitting window (Import from clipboard) or Pyfa"));
    let (ctx, fit) = (lib.ctx.clone(), f.clone());
    copy.connect_clicked(move |_| super::copy_eft(&ctx, std::slice::from_ref(&fit)));
    b.append(&copy);
    b.upcast()
}

fn items_view(f: &DoctrineFit) -> gtk::Widget {
    let list = w::vbox(2);
    for i in f.items.iter().skip(1) {
        let row = w::hbox(12);
        row.set_margin_top(2);
        row.set_margin_bottom(2);
        row.append(&w::eve_image(&config::type_icon(i.type_id, 64), 32));
        let name = if i.quantity > 1 { format!("{} \u{00d7}{}", i.name, fmt::int(i.quantity as i64)) } else { i.name.clone() };
        let n = w::line(&name, &["body-medium"]);
        n.set_hexpand(true);
        row.append(&n);
        row.append(&w::line(&i.category, &["body-small", "dim"]));
        list.append(&row);
    }
    if f.items.len() <= 1 {
        list.append(&w::text("Only the hull.", &["body-medium", "dim"]));
    }
    list.upcast()
}

fn skills_view(f: &DoctrineFit) -> gtk::Widget {
    let mut skills: Vec<(String, u8)> = f.all_skills().into_iter().map(|(id, l)| (podstack_core::features::fitting::skill_name(id), l)).collect();
    skills.sort();
    let list = w::vbox(2);
    list.append(&w::text("Every skill the fit needs, at the level its items need it (prerequisites not listed).", &["body-small", "dim"]));
    for (name, level) in skills {
        let row = w::hbox(12);
        row.set_margin_top(2);
        let n = w::line(&name, &["body-medium"]);
        n.set_hexpand(true);
        row.append(&n);
        row.append(&w::skill_pips(level, false, 0));
        let l = w::line(roman(level), &["body-medium", "numeric", "dim"]);
        l.set_width_chars(3);
        l.set_xalign(1.0);
        row.append(&l);
        list.append(&row);
    }
    list.upcast()
}

fn pilots_view(lib: &Rc<Lib>, f: &DoctrineFit) -> gtk::Widget {
    let checks = lib.checks();
    let list = w::vbox(6);
    if checks.pilots.is_empty() {
        list.append(&w::text("No characters with loaded skills yet.", &["body-medium", "dim"]));
        return list.upcast();
    }
    let mut rows: Vec<_> = checks.pilots.iter().filter_map(|c| checks.check(c.id, &f.id).map(|x| (c, x))).collect();
    rows.sort_by_key(|(_, x)| (std::cmp::Reverse(x.readiness), x.time_to_fit));
    for (c, x) in rows {
        let row = w::hbox(12);
        row.append(&w::avatar(&config::character_portrait(c.id, 64), 32));
        let n = w::line(&c.name(), &["body-medium"]);
        n.set_hexpand(true);
        row.append(&n);
        let pill = super::doctrine::check_pill(x);
        pill.set_tooltip_text(Some(&check_tooltip(x)));
        row.append(&pill);
        list.append(&row);
    }
    let ready = checks.fit_ready(&f.id);
    let hull = checks.ids.iter().filter(|id| checks.check(**id, &f.id).is_some_and(|x| x.readiness == Readiness::HullOnly)).count();
    let summary = format!("{} can fly it, {} only the hull.", plural(ready, "character", "characters"), hull);
    list.prepend(&w::text(&summary, &["body-small", "dim"]));
    list.upcast()
}
