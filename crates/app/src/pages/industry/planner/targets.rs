//! The open project's targets: what to build and how many, added one by one, from a fit or from a pasted list.

use super::{State, NOTE_KEY};
use crate::pages::industry::{common, icon_url, type_cell};
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::blueprints::{Activity, INDUSTRY};
use podstack_core::features::fitting::{self, ItemRef};
use podstack_core::features::trade;
use podstack_core::sde::MARKET;
use std::rc::Rc;

pub(super) fn card(state: &Rc<State>) -> gtk::Box {
    let col = w::vbox(8);
    let project = state.project();
    let targets = project.as_ref().map(|p| p.targets.clone()).unwrap_or_default();

    let add = adder(state);
    if let Some(parent) = add.parent().and_downcast::<gtk::Box>() {
        parent.remove(&add);
    }
    col.append(&add);

    let note = state.ctx.recall(NOTE_KEY).unwrap_or_default();
    if !note.is_empty() {
        let b = w::banner("block", &note, w::BannerKind::Notice);
        let close = w::icon_button("close", "Dismiss");
        let state2 = state.clone();
        let b2 = b.clone();
        close.connect_clicked(move |_| {
            state2.ctx.remember(NOTE_KEY, "");
            b2.set_visible(false);
        });
        b.append(&close);
        col.append(&b);
    }

    if targets.is_empty() {
        col.append(&w::text("Nothing yet. Pick an item above, or import a fit or a list of items.", &["body-medium", "dim"]));
    }
    let list = w::vbox(2);
    for (t, q) in &targets {
        list.append(&target_row(state, *t, *q));
    }
    col.append(&list);

    let import_fit = w::button_icon("rocket_launch", "Import fit", w::Btn::Tonal);
    import_fit.set_tooltip_text(Some("Add a ship and everything fitted to it (EFT, XML or DNA)"));
    {
        let state = state.clone();
        import_fit.connect_clicked(move |_| fit_dialog(&state));
    }
    let paste = w::button_icon("content_paste", "Paste list", w::Btn::Tonal);
    paste.set_tooltip_text(Some("Add a list of items (\"Name x5\", \"5 Name\", columns copied from the game)"));
    {
        let state = state.clone();
        paste.connect_clicked(move |_| list_dialog(&state));
    }
    let mut actions: Vec<gtk::Widget> = vec![import_fit.upcast(), paste.upcast()];
    if targets.len() > 1 {
        let clear = w::icon_button("clear_all", "Remove all targets");
        let state = state.clone();
        clear.connect_clicked(move |_| {
            let state = state.clone();
            w::confirm(&state.ctx.clone(), "Remove all targets?", "The project keeps its name and choices.", "Remove all", true, move || {
                state.with_project(|p| p.targets.clear());
                state.render_all();
            });
        });
        actions.push(clear.upcast());
    }
    let units: i64 = targets.iter().map(|t| t.1).sum();
    let sub = match targets.len() {
        0 => None,
        n => Some(format!("{n} item{} · {} units", if n == 1 { "" } else { "s" }, podstack_core::fmt::int(units))),
    };
    w::card_with("Build", sub.as_deref(), &actions, &col)
}

/// The item search and the quantity to add, made once per page and moved into each redraw: the search keeps its
/// suggestions popover (parented to the entry) and, after adding, the keyboard focus.
fn adder(state: &Rc<State>) -> gtk::Box {
    if let Some(a) = state.adder.borrow().as_ref() {
        return a.clone();
    }
    let add = w::hbox(8);
    let qty = gtk::SpinButton::with_range(1.0, 1e9, 1.0);
    qty.set_digits(0);
    qty.set_value(1.0);
    qty.set_tooltip_text(Some("How many to add"));
    qty.set_valign(gtk::Align::Start);
    let picker = {
        let (weak, qty) = (Rc::downgrade(state), qty.clone());
        common::product_picker("Add something to build, e.g. Venture", move |t| {
            let Some(state) = weak.upgrade() else { return };
            state.add_targets(&[(t, qty.value_as_int().max(1) as i64)]);
            qty.set_value(1.0);
            gtk::glib::idle_add_local_once(move || state.render_all());
        })
    };
    picker.set_hexpand(true);
    add.append(&picker);
    add.append(&w::line("×", &["dim"]));
    add.append(&qty);
    *state.adder.borrow_mut() = Some(add.clone());
    add
}

fn target_row(state: &Rc<State>, type_id: i64, quantity: i64) -> gtk::Box {
    let row = w::hbox(8);
    row.add_css_class("ind-plan-target");
    let name = INDUSTRY.type_name(type_id);
    let pill = match INDUSTRY.maker(type_id) {
        None => Some(w::status("Not buildable", w::Status::Bad)),
        Some((_, Activity::Reaction)) => Some(w::status("Reaction", w::Status::Neutral)),
        Some(_) => None,
    };
    let sub = INDUSTRY.get_type(type_id).and_then(|t| INDUSTRY.group_name(t.group)).map(|s| s.to_string());
    let cell = type_cell(&icon_url(type_id, &name, false), &name, sub.as_deref(), pill);
    cell.set_hexpand(true);
    row.append(&cell);
    let spin = gtk::SpinButton::with_range(1.0, 1e9, 1.0);
    spin.set_digits(0);
    spin.set_value(quantity as f64);
    spin.set_valign(gtk::Align::Center);
    spin.set_tooltip_text(Some("Units to build"));
    {
        let state = state.clone();
        spin.connect_value_changed(move |s| {
            let n = s.value_as_int().max(1) as i64;
            let created = state.with_project(|p| {
                if let Some(t) = p.targets.iter_mut().find(|t| t.0 == type_id) {
                    t.1 = n;
                }
            });
            state.after_edit(created, true);
        });
    }
    row.append(&spin);
    if INDUSTRY.maker(type_id).is_some() {
        let calc = w::icon_button("calculate", "Open in the calculator");
        let ctx = state.ctx.clone();
        calc.connect_clicked(move |_| common::open_in_calculator(&ctx, type_id));
        calc.set_valign(gtk::Align::Center);
        row.append(&calc);
    }
    let remove = w::icon_button("close", "Remove");
    remove.set_valign(gtk::Align::Center);
    {
        let state = state.clone();
        remove.connect_clicked(move |_| {
            state.with_project(|p| p.targets.retain(|t| t.0 != type_id));
            let state = state.clone();
            gtk::glib::idle_add_local_once(move || state.render_all());
        });
    }
    row.append(&remove);
    row
}

/// The type an item reference names, from the market's names.
fn resolve(item: &ItemRef) -> Result<i64, String> {
    match item {
        ItemRef::TypeId(id) => Ok(*id),
        ItemRef::Name(n) => MARKET.by_name(n).map(|t| t.id).ok_or_else(|| n.clone()),
    }
}

/// Splits items into what something in the game builds and the names of the rest.
fn split_buildable(items: impl IntoIterator<Item = (Result<i64, String>, i64)>) -> (Vec<(i64, i64)>, Vec<String>) {
    let mut build: Vec<(i64, i64)> = Vec::new();
    let mut skipped = Vec::new();
    for (item, q) in items {
        match item {
            Ok(t) if INDUSTRY.maker(t).is_some() => match build.iter_mut().find(|b| b.0 == t) {
                Some(b) => b.1 += q,
                None => build.push((t, q)),
            },
            Ok(t) => skipped.push(INDUSTRY.type_name(t)),
            Err(name) => skipped.push(name),
        }
    }
    (build, skipped)
}

/// A dialog with a paste box, a multiplier and an Import button; `import` gets the text and the multiplier and
/// returns the targets to add and the names nothing builds, or an error to show.
fn paste_dialog(state: &Rc<State>, title: &str, hint: &str, times_label: &str, import: impl Fn(&str, i64) -> Result<(Vec<(i64, i64)>, Vec<String>), String> + 'static) {
    let (win, content, actions) = w::dialog(&state.ctx, title, 560);
    content.append(&w::text(hint, &["body-medium", "dim"]));
    let view = gtk::TextView::new();
    view.add_css_class("ind-plan-paste");
    view.set_wrap_mode(gtk::WrapMode::None);
    let scroll = w::scrolled(&view);
    scroll.set_min_content_height(240);
    scroll.set_hscrollbar_policy(gtk::PolicyType::Automatic);
    content.append(&scroll);
    let times = gtk::SpinButton::with_range(1.0, 100_000.0, 1.0);
    times.set_digits(0);
    times.set_value(1.0);
    let row = w::hbox(8);
    row.append(&w::line(times_label, &["body-medium"]));
    row.append(&times);
    content.append(&row);
    let error = w::text("", &["body-medium", "error-text"]);
    error.set_visible(false);
    content.append(&error);
    let cancel = w::button("Cancel", w::Btn::Text);
    let go = w::button_icon("add", "Add to project", w::Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (state, win, view, times, error) = (state.clone(), win.clone(), view.clone(), times.clone(), error.clone());
        go.connect_clicked(move |_| {
            let (s, e) = view.buffer().bounds();
            let text = view.buffer().text(&s, &e, false);
            match import(text.as_str(), times.value_as_int().max(1) as i64) {
                Ok((build, skipped)) if build.is_empty() => {
                    let what = if skipped.is_empty() { String::new() } else { format!(" ({})", skipped.join(", ")) };
                    error.set_text(&format!("Nothing in there can be manufactured{what}."));
                    error.set_visible(true);
                }
                Ok((build, skipped)) => {
                    state.add_targets(&build);
                    state.set_note(skipped);
                    win.close();
                    state.render_all();
                }
                Err(msg) => {
                    error.set_text(&msg);
                    error.set_visible(true);
                }
            }
        });
    }
    actions.append(&cancel);
    actions.append(&go);
    win.present();
    view.grab_focus();
}

fn fit_dialog(state: &Rc<State>) {
    paste_dialog(
        state,
        "Import a fit",
        "Paste a fit copied from the game's fitting window (EFT), a saved XML fitting or a DNA string. The ship, modules, rigs, drones and charges are added; officer, deadspace and other items nobody can manufacture are left out.",
        "Number of fits",
        |text, n| {
            let fits = fitting::parse_fits(text)?;
            let mut items: Vec<(Result<i64, String>, i64)> = Vec::new();
            for fit in &fits {
                items.push((resolve(&fit.ship), n));
                for it in &fit.items {
                    items.push((resolve(&it.item), it.quantity as i64 * n));
                }
            }
            Ok(split_buildable(items))
        },
    );
}

fn list_dialog(state: &Rc<State>) {
    paste_dialog(
        state,
        "Paste a list",
        "One item per line: \"Name x5\", \"5 Name\", or rows copied from an inventory, contract or the multibuy window. Items nobody can manufacture are left out.",
        "Multiply by",
        |text, n| {
            let parsed = trade::parse_items(text);
            if parsed.items.is_empty() {
                return Err("None of the lines named an item.".into());
            }
            let items = parsed.items.iter().map(|(t, q)| (Ok(*t), q * n)).chain(parsed.unknown.iter().map(|u| (Err(u.clone()), 0)));
            Ok(split_buildable(items))
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_import_keeps_what_can_be_built() {
        let fit = "[Venture, Mining]\nMiner II\nMiner II\nCaldari Navy Ballistic Control System\nPithum C-Type Medium Shield Booster\n\nHobgoblin I x2\n";
        let fits = fitting::parse_fits(fit).unwrap();
        let mut items: Vec<(Result<i64, String>, i64)> = Vec::new();
        for f in &fits {
            items.push((resolve(&f.ship), 3));
            for it in &f.items {
                items.push((resolve(&it.item), it.quantity as i64 * 3));
            }
        }
        let (build, skipped) = split_buildable(items);
        let id = |n: &str| MARKET.by_name(n).unwrap().id;
        assert!(build.contains(&(id("Venture"), 3)), "{build:?}");
        assert!(build.contains(&(id("Miner II"), 6)), "{build:?}");
        assert!(build.contains(&(id("Hobgoblin I"), 6)), "{build:?}");
        // LP store faction items have blueprints; deadspace and officer items don't
        assert!(build.contains(&(id("Caldari Navy Ballistic Control System"), 3)), "{build:?}");
        assert_eq!(skipped, vec!["Pithum C-Type Medium Shield Booster".to_string()]);
    }
}
