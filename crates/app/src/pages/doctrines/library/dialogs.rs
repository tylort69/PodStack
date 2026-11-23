//! Naming, moving and deleting groups and doctrines, and saving files.

use super::{plural, Lib, Sel};
use crate::pages::skill_browser::shared;
use crate::widgets::{self as w, Btn};
use gtk::{gio, prelude::*};
use podstack_core::features::doctrines::{self as dc, Doctrine, DoctrineFit, Group};
use std::rc::Rc;

/// A multi-line text field (notes) in the input frame style. Returns the frame and its buffer.
pub(crate) fn notes_field(initial: &str, placeholder: &str, min_height: i32) -> (gtk::Widget, gtk::TextBuffer) {
    let buffer = gtk::TextBuffer::new(None);
    buffer.set_text(initial);
    let view = gtk::TextView::with_buffer(&buffer);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_accepts_tab(false);
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_child(Some(&view));
    scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroll.set_min_content_height(min_height);
    scroll.set_max_content_height(min_height * 3);
    scroll.set_propagate_natural_height(true);
    let hint = w::text(placeholder, &["doclib-placeholder"]);
    hint.set_valign(gtk::Align::Start);
    hint.set_can_target(false);
    hint.set_visible(buffer.char_count() == 0);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&scroll));
    overlay.add_overlay(&hint);
    overlay.add_css_class("doclib-input-frame");
    overlay.set_overflow(gtk::Overflow::Hidden);
    buffer.connect_changed(move |b| hint.set_visible(b.char_count() == 0));
    (overlay.upcast(), buffer)
}

pub(crate) fn buffer_text(b: &gtk::TextBuffer) -> String {
    let (s, e) = b.bounds();
    b.text(&s, &e, false).to_string()
}

pub(crate) fn labelled(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    crate::pages::industry::form::labelled(label, child)
}

const GROUP_NOTES: &str = "Comms, fleet times, where the doctrines are posted…";
const DOCTRINE_NOTES: &str = "When it's used, the FC's rules, fitting notes…";

/// What a [`name_dialog`] says and starts with.
struct NameForm<'a> {
    title: &'a str,
    intro: Option<&'a str>,
    name: &'a str,
    name_hint: &'a str,
    notes: &'a str,
    notes_hint: &'a str,
    ok: &'a str,
}

/// A dialog asking for a name and notes. `on_ok` gets both (the name trimmed, never empty).
fn name_dialog(lib: &Rc<Lib>, form: NameForm, on_ok: impl Fn(String, String) + 'static) {
    let NameForm { title, intro, name, name_hint, notes, notes_hint, ok } = form;
    let (win, content, actions) = w::dialog(&lib.ctx, title, 480);
    if let Some(t) = intro {
        content.append(&w::text(t, &["body-medium", "dim"]));
    }
    let entry = gtk::Entry::new();
    entry.set_text(name);
    entry.set_placeholder_text(Some(name_hint));
    entry.set_activates_default(true);
    content.append(&labelled("Name", &entry));
    let (notes_frame, buffer) = notes_field(notes, notes_hint, 72);
    content.append(&labelled("Notes", &notes_frame));

    let cancel = w::button("Cancel", Btn::Text);
    let yes = w::button(ok, Btn::Filled);
    yes.set_sensitive(!name.trim().is_empty());
    let y = yes.clone();
    entry.connect_changed(move |e| y.set_sensitive(!e.text().trim().is_empty()));
    let wc = win.clone();
    cancel.connect_clicked(move |_| wc.close());
    let (wc, e) = (win.clone(), entry.clone());
    yes.connect_clicked(move |_| {
        let n = e.text().trim().to_string();
        if n.is_empty() {
            return;
        }
        on_ok(n, buffer_text(&buffer).trim().to_string());
        wc.close();
    });
    actions.append(&cancel);
    actions.append(&yes);
    win.set_default_widget(Some(&yes));
    win.present();
    entry.grab_focus();
}

/// A new group at the top level, or a subgroup of `parent`.
pub(crate) fn new_group(lib: &Rc<Lib>, parent: Option<&str>) {
    let doc = lib.doc();
    let parent_name = parent.and_then(|p| doc.group(p)).map(|g| g.name.clone());
    let (title, intro, hint) = match &parent_name {
        Some(p) => (
            "New subgroup".to_string(),
            format!("A SIG or squad inside {p}, such as Capitals, Blobs or Incursions. It can hold doctrines and subgroups of its own."),
            "e.g. Capitals",
        ),
        None => ("New group".to_string(), "People you fly with: an alliance, a corporation or a fleet community. Their doctrines go inside, in subgroups if you like.".to_string(), "e.g. your alliance"),
    };
    let l = lib.clone();
    let parent = parent.map(String::from);
    let form = NameForm { title: &title, intro: Some(&intro), name: "", name_hint: hint, notes: "", notes_hint: GROUP_NOTES, ok: "Create" };
    name_dialog(lib, form, move |name, notes| {
        let g = Group { id: dc::new_id(), name, notes, ..Default::default() };
        let id = g.id.clone();
        l.edit(|d| match parent.as_deref().and_then(|p| d.group_mut(p)) {
            Some(p) => p.groups.push(g),
            None => d.groups.push(g),
        });
        l.select(Sel::Group(id));
    });
}

pub(crate) fn new_doctrine(lib: &Rc<Lib>, group_id: &str) {
    let doc = lib.doc();
    let Some(g) = doc.group(group_id) else { return };
    let intro = format!("A fleet composition of {}, such as Home Defense or Armor Cruisers. Add the fits it accepts next.", g.name);
    let l = lib.clone();
    let gid = group_id.to_string();
    let form = NameForm { title: "New doctrine", intro: Some(&intro), name: "", name_hint: "e.g. Home Defense", notes: "", notes_hint: DOCTRINE_NOTES, ok: "Create" };
    name_dialog(lib, form, move |name, notes| {
        let d = Doctrine { id: dc::new_id(), name, notes, fits: vec![] };
        let id = d.id.clone();
        l.edit(|doc| {
            if let Some(g) = doc.group_mut(&gid) {
                g.doctrines.push(d);
            }
        });
        l.select(Sel::Doctrine(id));
    });
}

pub(crate) fn edit_group(lib: &Rc<Lib>, id: &str) {
    let doc = lib.doc();
    let Some(g) = doc.group(id) else { return };
    let l = lib.clone();
    let id = id.to_string();
    let form = NameForm { title: "Edit group", intro: None, name: &g.name, name_hint: "Group name", notes: &g.notes, notes_hint: GROUP_NOTES, ok: "Save" };
    name_dialog(lib, form, move |name, notes| {
        l.edit(|d| {
            if let Some(g) = d.group_mut(&id) {
                g.name = name.clone();
                g.notes = notes.clone();
            }
        });
    });
}

pub(crate) fn edit_doctrine(lib: &Rc<Lib>, id: &str) {
    let doc = lib.doc();
    let Some((_, _, d)) = doc.doctrine(id) else { return };
    let l = lib.clone();
    let id = id.to_string();
    let form = NameForm { title: "Edit doctrine", intro: None, name: &d.name, name_hint: "Doctrine name", notes: &d.notes, notes_hint: DOCTRINE_NOTES, ok: "Save" };
    name_dialog(lib, form, move |name, notes| {
        l.edit(|doc| {
            if let Some(d) = doc.doctrine_mut(&id) {
                d.name = name.clone();
                d.notes = notes.clone();
            }
        });
    });
}

/// Asks for a destination among `choices` (None = the top level) and calls `on_ok` with it.
fn pick_dialog(lib: &Rc<Lib>, title: &str, intro: &str, choices: Vec<(Option<String>, String)>, ok: &str, on_ok: impl Fn(Option<String>) + 'static) {
    let (win, content, actions) = w::dialog(&lib.ctx, title, 460);
    content.append(&w::text(intro, &["body-medium", "dim"]));
    let cancel = w::button("Cancel", Btn::Text);
    let wc = win.clone();
    cancel.connect_clicked(move |_| wc.close());
    actions.append(&cancel);
    if choices.is_empty() {
        content.append(&w::banner("info", "There's nowhere else to move it: make another group first.", w::BannerKind::Info));
        win.present();
        return;
    }
    let labels: Vec<String> = choices.iter().map(|c| c.1.clone()).collect();
    let dd = shared::dropdown(&labels, 0);
    dd.set_hexpand(true);
    content.append(&labelled("Move to", &dd));
    let yes = w::button(ok, Btn::Filled);
    let wc = win.clone();
    yes.connect_clicked(move |_| {
        if let Some((id, _)) = choices.get(dd.selected() as usize) {
            on_ok(id.clone());
        }
        wc.close();
    });
    actions.append(&yes);
    win.present();
}

pub(crate) fn move_doctrine(lib: &Rc<Lib>, id: &str) {
    let doc = lib.doc();
    let Some((_, current, d)) = doc.doctrine(id) else { return };
    let choices: Vec<(Option<String>, String)> =
        doc.all_groups().into_iter().filter(|(_, g)| g.id != current.id).map(|(p, g)| (Some(g.id.clone()), p.join(" \u{203a} "))).collect();
    let l = lib.clone();
    let id = id.to_string();
    let intro = format!("Move \u{201c}{}\u{201d} and its {} to another group or subgroup.", d.name, plural(d.fits.len(), "fit", "fits"));
    pick_dialog(lib, "Move doctrine", &intro, choices, "Move", move |to| {
        let Some(to) = to else { return };
        if l.edit(|d| d.move_doctrine(&id, &to)) {
            l.ctx.toast("Doctrine moved");
            l.select(Sel::Doctrine(id.clone()));
        }
    });
}

pub(crate) fn move_group(lib: &Rc<Lib>, id: &str) {
    let doc = lib.doc();
    let Some(g) = doc.group(id) else { return };
    let parent = doc.parent_of(id).map(|p| p.id.clone());
    let mut choices: Vec<(Option<String>, String)> = Vec::new();
    if parent.is_some() {
        choices.push((None, "Top level (its own group)".into()));
    }
    for (p, other) in doc.all_groups() {
        if !doc.is_within(&other.id, id) && Some(&other.id) != parent.as_ref() {
            choices.push((Some(other.id.clone()), format!("Inside {}", p.join(" \u{203a} "))));
        }
    }
    let l = lib.clone();
    let id = id.to_string();
    let intro = format!("Move \u{201c}{}\u{201d} with everything in it.", g.name);
    pick_dialog(lib, "Move group", &intro, choices, "Move", move |to| {
        if l.edit(|d| d.move_group(&id, to.as_deref())) {
            l.ctx.toast("Group moved");
            l.select(Sel::Group(id.clone()));
        }
    });
}

/// What deleting a group takes with it, for the confirmation.
fn group_contents(g: &Group) -> String {
    let subs = g.walk().len() - 1;
    let doctrines = g.all_doctrines().len();
    let fits = g.fit_count();
    let mut parts = vec![];
    if subs > 0 {
        parts.push(plural(subs, "subgroup", "subgroups"));
    }
    if doctrines > 0 {
        parts.push(plural(doctrines, "doctrine", "doctrines"));
    }
    if fits > 0 {
        parts.push(plural(fits, "fit", "fits"));
    }
    if parts.is_empty() {
        "It's empty.".into()
    } else {
        format!("This deletes its {}.", parts.join(", "))
    }
}

pub(crate) fn delete_group(lib: &Rc<Lib>, id: &str) {
    let doc = lib.doc();
    let Some(g) = doc.group(id) else { return };
    let parent = doc.parent_of(id).map(|p| p.id.clone());
    let body = format!("{} Saving it as a PodStack file first keeps a copy. This can't be undone.", group_contents(g));
    let l = lib.clone();
    let id = id.to_string();
    w::confirm(&lib.ctx, &format!("Delete \u{201c}{}\u{201d}?", g.name), &body, "Delete", true, move || {
        l.edit(|d| d.remove_group(&id));
        l.ctx.toast("Group deleted");
        l.select(parent.clone().map(Sel::Group).unwrap_or(Sel::Library));
    });
}

pub(crate) fn delete_doctrine(lib: &Rc<Lib>, id: &str) {
    let doc = lib.doc();
    let Some((_, g, d)) = doc.doctrine(id) else { return };
    let group = g.id.clone();
    let body = if d.fits.is_empty() {
        "It has no fits. This can't be undone.".to_string()
    } else {
        format!("This deletes its {}. This can't be undone.", plural(d.fits.len(), "fit", "fits"))
    };
    let l = lib.clone();
    let id = id.to_string();
    w::confirm(&lib.ctx, &format!("Delete \u{201c}{}\u{201d}?", d.name), &body, "Delete", true, move || {
        l.edit(|d| d.remove_doctrine(&id));
        l.ctx.toast("Doctrine deleted");
        l.select(Sel::Group(group.clone()));
    });
}

pub(crate) fn delete_fit(lib: &Rc<Lib>, id: &str, after: impl Fn() + 'static) {
    let doc = lib.doc();
    let Some((_, d, f)) = doc.fit(id) else { return };
    let body = format!("Removes it from {}. This can't be undone.", d.name);
    let l = lib.clone();
    let id = id.to_string();
    w::confirm(&lib.ctx, &format!("Delete \u{201c}{}\u{201d}?", f.name), &body, "Delete", true, move || {
        l.edit(|d| d.remove_fit(&id));
        l.ctx.toast("Fit deleted");
        after();
    });
}

/// Moves something one place up or down among its siblings.
pub(crate) fn shift(lib: &Rc<Lib>, id: &str, up: bool) {
    lib.edit(|d| d.shift(id, up));
}

/// Whether something can move up / down among its siblings.
pub(crate) fn can_shift(lib: &Lib, id: &str) -> (bool, bool) {
    match lib.doc().position(id) {
        Some((i, n)) => (i > 0, i + 1 < n),
        None => (false, false),
    }
}

/// A file name from a group or doctrine name: "Init Capitals.json" → "init-capitals.json".
pub(crate) fn file_name(name: &str, ext: &str) -> String {
    let slug: String = name.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let slug = slug.split('-').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("-");
    format!("{}.{ext}", if slug.is_empty() { "doctrines".into() } else { slug })
}

/// Asks where to save `text`, writes it and says `done` in a toast.
fn save_text(lib: &Rc<Lib>, title: &str, name: &str, filter: (&str, &str), text: String, done: String) {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    let f = gtk::FileFilter::new();
    f.set_name(Some(filter.0));
    f.add_suffix(filter.1);
    filters.append(&f);
    let d = gtk::FileDialog::new();
    d.set_title(title);
    d.set_modal(true);
    d.set_initial_name(Some(name));
    d.set_filters(Some(&filters));
    let ctx = lib.ctx.clone();
    d.save(Some(&ctx.window()), gio::Cancellable::NONE, move |r| {
        let Ok(file) = r else { return };
        let Some(path) = file.path() else { return };
        match std::fs::write(&path, text.as_bytes()) {
            Ok(()) => ctx.toast(&done),
            Err(e) => ctx.toast(&format!("Couldn't save the file: {e}")),
        }
    });
}

/// Saves groups as a PodStack doctrines file (to share with corp mates).
pub(crate) fn save_json(lib: &Rc<Lib>, name: &str, groups: &[Group]) {
    let fits: usize = groups.iter().map(|g| g.fit_count()).sum();
    if groups.is_empty() {
        lib.ctx.toast("Nothing to save yet");
        return;
    }
    let done = format!("Saved {} to share", plural(fits, "fit", "fits"));
    save_text(lib, "Save as a PodStack doctrines file", name, ("PodStack doctrines", "json"), dc::export_json(groups), done);
}

/// Saves fits as one EFT text file (Pyfa imports them all at once).
pub(crate) fn save_eft(lib: &Rc<Lib>, name: &str, fits: &[DoctrineFit]) {
    if fits.is_empty() {
        lib.ctx.toast("No fits to save");
        return;
    }
    let done = format!("Saved {} as EFT", plural(fits.len(), "fit", "fits"));
    save_text(lib, "Save fits as EFT", name, ("EFT fittings", "txt"), dc::export_eft(fits), done);
}

/// Shares one doctrine as a PodStack file: inside a copy of its group (without the group's other content), so it
/// merges into the same group on a corp mate's PodStack.
pub(crate) fn save_doctrine_json(lib: &Rc<Lib>, id: &str) {
    let doc = lib.doc();
    let Some((_, g, d)) = doc.doctrine(id) else { return };
    let wrapper = Group { id: g.id.clone(), name: g.name.clone(), notes: g.notes.clone(), groups: vec![], doctrines: vec![d.clone()] };
    save_json(lib, &file_name(&d.name, "json"), &[wrapper]);
}
