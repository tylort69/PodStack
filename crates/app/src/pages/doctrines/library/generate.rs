//! Doctrines from in-game fittings, on request: a character's saved fittings (through ESI) or a fittings export file,
//! grouped into doctrines by their names (see `features::doctrine_generate`). A preview shows the doctrines first;
//! generating looks each fit up, then adds a new group under "Auto-generated", so nothing already there changes.

use super::{plural, Lib, Sel};
use crate::pages::skill_browser::shared;
use crate::rt;
use crate::widgets::{self as w, BannerKind, Btn};
use gtk::{gio, prelude::*};
use podstack_core::features::doctrine_generate as dg;
use podstack_core::features::doctrines::{self as dc, DoctrineFit};
use podstack_core::features::fitting::{self, ParsedFit};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Gen {
    lib: Rc<Lib>,
    win: gtk::Window,
    /// The characters offered, in the dropdown's order: (id, name, can read fittings).
    chars: Vec<(i64, String, bool)>,
    pick: gtk::DropDown,
    read: gtk::Button,
    open_file: gtk::Button,
    preview: gtk::Box,
    bar: gtk::ProgressBar,
    bar_label: gtk::Label,
    generate: gtk::Button,
    /// The fits read, and where they came from ("Kira Vex", "fittings.xml").
    fits: RefCell<Option<(Vec<ParsedFit>, String)>>,
    closed: Cell<bool>,
}

pub(super) fn open(lib: &Rc<Lib>) {
    let (win, content, actions) = w::dialog(&lib.ctx, "Doctrines from in-game fittings", 600);
    content.append(&w::text(
        "Reads fittings and makes a doctrine of each set whose names belong together (\"INIT Muninn - DPS\" and \"INIT \
         Muninn - Logi\" make Muninn). They go into a new group under Auto-generated: nothing you have changes.",
        &["body-medium"],
    ));
    content.append(&w::text(
        "EVE only lets apps read a character's own saved fittings, not corporation or alliance ones. To use alliance \
         fits, save them to that character's personal fittings in game, or export them from the Fitting window to a \
         file and open that here.",
        &["body-small", "dim"],
    ));

    let auths = lib.ctx.core().auth.all();
    let mut chars: Vec<(i64, String, bool)> = lib
        .ctx
        .core()
        .characters()
        .iter()
        .map(|c| (c.id, c.name(), auths.iter().any(|a| a.id == c.id && a.has_scope(dg::FITTINGS_SCOPE) && !a.needs_reauth())))
        .collect();
    chars.sort_by_key(|(_, n, ok)| (!ok, n.to_lowercase()));
    let labels: Vec<String> =
        chars.iter().map(|(_, n, ok)| if *ok { n.clone() } else { format!("{n} (log in again to allow Read Fittings)") }).collect();

    let row = w::hbox(8);
    let pick = shared::dropdown(&labels, 0);
    pick.set_hexpand(true);
    row.append(&pick);
    let read = w::button_icon("download", "Read fittings", Btn::Tonal);
    row.append(&read);
    content.append(&row);
    let open_file = w::button_icon("file_open", "Or open a fittings export file\u{2026}", Btn::Text);
    open_file.set_halign(gtk::Align::Start);
    content.append(&open_file);

    let preview = w::vbox(8);
    content.append(&preview);
    let bar = gtk::ProgressBar::new();
    bar.set_visible(false);
    content.append(&bar);
    let bar_label = w::line("", &["body-small", "dim"]);
    bar_label.set_visible(false);
    content.append(&bar_label);

    let cancel = w::button("Cancel", Btn::Text);
    actions.append(&cancel);
    let generate = w::button_icon("auto_awesome", "Generate", Btn::Filled);
    generate.set_sensitive(false);
    actions.append(&generate);

    let st = Rc::new(Gen {
        lib: lib.clone(),
        win: win.clone(),
        chars,
        pick,
        read,
        open_file,
        preview,
        bar,
        bar_label,
        generate,
        fits: RefCell::new(None),
        closed: Cell::new(false),
    });
    let can_read = {
        let st = Rc::downgrade(&st);
        move || {
            let Some(st) = st.upgrade() else { return };
            let ok = st.chars.get(st.pick.selected() as usize).is_some_and(|c| c.2);
            st.read.set_sensitive(ok);
        }
    };
    can_read();
    st.pick.connect_selected_notify(move |_| can_read());
    {
        let s = st.clone();
        st.read.connect_clicked(move |_| read_character(&s));
    }
    {
        let s = st.clone();
        st.open_file.connect_clicked(move |_| read_file(&s));
    }
    {
        let s = st.clone();
        st.generate.connect_clicked(move |_| start(&s));
    }
    {
        let w2 = win.clone();
        cancel.connect_clicked(move |_| w2.close());
    }
    {
        // the dialog holds `st` through its buttons: drop that when it closes, and stop a lookup in progress
        let s = Rc::downgrade(&st);
        win.connect_close_request(move |_| {
            if let Some(s) = s.upgrade() {
                s.closed.set(true);
            }
            gtk::glib::Propagation::Proceed
        });
    }
    if st.chars.is_empty() {
        st.read.set_sensitive(false);
    }
    win.present();
}

fn busy(st: &Gen, on: bool, label: &str) {
    st.read.set_sensitive(!on && st.chars.get(st.pick.selected() as usize).is_some_and(|c| c.2));
    st.open_file.set_sensitive(!on);
    st.pick.set_sensitive(!on);
    st.generate.set_sensitive(!on && st.fits.borrow().as_ref().is_some_and(|(f, _)| !f.is_empty()));
    st.bar.set_visible(on);
    st.bar_label.set_visible(on || !label.is_empty());
    st.bar_label.set_text(label);
}

fn read_character(st: &Rc<Gen>) {
    let Some((id, name, _)) = st.chars.get(st.pick.selected() as usize).cloned() else { return };
    busy(st, true, &format!("Reading {name}'s fittings\u{2026}"));
    st.bar.pulse();
    let core = st.lib.ctx.core().clone();
    let s = st.clone();
    rt::run(async move { dg::character_fittings(&core, id).await }, move |r| {
        if s.closed.get() {
            return;
        }
        match r {
            Ok(fits) => show(&s, fits, name),
            Err(e) => {
                *s.fits.borrow_mut() = None;
                busy(&s, false, "");
                shared::clear(&s.preview);
                s.preview.append(&w::banner("error", &e, BannerKind::Error));
            }
        }
    });
}

fn read_file(st: &Rc<Gen>) {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    let f = gtk::FileFilter::new();
    f.set_name(Some("Fittings (EVE XML or EFT)"));
    for ext in ["xml", "txt", "eft", "cfg"] {
        f.add_suffix(ext);
    }
    filters.append(&f);
    let d = gtk::FileDialog::new();
    d.set_title("Open a fittings export");
    d.set_modal(true);
    d.set_filters(Some(&filters));
    let s = st.clone();
    d.open(Some(&st.win), gio::Cancellable::NONE, move |r| {
        let Ok(file) = r else { return };
        let Some(path) = file.path() else { return };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let text = match std::fs::read(&path) {
            Ok(b) => String::from_utf8_lossy(&b).to_string(),
            Err(e) => {
                s.lib.ctx.toast(&format!("Couldn't read the file: {e}"));
                return;
            }
        };
        let parsed = if text.trim_start().starts_with('<') {
            fitting::parse_xml(&text)
        } else {
            dc::parse_paste(&text).map(|v| v.into_iter().map(|(p, _)| p).collect())
        };
        match parsed {
            Ok(fits) => show(&s, fits, name),
            Err(e) => s.lib.ctx.toast(&e),
        }
    });
}

/// The doctrines the fits would make.
fn show(st: &Rc<Gen>, fits: Vec<ParsedFit>, source: String) {
    shared::clear(&st.preview);
    let names: Vec<String> = fits.iter().map(name_of).collect();
    let groups = dg::group_names(&names);
    if fits.is_empty() {
        st.preview.append(&w::banner("info", &format!("{source} has no saved fittings."), BannerKind::Info));
    } else {
        st.preview.append(&w::line(
            &format!("{} from {source}, as {}:", plural(fits.len(), "fitting", "fittings"), plural(groups.len(), "doctrine", "doctrines")),
            &["title-small"],
        ));
        let list = w::vbox(2);
        for (doctrine, idx) in &groups {
            let fit_names: Vec<&str> = idx.iter().map(|&i| names[i].as_str()).collect();
            let l = w::line(&format!("{doctrine} \u{00b7} {}", plural(idx.len(), "fit", "fits")), &["body-medium"]);
            l.set_tooltip_text(Some(&fit_names.join("\n")));
            list.append(&l);
        }
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        // a fixed height: the dialog is sized when it opens, and a compositor needn't let it grow
        scroll.set_min_content_height(220);
        scroll.set_max_content_height(220);
        scroll.set_child(Some(&list));
        st.preview.append(&scroll);
    }
    *st.fits.borrow_mut() = Some((fits, source));
    busy(st, false, "");
}

fn name_of(f: &ParsedFit) -> String {
    f.fit_name.clone().unwrap_or_else(|| dc::parsed_ship_label(f))
}

fn start(st: &Rc<Gen>) {
    let Some((fits, source)) = st.fits.borrow().clone() else { return };
    busy(st, true, "");
    resolve_next(st.clone(), Rc::new(fits), 0, Vec::new(), 0, source);
}

/// Looks the fits up one after the other (so the bar moves), then adds the doctrines.
fn resolve_next(st: Rc<Gen>, fits: Rc<Vec<ParsedFit>>, i: usize, mut done: Vec<DoctrineFit>, failed: usize, source: String) {
    if st.closed.get() {
        return;
    }
    let n = fits.len();
    if i >= n {
        finish(&st, done, failed, &source);
        return;
    }
    st.bar.set_fraction(i as f64 / n as f64);
    st.bar_label.set_text(&format!("Looking up fit {} of {n}\u{2026}", i + 1));
    let parsed = fits[i].clone();
    let core = st.lib.ctx.core().clone();
    rt::run(async move { dc::resolve(&core, &parsed, "").await }, move |r| {
        let failed = match r {
            Ok(f) => {
                done.push(f);
                failed
            }
            Err(e) => {
                tracing::warn!("generating doctrines: {e}");
                failed + 1
            }
        };
        resolve_next(st, fits, i + 1, done, failed, source);
    });
}

fn finish(st: &Rc<Gen>, done: Vec<DoctrineFit>, failed: usize, source: &str) {
    if done.is_empty() {
        busy(st, false, "None of the fittings could be looked up.");
        return;
    }
    let doctrines = dg::doctrines(done);
    let count = doctrines.len();
    let name = format!("From {source} \u{00b7} {}", chrono::Local::now().format("%Y-%m-%d"));
    let id = st.lib.edit(|doc| dg::add_generated(doc, &name, doctrines));
    st.win.close();
    st.lib.select(Sel::Group(id));
    let skipped = if failed > 0 { format!(" ({failed} fittings couldn't be looked up)") } else { String::new() };
    st.lib.ctx.toast(&format!("Made {} under {}{skipped}", plural(count, "doctrine", "doctrines"), dg::AUTO_GROUP));
}
