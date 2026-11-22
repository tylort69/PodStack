//! Import and export menus: PodStack plan files, EVEMon plans, EVE's skill plan text (clipboard or file), pasted
//! lists, the EVE skill queue and ship fittings.

use super::{dialogs, plural, P};
use crate::widgets as w;
use gtk::{gio, prelude::*};
use podstack_core::features::plan_tools::{io as pio, Planner};

type Action = Box<dyn Fn()>;

/// A toolbar button opening a menu.
fn menu(icon: &str, label: &str, tooltip: &str, items: Vec<(&str, Action)>) -> gtk::MenuButton {
    let mb = gtk::MenuButton::new();
    let row = w::hbox(8);
    row.append(&w::icon_sized(icon, 18));
    row.append(&gtk::Label::new(Some(label)));
    row.append(&w::icon_sized("arrow_drop_down", 18));
    mb.set_child(Some(&row));
    mb.add_css_class("text-button");
    mb.set_tooltip_text(Some(tooltip));
    let pop = gtk::Popover::new();
    pop.set_has_arrow(false);
    let bx = w::vbox(0);
    for (label, f) in items {
        let b = gtk::Button::with_label(label);
        b.add_css_class("flat");
        b.add_css_class("menu-item");
        if let Some(l) = b.child().and_then(|c| c.downcast::<gtk::Label>().ok()) {
            l.set_xalign(0.0);
        }
        let popw = pop.downgrade();
        b.connect_clicked(move |_| {
            if let Some(p) = popw.upgrade() {
                p.popdown();
            }
            f();
        });
        bx.append(&b);
    }
    pop.set_child(Some(&bx));
    mb.set_popover(Some(&pop));
    mb
}

fn filters(list: &[(&str, &[&str])]) -> gio::ListStore {
    let store = gio::ListStore::new::<gtk::FileFilter>();
    for (name, exts) in list {
        let f = gtk::FileFilter::new();
        f.set_name(Some(name));
        for e in *exts {
            if *e == "*" {
                f.add_pattern("*");
            } else {
                f.add_suffix(e);
            }
        }
        store.append(&f);
    }
    store
}

/// Asks for a file to read, then calls `done` with its name and text.
fn open_file(p: &P, title: &str, filter: &[(&str, &[&str])], done: impl Fn(String, String) + 'static) {
    let d = gtk::FileDialog::new();
    d.set_title(title);
    d.set_modal(true);
    d.set_filters(Some(&filters(filter)));
    let ctx = p.ctx.clone();
    d.open(Some(&p.ctx.window()), gio::Cancellable::NONE, move |r| {
        let Ok(file) = r else { return };
        let Some(path) = file.path() else { return };
        match std::fs::read(&path) {
            Ok(bytes) => {
                let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                done(name, String::from_utf8_lossy(&bytes).to_string());
            }
            Err(e) => ctx.toast(&format!("Couldn't read the file: {e}")),
        }
    });
}

/// Asks where to save, then writes `text` there and shows `message(count)`.
fn save_file(p: &P, title: &str, initial: &str, filter: &[(&str, &[&str])], text: String, message: String) {
    let d = gtk::FileDialog::new();
    d.set_title(title);
    d.set_modal(true);
    d.set_initial_name(Some(initial));
    d.set_filters(Some(&filters(filter)));
    let ctx = p.ctx.clone();
    d.save(Some(&p.ctx.window()), gio::Cancellable::NONE, move |r| {
        let Ok(file) = r else { return };
        let Some(path) = file.path() else { return };
        match std::fs::write(&path, text.as_bytes()) {
            Ok(()) => ctx.toast(&message),
            Err(e) => ctx.toast(&format!("Couldn't save the file: {e}")),
        }
    });
}

fn safe_name(s: &str) -> String {
    let n: String = s.chars().map(|c| if c == '/' || c == '\\' || c == '\0' { '-' } else { c }).collect();
    if n.trim().is_empty() {
        "Skill plan".into()
    } else {
        n
    }
}

pub(super) fn import_button(p: &P) -> gtk::Widget {
    let mk = |f: fn(&P)| -> Action {
        let pp = p.clone();
        Box::new(move || f(&pp))
    };
    menu(
        "file_download",
        "Import",
        "Add skills from a file, a paste, a ship fitting or the EVE skill queue",
        vec![
            ("PodStack plan", mk(import_podstack)),
            ("EVEMon plan", mk(import_evemon)),
            ("Text file (EVE skill plan or list)", mk(import_text_file)),
            ("Ship fitting", mk(|p| {
                p.st.borrow_mut().fit_open = true;
                p.render_right();
            })),
            ("Current EVE skill queue", mk(import_eve_queue)),
            ("EVE in-game plan", mk(|p| dialogs::paste(p, true))),
            ("Paste skill list", mk(|p| dialogs::paste(p, false))),
        ],
    )
    .upcast()
}

pub(super) fn export_button(p: &P) -> gtk::Widget {
    let mk = |f: fn(&P)| -> Action {
        let pp = p.clone();
        Box::new(move || f(&pp))
    };
    let b = menu(
        "file_upload",
        "Export",
        "Save or copy what's shown: the open plan or the training queue",
        vec![
            ("PodStack plan", mk(export_podstack)),
            ("EVE skill plan (copy to clipboard)", mk(copy_eve)),
            ("EVE skill plan (.txt file)", mk(export_eve_text)),
        ],
    );
    b.set_sensitive(p.st.borrow().planner.skill_count() > 0);
    b.upcast()
}

const PODSTACK: (&str, &[&str]) = ("PodStack skill plans", &["podstack_plan", "cerebral_plan"]);
const ALL: (&str, &[&str]) = ("All files", &["*"]);

fn import_podstack(p: &P) {
    let pp = p.clone();
    // .cerebral_plan is the legacy extension for the same JSON format
    open_file(p, "Import PodStack plan", &[PODSTACK, ALL], move |name, text| {
        pp.import_skills(&format!("Import of \"{name}\""), "PodStack JSON", &pio::import_json(&text));
    });
}

fn import_evemon(p: &P) {
    let pp = p.clone();
    open_file(p, "Import EVEMon plan", &[("EVEMon skill plans", &["xml"]), ALL], move |name, text| {
        pp.import_skills(&format!("Import of \"{name}\""), "EVEMon XML", &pio::import_evemon_xml(&text));
    });
}

fn import_text_file(p: &P) {
    let pp = p.clone();
    open_file(p, "Import skill list", &[("Text files", &["txt"]), ALL], move |name, text| {
        let (skills, source) = pio::import_any(&text);
        pp.import_skills(&format!("Import of \"{name}\""), source, &skills);
    });
}

/// A new plan holding the character's skill queue in EVE (as last loaded from ESI): the levels not yet trained,
/// in the queue's order. A paused queue counts too.
fn import_eve_queue(p: &P) {
    let Some(c) = p.character() else { return };
    let now = chrono::Utc::now();
    let mut entries: Vec<_> = c.queue().iter().filter(|q| q.finish_date.map(|f| f > now).unwrap_or(true)).cloned().collect();
    entries.sort_by_key(|q| q.queue_position);
    if entries.is_empty() {
        dialogs::alert(&p.ctx, "Nothing to import", &format!("{}'s skill queue in EVE is empty, so there's nothing to make a plan from.", c.name()), &[], None);
        return;
    }
    let mut pl = Planner::new(&p.base());
    pl.add_note("EVE skill queue", &format!("Copied from the skill queue in EVE on {}", podstack_core::fmt::datetime(now)));
    for q in &entries {
        pl.plan_skill(q.skill_id, q.finished_level, None);
    }
    p.create_plan(&format!("EVE skill queue {}", podstack_core::fmt::date(now)), &pl.queue);
    dialogs::prompt_optimise(p, true);
}

fn shown_name(p: &P) -> String {
    let st = p.st.borrow();
    if st.open.is_some() {
        st.name.clone()
    } else {
        "Training queue".into()
    }
}

fn export_podstack(p: &P) {
    let json = pio::export_json(&p.st.borrow().planner.queue);
    let n = p.st.borrow().planner.skill_count();
    save_file(
        p,
        "Export PodStack plan",
        &format!("{}.podstack_plan", safe_name(&shown_name(p))),
        &[("PodStack skill plans", &["podstack_plan"]), ALL],
        json,
        format!("Saved {n} skill level{} as a PodStack plan.", plural(n)),
    );
}

/// EVE's skill plan format on the clipboard, for Skills → Skill Plans → import in the EVE client.
pub(super) fn copy_eve(p: &P) {
    let text = pio::eve_text(&p.st.borrow().planner.queue);
    let n = pio::eve_text_count(&text);
    p.ctx.window().clipboard().set_text(&text);
    p.ctx.toast(&format!(
        "Copied {n} skill {} as an EVE skill plan. In EVE, import it from the clipboard in the Skill Plans window.",
        if n == 1 { "level" } else { "levels" }
    ));
}

fn export_eve_text(p: &P) {
    let text = pio::eve_text(&p.st.borrow().planner.queue);
    let n = pio::eve_text_count(&text);
    save_file(
        p,
        "Export EVE skill plan",
        &format!("{}.txt", safe_name(&shown_name(p))),
        &[("Text files", &["txt"]), ALL],
        text,
        format!(
            "Saved {n} skill {} as an EVE skill plan. Copy the file's text and import it from the clipboard in EVE's Skill Plans window.",
            if n == 1 { "level" } else { "levels" }
        ),
    );
}

