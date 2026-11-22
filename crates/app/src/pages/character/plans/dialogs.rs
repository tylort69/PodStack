//! The plan tab's dialogs: plan names, notes, pasted skill lists, merging plans, remaps, and optimising.

use super::{dur, plural, P};
use crate::ctx::Ctx;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::plan_tools::{
    io as pio, order, profile, remap as rh,
    sim::{remap_label, ATTRIBUTE_ORDER},
    Item,
};
use podstack_core::sde::Attribute;
use podstack_core::training::AttributeSet;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Asks for a name (new plan, rename); `on_ok` gets the trimmed, non-empty name.
pub(super) fn name(ctx: &Ctx, title: &str, initial: &str, ok_label: &str, on_ok: impl Fn(String) + 'static) {
    let (win, content, actions) = w::dialog(ctx, title, 400);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("Plan name"));
    entry.set_text(initial);
    content.append(&entry);
    let cancel = w::button("Cancel", Btn::Text);
    let ok = w::button(ok_label, Btn::Filled);
    ok.set_sensitive(!initial.trim().is_empty());
    {
        let ok = ok.clone();
        entry.connect_changed(move |e| ok.set_sensitive(!e.text().trim().is_empty()));
    }
    let on_ok = Rc::new(on_ok);
    let submit = {
        let (win, entry, on_ok) = (win.clone(), entry.clone(), on_ok.clone());
        Rc::new(move || {
            let t = entry.text().trim().to_string();
            if !t.is_empty() {
                win.close();
                on_ok(t);
            }
        })
    };
    {
        let s = submit.clone();
        entry.connect_activate(move |_| s());
    }
    ok.connect_clicked(move |_| submit());
    let w2 = win.clone();
    cancel.connect_clicked(move |_| w2.close());
    actions.append(&cancel);
    actions.append(&ok);
    win.present();
    entry.grab_focus();
}

/// A message with an OK button, and optionally label/value rows (e.g. attributes to remap to).
pub(super) fn alert(ctx: &Ctx, title: &str, body: &str, rows: &[(String, String)], then: Option<Box<dyn Fn()>>) {
    let (win, content, actions) = w::dialog(ctx, title, 480);
    content.append(&w::text(body, &["body-medium", "dim"]));
    if !rows.is_empty() {
        let refs: Vec<(&str, String)> = rows.iter().map(|(k, v)| (k.as_str(), v.clone())).collect();
        let kv = w::key_values(&refs);
        kv.set_margin_top(4);
        content.append(&kv);
    }
    let ok = w::button("OK", Btn::Filled);
    let then = Rc::new(then);
    let w2 = win.clone();
    ok.connect_clicked(move |_| {
        w2.close();
        if let Some(f) = then.as_ref() {
            f();
        }
    });
    actions.append(&ok);
    win.present();
}

/// The note editor: title and details. `index` edits that note; None adds one at the end.
pub(super) fn note(p: &P, index: Option<usize>) {
    let (title0, details0) = match index.and_then(|i| p.st.borrow().planner.queue.get(i).cloned()) {
        Some(Item::Note { title, details, .. }) => (title, details),
        _ => (String::new(), String::new()),
    };
    let (win, content, actions) = w::dialog(&p.ctx, if index.is_some() { "Edit note" } else { "Note" }, 460);
    content.append(&w::text("Title", &["label-large", "dim"]));
    let entry = gtk::Entry::new();
    entry.set_text(&title0);
    content.append(&entry);
    content.append(&w::text("Details", &["label-large", "dim"]));
    let tv = gtk::TextView::new();
    tv.set_wrap_mode(gtk::WrapMode::WordChar);
    tv.buffer().set_text(&details0);
    let sw = gtk::ScrolledWindow::new();
    sw.set_child(Some(&tv));
    sw.set_min_content_height(160);
    content.append(&sw);
    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button("Save", Btn::Filled);
    save.set_sensitive(!title0.trim().is_empty());
    {
        let save = save.clone();
        entry.connect_changed(move |e| save.set_sensitive(!e.text().trim().is_empty()));
    }
    {
        let (win, pp) = (win.clone(), p.clone());
        let entry = entry.clone();
        save.connect_clicked(move |_| {
            let title = entry.text().trim().to_string();
            let b = tv.buffer();
            let details = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
            win.close();
            pp.edit(|pl| match index {
                Some(i) => pl.edit_note(i, &title, &details),
                None => pl.add_note(&title, &details),
            });
        });
    }
    let w2 = win.clone();
    cancel.connect_clicked(move |_| w2.close());
    actions.append(&cancel);
    actions.append(&save);
    win.present();
    entry.grab_focus();
}

/// Paste a skill list (one "Skill Name IV" per line), opened with the clipboard's text. `from_eve` explains the
/// EVE client's skill plan export instead.
pub(super) fn paste(p: &P, from_eve: bool) {
    let (win, content, actions) = w::dialog(&p.ctx, if from_eve { "Import EVE skill plan" } else { "Paste skill list" }, 600);
    let help = if from_eve {
        "In EVE, open the Skills window (Alt+X), go to Skill Plans and select the plan, then use the ☰ menu at its \
         top right and choose to copy it to the clipboard. Paste it below if it isn't shown already. Skills you've \
         already trained are skipped and missing prerequisites are added."
    } else {
        "One skill per line, as copied from the EVE client, EVEMon or a forum post, e.g. \"Caldari Cruiser IV\" or \
         \"Caldari Cruiser 4\". Missing prerequisites are added too."
    };
    content.append(&w::text(help, &["body-medium", "dim"]));
    let tv = gtk::TextView::new();
    tv.set_monospace(true);
    tv.add_css_class("paste-text");
    let sw = gtk::ScrolledWindow::new();
    sw.set_child(Some(&tv));
    sw.set_min_content_height(240);
    content.append(&sw);
    let summary = w::hbox(16);
    let ok_label = w::line("0 recognised", &["body-medium"]);
    let bad_label = w::line("", &["body-medium", "warning-text"]);
    bad_label.set_hexpand(true);
    summary.append(&ok_label);
    summary.append(&bad_label);
    content.append(&summary);

    let cancel = w::button("Cancel", Btn::Text);
    let add = w::button("Add 0 skills", Btn::Filled);
    add.set_sensitive(false);
    let parsed = Rc::new(RefCell::new(pio::Parsed::default()));
    {
        let (parsed, add, ok_label, bad_label) = (parsed.clone(), add.clone(), ok_label.clone(), bad_label.clone());
        tv.buffer().connect_changed(move |b| {
            let text = b.text(&b.start_iter(), &b.end_iter(), false);
            let r = pio::parse_skill_text(&text);
            let n = r.skills.len();
            add.set_label(&format!("Add {n} skill{}", plural(n)));
            add.set_sensitive(n > 0);
            ok_label.set_text(&format!("{n} recognised"));
            if n > 0 {
                ok_label.add_css_class("success-text");
            } else {
                ok_label.remove_css_class("success-text");
            }
            if r.unknown.is_empty() {
                bad_label.set_text("");
            } else {
                let shown: Vec<&str> = r.unknown.iter().take(3).map(|s| s.as_str()).collect();
                bad_label.set_text(&format!("{} not recognised: {}{}", r.unknown.len(), shown.join(", "), if r.unknown.len() > 3 { "…" } else { "" }));
                bad_label.set_tooltip_text(Some(&r.unknown.join("\n")));
            }
            *parsed.borrow_mut() = r;
        });
    }
    // start with the clipboard's text
    {
        let buf = tv.buffer();
        p.ctx.window().clipboard().read_text_async(None::<&gtk::gio::Cancellable>, move |r| {
            if let Ok(Some(t)) = r {
                if buf.char_count() == 0 {
                    buf.set_text(&t);
                }
            }
        });
    }
    {
        let (win, pp) = (win.clone(), p.clone());
        add.connect_clicked(move |_| {
            let skills = parsed.borrow().skills.clone();
            win.close();
            if from_eve {
                pp.import_skills("EVE skill plan", "the EVE client", &skills);
            } else {
                pp.import_skills("Pasted skill list", "pasted text", &skills);
            }
        });
    }
    let w2 = win.clone();
    cancel.connect_clicked(move |_| w2.close());
    actions.append(&cancel);
    actions.append(&add);
    win.present();
    tv.grab_focus();
}

/// Picks two or more plans and a name for the plan they're merged into, in priority order.
pub(super) fn merge(p: &P) {
    let plans = p.plans();
    let (win, content, actions) = w::dialog(&p.ctx, "Merge plans", 460);
    content.append(&w::text("Creates a new plan from the selected plans, in priority order. The originals are kept.", &["body-medium", "dim"]));
    let checks: Vec<(String, gtk::CheckButton)> = plans.iter().map(|pl| (pl.id.clone(), gtk::CheckButton::with_label(&pl.name))).collect();
    let bx = w::vbox(4);
    for (_, c) in &checks {
        bx.append(c);
    }
    let sw = gtk::ScrolledWindow::new();
    sw.set_child(Some(&bx));
    sw.set_propagate_natural_height(true);
    sw.set_max_content_height(320);
    content.append(&sw);
    let entry = gtk::Entry::new();
    entry.set_placeholder_text(Some("New plan name"));
    content.append(&entry);
    let cancel = w::button("Cancel", Btn::Text);
    let go = w::button("Merge", Btn::Filled);
    go.set_sensitive(false);
    let checks = Rc::new(checks);
    let update = {
        let (checks, entry, go) = (checks.clone(), entry.clone(), go.clone());
        Rc::new(move || {
            let n = checks.iter().filter(|(_, c)| c.is_active()).count();
            go.set_sensitive(n >= 2 && !entry.text().trim().is_empty());
        })
    };
    for (_, c) in checks.iter() {
        let u = update.clone();
        c.connect_toggled(move |_| u());
    }
    {
        let u = update.clone();
        entry.connect_changed(move |_| u());
    }
    {
        let (win, pp, entry) = (win.clone(), p.clone(), entry.clone());
        go.connect_clicked(move |_| {
            // keep priority order regardless of the order they were ticked in
            let ids: Vec<String> = checks.iter().filter(|(_, c)| c.is_active()).map(|(id, _)| id.clone()).collect();
            let chosen: Vec<_> = plans.iter().filter(|pl| ids.contains(&pl.id)).cloned().collect();
            let name = entry.text().trim().to_string();
            win.close();
            let merged = podstack_core::features::plan_tools::sim::merge(&pp.base(), &chosen);
            pp.create_plan(&name, &merged.queue);
        });
    }
    let w2 = win.clone();
    cancel.connect_clicked(move |_| w2.close());
    actions.append(&cancel);
    actions.append(&go);
    win.present();
}

/// A question with several answers; `on_choice` gets the index of the button picked.
fn choose(ctx: &Ctx, title: &str, body: &str, cancel_label: &str, choices: &[(&str, bool)], on_choice: impl Fn(usize) + 'static) {
    let (win, content, actions) = w::dialog(ctx, title, 520);
    content.append(&w::text(body, &["body-medium", "dim"]));
    let cancel = w::button(cancel_label, Btn::Text);
    let w2 = win.clone();
    cancel.connect_clicked(move |_| w2.close());
    actions.append(&cancel);
    let on_choice = Rc::new(on_choice);
    for (i, (label, primary)) in choices.iter().enumerate() {
        let b = w::button(label, if *primary { Btn::Filled } else { Btn::Tonal });
        let (w2, f) = (win.clone(), on_choice.clone());
        b.connect_clicked(move |_| {
            w2.close();
            f(i);
        });
        actions.append(&b);
    }
    win.present();
}

/// Asks how to optimise the open plan (the Optimise button, and after an import): reorder it with the current
/// attributes, or find the best remap for it and reorder around that.
pub(super) fn prompt_optimise(p: &P, after_import: bool) {
    let (pid, skills) = {
        let st = p.st.borrow();
        (st.open.clone(), st.planner.skill_count())
    };
    if pid.is_none() || skills < 2 {
        return;
    }
    let pp = p.clone();
    choose(
        &p.ctx,
        if after_import { "Optimise the imported plan?" } else { "Optimise plan" },
        "Reorder the plan to finish it as soon as possible, keeping every skill after its prerequisites.\n\n\
         With current attributes: the plan keeps the attributes it has. With a remap: PodStack works out the remap \
         that trains this plan fastest, adds it at the start, and tells you which attributes to pick.",
        if after_import { "Not now" } else { "Cancel" },
        &[("Current attributes", false), ("With a remap", true)],
        move |choice| {
            // the plan may have been switched while the question was open
            if pp.st.borrow().open != pid {
                return;
            }
            if choice == 1 {
                optimise_with_remap(&pp);
            } else {
                optimise_current(&pp);
            }
        },
    );
}

fn optimise_current(p: &P) {
    let result = order::optimise_order(&p.st.borrow().planner);
    let Some(r) = result else {
        alert(
            &p.ctx,
            "Already as fast as it gets",
            "No order of this plan trains it any sooner.\n\nWith the same attributes throughout, every order takes the \
             same total time: each level takes its skill points over its training speed, wherever it is. Order only \
             saves time around remaps in the plan (each skill trained after the remap that suits it best) and while a \
             cerebral accelerator is active (slowest skills first while it lasts). Optimise with a remap to change the \
             attributes too.",
            &[],
            None,
        );
        return;
    };
    let accel = p.st.borrow().planner.accelerator().is_some();
    p.edit(|pl| pl.rebuild(&r.queue));
    let mut body = format!("The plan now takes {} instead of {}, {} sooner.\n\n", dur(r.time), dur(r.before), dur(r.before - r.time));
    if r.moved > 0 {
        body.push_str(&format!("{} skill level{} moved to another remap section. ", r.moved, plural(r.moved)));
    }
    if accel {
        body.push_str("The slowest skills train first while the accelerator lasts. ");
    }
    body.push_str("Every skill still comes after its prerequisites.");
    alert(&p.ctx, "Plan optimised", &body, &[], None);
}

/// The fastest remap for the plan, at its start (replacing a remap already there), then the order optimised
/// around it; a message says which attributes to pick and whether a remap is available.
fn optimise_with_remap(p: &P) {
    let Some(c) = p.character() else { return };
    let (before, mut queue, omega) = {
        let st = p.st.borrow();
        (st.planner.time, st.planner.queue.clone(), st.planner.omega())
    };
    let first = queue.iter().position(|i| !matches!(i, Item::Note { .. }));
    let replacing = first.map(|f| matches!(queue[f], Item::Remap { .. })).unwrap_or(false);

    // the implant set every attribute has (as the remap dialog assumes), and the attributes now
    let (implants, _) = profile::uniform_implants(&c);
    let current = profile::base_attributes(&c);
    let skills = rh::section_after(&queue, if replacing { first } else { None });
    let best = rh::optimise(&skills, implants, omega, Some(&current));

    if rh::changes(&current, &best.attributes).is_empty() && !replacing {
        let pp = p.clone();
        alert(
            &p.ctx,
            "No remap needed",
            "Your current attributes are already the best remap for this plan, so a remap wouldn't save any time. \
             The plan is optimised with them instead.",
            &[],
            Some(Box::new(move || optimise_current(&pp))),
        );
        return;
    }

    let remap = Item::remap(best.attributes, implants);
    match first {
        Some(f) if replacing => queue[f] = remap,
        Some(f) => queue.insert(f, remap),
        None => queue.push(remap),
    }
    p.edit(|pl| {
        pl.rebuild(&queue);
        // and the best order around it (and any later remaps or an accelerator)
        if let Some(r) = order::optimise_order(pl) {
            pl.rebuild(&r.queue);
        }
    });

    let st = p.st.borrow();
    let after = st.planner.time;
    // skills the reorder kept ahead of the remap, as they train faster with the current attributes
    let remap_at = st.planner.queue.iter().position(|i| matches!(i, Item::Remap { .. })).unwrap_or(0);
    let skills_first = st.planner.queue[..remap_at].iter().filter(|i| i.is_skill()).count();
    drop(st);
    let a = &best.attributes;
    let mut body = String::from("Remap to these attributes in EVE (Character Sheet → Attributes → Remap):\n\n");
    if implants > 0 {
        body.push_str(&format!("With your +{implants} implants they come to {}. ", remap_label(&a.plus_all(implants as f64)).replace(' ', " / ")));
    }
    body.push_str(&format!("The plan now takes {} instead of {}", dur(after), dur(before)));
    if before > after {
        body.push_str(&format!(", {} sooner", dur(before - after)));
    }
    body.push_str(". ");
    if skills_first > 0 {
        body.push_str(&format!(
            "Train the first {skills_first} skill level{} of the plan before remapping: they train faster with your current \
             attributes. The remap is in the plan after them.",
            plural(skills_first)
        ));
    } else if replacing {
        body.push_str("The remap at the start of the plan was changed to this one.");
    } else {
        body.push_str("A remap was added at the start of the plan.");
    }
    if let Some(av) = availability(&c) {
        body.push_str(&format!("\n\n{av}"));
    }
    let rows: Vec<(String, String)> = ATTRIBUTE_ORDER.iter().map(|x| (x.name().to_string(), format!("{}", a.get(*x)))).collect();
    alert(&p.ctx, "Remap for this plan", &body, &rows, None);
}

/// Whether a remap can be used now, for messages.
fn availability(c: &podstack_core::Character) -> Option<String> {
    let attrs = c.attributes.as_ref()?;
    let bonus = attrs.bonus_remaps.unwrap_or(0);
    Some(match c.next_remap_date() {
        None => "Your yearly remap is available now.".into(),
        Some(_) if bonus > 0 => format!("You have {bonus} bonus remap{} to use now.", if bonus == 1 { "" } else { "s" }),
        Some(d) => format!("No remap is available yet: the yearly remap comes back on {}.", podstack_core::fmt::date(d)),
    })
}

fn signed(n: i32) -> String {
    match n {
        n if n > 0 => format!("+{n}"),
        n if n < 0 => format!("−{}", -n),
        _ => "0".into(),
    }
}

/// The remap editor. Compares the character's current attributes with the fastest remap for the skills this
/// remap affects, says what to change, and lets the split be adjusted before saving. `index` edits that remap;
/// None adds one at the start of the plan.
pub(super) fn remap(p: &P, index: Option<usize>) {
    let Some(c) = p.character() else { return };
    let (queue, omega, editing) = {
        let st = p.st.borrow();
        let editing = index.and_then(|i| match st.planner.queue.get(i) {
            Some(Item::Remap { attributes, implants, .. }) => Some((*attributes, *implants)),
            _ => None,
        });
        (st.planner.queue.clone(), st.planner.omega(), editing)
    };
    let skills = Rc::new(rh::section_after(&queue, index));
    let current = profile::base_attributes(&c);
    let (uniform, mixed) = profile::uniform_implants(&c);
    let implants0 = editing.map(|e| e.1).unwrap_or(uniform);
    // editing keeps the saved split; a new remap starts on the optimum
    let attrs0 = editing.map(|e| e.0).unwrap_or_else(|| rh::optimise(&skills, implants0, omega, Some(&current)).attributes);

    let (win, content, actions) = w::dialog(&p.ctx, if index.is_some() { "Edit remap" } else { "Remap" }, 760);
    win.set_default_height(-1);

    if let Some(av) = c.attributes.as_ref() {
        let bonus = av.bonus_remaps.unwrap_or(0);
        let yearly = match c.next_remap_date() {
            None => "yearly remap available".to_string(),
            Some(d) => format!("next yearly remap in {}", podstack_core::fmt::time_until(d)),
        };
        let ready = c.next_remap_date().is_none() || bonus > 0;
        content.append(&w::text(
            &format!("{bonus} bonus remap{} · {yearly}", if bonus == 1 { "" } else { "s" }),
            &["body-medium", if ready { "success-text" } else { "warning-text" }],
        ));
    }
    let advice = w::vbox(4);
    content.append(&advice);

    let grid = gtk::Grid::new();
    grid.add_css_class("remap-grid");
    grid.set_column_spacing(20);
    grid.set_row_spacing(6);
    for (col, h) in ["Attribute", "Current", "Optimal", "Change", "Remap to", ""].iter().enumerate() {
        let l = w::line(h, &["head"]);
        if (1..=3).contains(&col) {
            l.set_xalign(1.0);
        }
        grid.attach(&l, col as i32, 0, 1, 1);
    }
    let attrs = Rc::new(Cell::new(attrs0));
    let implants = Rc::new(Cell::new(implants0));
    let busy = Rc::new(Cell::new(false));
    let mut opt_labels = vec![];
    let mut chg_labels = vec![];
    let mut val_labels = vec![];
    let mut scales = vec![];
    for (r, a) in ATTRIBUTE_ORDER.iter().enumerate() {
        let row = r as i32 + 1;
        grid.attach(&w::line(a.name(), &["body-medium"]), 0, row, 1, 1);
        let cur = w::line(&format!("{}", current.get(*a)), &["numeric"]);
        cur.set_xalign(1.0);
        grid.attach(&cur, 1, row, 1, 1);
        let opt = w::line("", &["numeric", "primary-text"]);
        opt.set_xalign(1.0);
        grid.attach(&opt, 2, row, 1, 1);
        let chg = w::line("", &["numeric"]);
        chg.set_xalign(1.0);
        grid.attach(&chg, 3, row, 1, 1);
        let sc = gtk::Scale::with_range(gtk::Orientation::Horizontal, rh::MIN_ATTRIBUTE as f64, rh::MAX_ATTRIBUTE as f64, 1.0);
        sc.set_draw_value(false);
        sc.set_round_digits(0);
        sc.set_hexpand(true);
        sc.set_value(attrs0.get(*a));
        grid.attach(&sc, 4, row, 1, 1);
        let val = w::line("", &["numeric", "title-small"]);
        val.set_xalign(1.0);
        val.set_width_chars(3);
        grid.attach(&val, 5, row, 1, 1);
        opt_labels.push(opt);
        chg_labels.push(chg);
        val_labels.push(val);
        scales.push(sc);
    }
    let imp_row = ATTRIBUTE_ORDER.len() as i32 + 1;
    grid.attach(&w::line("Implants", &["body-medium"]), 0, imp_row, 1, 1);
    let imp_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 5.0, 1.0);
    imp_scale.set_draw_value(false);
    imp_scale.set_round_digits(0);
    imp_scale.set_value(implants0 as f64);
    grid.attach(&imp_scale, 4, imp_row, 1, 1);
    let imp_val = w::line("", &["numeric", "title-small"]);
    imp_val.set_xalign(1.0);
    grid.attach(&imp_val, 5, imp_row, 1, 1);
    content.append(&grid);

    let unspent_label = w::text("", &["body-medium", "warning-text"]);
    content.append(&unspent_label);
    let second_box = w::vbox(8);
    content.append(&second_box);
    let totals = w::hbox(12);
    content.append(&totals);
    if mixed {
        content.append(&w::text(&format!("Your current implants aren't the same for every attribute; times here assume +{implants0} on all five."), &["body-small", "dim"]));
    }

    let use_opt = w::button("Use optimal", Btn::Text);
    let use_cur = w::button("Use current", Btn::Text);
    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button(if index.is_some() { "Save" } else { "Add remap" }, Btn::Filled);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    actions.set_halign(gtk::Align::Fill);
    actions.append(&use_opt);
    actions.append(&use_cur);
    actions.append(&spacer);
    actions.append(&cancel);
    actions.append(&save);

    // optimum and second-remap search, redone only when the implants change
    type Cached = (i32, rh::Remap, Option<rh::SecondRemap>);
    let cache: Rc<RefCell<Option<Cached>>> = Rc::new(RefCell::new(None));
    let scales = Rc::new(scales);
    let refresh: Rc<dyn Fn()> = {
        let (attrs, implants, skills, cache, scales) = (attrs.clone(), implants.clone(), skills.clone(), cache.clone(), scales.clone());
        let (advice, unspent_label, second_box, totals, save, imp_val) =
            (advice.clone(), unspent_label.clone(), second_box.clone(), totals.clone(), save.clone(), imp_val.clone());
        let (pp, win2) = (p.clone(), win.clone());
        let busy = busy.clone();
        Rc::new(move || {
            let imp = implants.get();
            let a = attrs.get();
            if cache.borrow().as_ref().map(|c| c.0 != imp).unwrap_or(true) {
                let best = rh::optimise(&skills, imp, omega, Some(&current));
                let second = rh::optimise_second(&skills, imp, omega, Some(&current));
                *cache.borrow_mut() = Some((imp, best, second));
            }
            let (_, best, second) = cache.borrow().clone().expect("cached");
            busy.set(true);
            for (k, x) in ATTRIBUTE_ORDER.iter().enumerate() {
                opt_labels[k].set_text(&format!("{}", best.attributes.get(*x)));
                let d = (best.attributes.get(*x) - current.get(*x)) as i32;
                chg_labels[k].set_text(&signed(d));
                for cls in ["success-text", "warning-text", "dim"] {
                    chg_labels[k].remove_css_class(cls);
                }
                chg_labels[k].add_css_class(if d > 0 { "success-text" } else if d < 0 { "warning-text" } else { "dim" });
                val_labels[k].set_text(&format!("{}", a.get(*x)));
                if (scales[k].value() - a.get(*x)).abs() > 0.1 {
                    scales[k].set_value(a.get(*x));
                }
            }
            busy.set(false);
            imp_val.set_text(&format!("+{imp}"));

            // advice
            while let Some(ch) = advice.first_child() {
                advice.remove(&ch);
            }
            if skills.is_empty() {
                advice.append(&w::text(
                    &format!("There are no skills {} to optimise for.", if index.is_some() { "after this remap" } else { "in this plan" }),
                    &["body-medium", "dim"],
                ));
            } else {
                let ch = rh::changes(&current, &best.attributes);
                let cur_time = rh::training_time(&skills, &current, imp, omega);
                let saving = cur_time - best.time;
                if ch.is_empty() {
                    advice.append(&w::text("Your current attributes are already optimal for this plan.", &["body-medium", "success-text"]));
                } else {
                    let list: Vec<String> = ch.iter().map(|(x, d)| format!("{} {}", signed(*d), x.name())).collect();
                    let mut m = format!("<b>To train fastest:</b> {}", list.join(", "));
                    if saving > 60_000.0 {
                        m.push_str(&format!(" · saves {}", dur(saving)));
                    }
                    advice.append(&w::markup(&m, &["body-medium"]));
                }
                let n = skills.len();
                advice.append(&w::text(
                    &format!(
                        "Optimised for the {n} skill level{} {}.",
                        plural(n),
                        if index.is_some() { "between this remap and the next one" } else { "in this plan up to its first remap" }
                    ),
                    &["body-small", "dim"],
                ));
            }

            let spent: f64 = Attribute::ALL.iter().map(|x| a.get(*x)).sum();
            let unspent = rh::TOTAL_POINTS - spent as i32;
            unspent_label.set_visible(unspent != 0);
            unspent_label.set_text(&if unspent > 0 { format!("{unspent} point{} left to assign.", if unspent == 1 { "" } else { "s" }) } else { format!("{} points too many.", -unspent) });
            save.set_sensitive(unspent == 0);

            // a second remap for plans over a year
            while let Some(ch) = second_box.first_child() {
                second_box.remove(&ch);
            }
            if let Some(sr) = second.as_ref().filter(|_| !skills.is_empty()) {
                let after = &skills[sr.split_after];
                let b = w::vbox(6);
                b.add_css_class("remap-second");
                let head = w::hbox(8);
                head.append(&w::icon_sized("event_repeat", 20));
                head.append(&w::line("This takes over a year: remap a second time", &["title-small"]));
                b.append(&head);
                b.append(&w::markup(
                    &format!(
                        "Your yearly remap is available again after a year. Remap now to <b>{}</b>, then after <b>{}</b> (about {} in) remap to <b>{}</b>.",
                        remap_label(&sr.first.attributes),
                        podstack_core::fmt::escape(&after.title()),
                        podstack_core::fmt::duration(chrono::Duration::milliseconds(sr.first_duration as i64), 2),
                        remap_label(&sr.second.attributes)
                    ),
                    &["body-medium"],
                ));
                let foot = w::hbox(8);
                let t = w::markup(&format!("Total <b>{}</b> · saves {} over one remap", dur(sr.time), dur(sr.saving)), &["body-medium"]);
                t.set_hexpand(true);
                foot.append(&t);
                let both = w::button("Use both remaps", Btn::Filled);
                {
                    let (pp, win2, sr) = (pp.clone(), win2.clone(), sr.clone());
                    let after_id = (after.id, after.level);
                    both.connect_clicked(move |_| {
                        win2.close();
                        add_two_remaps(&pp, index, sr.first.attributes, imp, after_id, sr.second.attributes);
                    });
                }
                foot.append(&both);
                b.append(&foot);
                second_box.append(&b);
            }

            // totals
            while let Some(ch) = totals.first_child() {
                totals.remove(&ch);
            }
            if !skills.is_empty() {
                totals.append(&w::stat_tile("Current attributes", &dur(rh::training_time(&skills, &current, imp, omega)), None));
                let o = w::stat_tile("Optimal", &dur(best.time), None);
                o.add_css_class("accent");
                totals.append(&o);
                totals.append(&w::stat_tile("This remap", &dur(rh::training_time(&skills, &a, imp, omega)), None));
            }
        })
    };

    for (k, sc) in scales.iter().enumerate() {
        let (attrs, refresh, busy, attr) = (attrs.clone(), refresh.clone(), busy.clone(), ATTRIBUTE_ORDER[k]);
        sc.connect_value_changed(move |sc| {
            if busy.get() {
                return;
            }
            let mut a = attrs.get();
            let others: f64 = Attribute::ALL.iter().filter(|x| **x != attr).map(|x| a.get(*x)).sum();
            // can't spend more points than are left
            let v = sc.value().round().min(rh::TOTAL_POINTS as f64 - others).max(rh::MIN_ATTRIBUTE as f64);
            a.set(attr, v);
            attrs.set(a);
            refresh();
        });
    }
    {
        let (implants, refresh) = (implants.clone(), refresh.clone());
        imp_scale.connect_value_changed(move |sc| {
            let v = sc.value().round() as i32;
            if v != implants.get() {
                implants.set(v);
                refresh();
            }
        });
    }
    {
        let (attrs, cache, refresh) = (attrs.clone(), cache.clone(), refresh.clone());
        use_opt.connect_clicked(move |_| {
            if let Some((_, best, _)) = cache.borrow().clone() {
                attrs.set(best.attributes);
            }
            refresh();
        });
    }
    {
        let (attrs, refresh) = (attrs.clone(), refresh.clone());
        use_cur.connect_clicked(move |_| {
            attrs.set(current);
            refresh();
        });
    }
    {
        let (win2, pp, attrs, implants) = (win.clone(), p.clone(), attrs.clone(), implants.clone());
        save.connect_clicked(move |_| {
            win2.close();
            let (a, imp) = (attrs.get(), implants.get());
            pp.edit(|pl| match index {
                Some(i) => pl.edit_remap(i, a, imp),
                // a new remap happens now, so it goes at the start of the plan; times after it are recalculated
                None => {
                    let mut items = vec![Item::remap(a, imp)];
                    items.extend(pl.queue.iter().cloned());
                    pl.rebuild(&items);
                }
            });
        });
    }
    let w2 = win.clone();
    cancel.connect_clicked(move |_| w2.close());
    refresh();
    let sw = gtk::ScrolledWindow::new();
    sw.set_hscrollbar_policy(gtk::PolicyType::Never);
    sw.set_propagate_natural_height(true);
    sw.set_max_content_height(p.ctx.window().height().max(600) - 120);
    if let Some(surface) = win.child() {
        win.set_child(None::<&gtk::Widget>);
        sw.set_child(Some(&surface));
        win.set_child(Some(&sw));
    }
    win.present();
}

/// The first remap as usual, then a second one after the skill the optimiser chose.
fn add_two_remaps(p: &P, index: Option<usize>, first: AttributeSet, implants: i32, after: (i64, u8), second: AttributeSet) {
    p.edit(|pl| {
        let mut items = pl.queue.clone();
        match index {
            None => items.insert(0, Item::remap(first, implants)),
            Some(i) => items[i] = Item::remap(first, implants),
        }
        let start = index.map(|i| i + 1).unwrap_or(1);
        let pos = items
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, it)| matches!(it, Item::Skill(s) if s.id == after.0 && s.level == after.1))
            .map(|(k, _)| k);
        if let Some(k) = pos {
            items.insert(k + 1, Item::remap(second, implants));
        }
        pl.rebuild(&items);
    });
}
