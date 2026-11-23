//! Bringing fits in: a paste dialog (several EFT fits at once, EVE's XML or a DNA string) with a preview of what it
//! found, looked up one fit at a time with a progress bar and added to a doctrine (an existing one or a new one);
//! files (EFT .txt, EVE .xml, PodStack .json); and PodStack doctrines files merged into the library.

use super::dialogs::{buffer_text, labelled};
use super::{plural, ship_icon, Lib, Sel};
use crate::pages::skill_browser::shared;
use crate::rt;
use crate::widgets::{self as w, BannerKind, Btn};
use gtk::{gio, prelude::*};
use podstack_core::features::doctrines::{self as dc, Doctrine, DoctrineFit, Group};
use podstack_core::features::fitting::ParsedFit;
use podstack_core::sde::SHIPS;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const PLACEHOLDER: &str = "[Ishtar, Home Defense]\nDrone Damage Amplifier II\n…\n\n[Guardian, Home Defense Logi]\n…\n\nor EVE fitting XML, or a DNA string";

/// What a paste holds.
enum Found {
    Nothing,
    Fits(Vec<(ParsedFit, String)>),
    Shared(Vec<Group>),
    Error(String),
}

fn read(text: &str) -> Found {
    let t = text.trim();
    if t.is_empty() {
        return Found::Nothing;
    }
    if t.starts_with('{') {
        return match dc::import_json(t) {
            Ok(g) => Found::Shared(g),
            Err(e) => Found::Error(e),
        };
    }
    match dc::parse_paste(t) {
        Ok(f) => Found::Fits(f),
        Err(e) => Found::Error(e),
    }
}

/// The doctrine the fits go to: an existing one, or a new one in an existing group or a new group.
#[derive(Clone)]
enum Target {
    Existing(String),
    New { name: String, group: Result<String, String> },
}

struct Import {
    lib: Rc<Lib>,
    win: gtk::Window,
    buffer: gtk::TextBuffer,
    preview: gtk::Box,
    target_box: gtk::Box,
    progress: gtk::Box,
    bar: gtk::ProgressBar,
    bar_label: gtk::Label,
    result: gtk::Box,
    go: gtk::Button,
    cancel: gtk::Button,
    /// (doctrine id, label) choices; the dropdown's last entry is "New doctrine".
    doctrines: Vec<(String, String)>,
    groups: Vec<(String, String)>,
    target: gtk::DropDown,
    new_box: gtk::Box,
    new_name: gtk::Entry,
    group: Option<gtk::DropDown>,
    group_name: gtk::Entry,
    busy: Cell<bool>,
    closed: Cell<bool>,
    found: RefCell<Rc<Found>>,
    /// The doctrine an earlier try created (fits that failed then go to it too).
    created: RefCell<Option<String>>,
}

impl Import {
    fn target(&self) -> Option<Target> {
        if let Some(id) = &*self.created.borrow() {
            return Some(Target::Existing(id.clone()));
        }
        let i = self.target.selected() as usize;
        if let Some((id, _)) = self.doctrines.get(i) {
            return Some(Target::Existing(id.clone()));
        }
        let name = self.new_name.text().trim().to_string();
        if name.is_empty() {
            return None;
        }
        let group = match &self.group {
            Some(dd) => Ok(self.groups.get(dd.selected() as usize)?.0.clone()),
            None => {
                let g = self.group_name.text().trim().to_string();
                if g.is_empty() {
                    return None;
                }
                Err(g)
            }
        };
        Some(Target::New { name, group })
    }

    /// Enables the import button and shows the target fields for what was found.
    fn update(&self) {
        let found = self.found.borrow().clone();
        let new = self.target.selected() as usize >= self.doctrines.len();
        self.new_box.set_visible(new);
        self.target_box.set_visible(matches!(&*found, Found::Fits(_)) && self.created.borrow().is_none());
        let (label, ok) = match &*found {
            Found::Fits(f) => (format!("Add {}", plural(f.len(), "fit", "fits")), self.target().is_some()),
            Found::Shared(_) => ("Merge into library".to_string(), true),
            _ => ("Add fits".to_string(), false),
        };
        self.go.set_label(&label);
        self.go.set_sensitive(ok && !self.busy.get());
    }
}

/// Asks for a file: a PodStack doctrines file merges into the library (after a confirmation), fits open in the paste
/// dialog to pick their doctrine.
pub(super) fn open_file(lib: &Rc<Lib>, doctrine_id: Option<String>) {
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    let f = gtk::FileFilter::new();
    f.set_name(Some("Fits and PodStack doctrines"));
    for ext in ["json", "txt", "eft", "xml", "cfg"] {
        f.add_suffix(ext);
    }
    filters.append(&f);
    let all = gtk::FileFilter::new();
    all.set_name(Some("All files"));
    all.add_pattern("*");
    filters.append(&all);
    let d = gtk::FileDialog::new();
    d.set_title("Open fits or a doctrines file");
    d.set_modal(true);
    d.set_filters(Some(&filters));
    let l = lib.clone();
    d.open(Some(&lib.ctx.window()), gio::Cancellable::NONE, move |r| {
        let Ok(file) = r else { return };
        let Some(path) = file.path() else { return };
        let text = match std::fs::read(&path) {
            Ok(b) => String::from_utf8_lossy(&b).to_string(),
            Err(e) => {
                l.ctx.toast(&format!("Couldn't read the file: {e}"));
                return;
            }
        };
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        match read(&text) {
            Found::Shared(groups) => confirm_merge(&l, &name, groups),
            Found::Error(e) if text.trim().starts_with('{') => l.ctx.toast(&e),
            _ => paste_dialog(&l, doctrine_id.clone(), Some(text)),
        }
    });
}

/// "3 groups, 7 doctrines, 21 fits".
fn shared_summary(groups: &[Group]) -> String {
    let subs: usize = groups.iter().map(|g| g.walk().len()).sum();
    let doctrines: usize = groups.iter().map(|g| g.all_doctrines().len()).sum();
    let fits: usize = groups.iter().map(|g| g.fit_count()).sum();
    format!("{}, {}, {}", plural(subs, "group", "groups"), plural(doctrines, "doctrine", "doctrines"), plural(fits, "fit", "fits"))
}

fn confirm_merge(lib: &Rc<Lib>, file: &str, groups: Vec<Group>) {
    let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    let body = format!(
        "{file} holds {} ({}). A group or doctrine with the same name as one you have is merged into it, adding only the fits it doesn't have yet.",
        shared_summary(&groups),
        names.join(", ")
    );
    let l = lib.clone();
    let groups = RefCell::new(Some(groups));
    w::confirm(&lib.ctx, "Import doctrines", &body, "Import", false, move || {
        if let Some(g) = groups.borrow_mut().take() {
            merge(&l, g);
        }
    });
}

fn merge(lib: &Rc<Lib>, groups: Vec<Group>) {
    let first = groups.first().map(|g| g.name.clone());
    let added = lib.edit(|d| dc::merge_groups(d, groups));
    lib.ctx.toast(&match added {
        0 => "Nothing new: you have every fit already".to_string(),
        n => format!("Imported {}", plural(n, "fit", "fits")),
    });
    // show the (first) group it went into
    let doc = lib.doc();
    if let Some(g) = first.and_then(|n| doc.groups.iter().find(|g| g.name.eq_ignore_ascii_case(&n)).map(|g| g.id.clone())) {
        lib.select(Sel::Group(g));
    }
}

/// The paste dialog. `doctrine_id` is the doctrine to add to (else the one open, else a new one); `text` fills it.
pub(super) fn paste_dialog(lib: &Rc<Lib>, doctrine_id: Option<String>, text: Option<String>) {
    let doc = lib.doc();
    let (win, content, actions) = w::dialog(&lib.ctx, "Add fits", 640);
    win.set_default_height(660);
    let inner = w::vbox(12);
    let scroll = w::scrolled(&inner);
    scroll.set_min_content_height(400);
    content.append(&scroll);
    inner.append(&w::text(
        "Paste one fit or several: EFT from EVE's fitting window (Copy to clipboard) or Pyfa, EVE's XML export, or a DNA string. A PodStack doctrines file merges into the library.",
        &["body-medium", "dim"],
    ));

    // the text
    let buffer = gtk::TextBuffer::new(None);
    let view = gtk::TextView::with_buffer(&buffer);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.set_accepts_tab(false);
    let sw = gtk::ScrolledWindow::new();
    sw.set_child(Some(&view));
    sw.set_min_content_height(180);
    sw.set_max_content_height(300);
    sw.set_propagate_natural_height(true);
    let hint = w::text(PLACEHOLDER, &["doclib-placeholder"]);
    hint.set_valign(gtk::Align::Start);
    hint.set_can_target(false);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&sw));
    overlay.add_overlay(&hint);
    overlay.add_css_class("doclib-input-frame");
    overlay.set_overflow(gtk::Overflow::Hidden);
    inner.append(&overlay);
    let buttons = w::hbox(8);
    let paste = w::button_icon("content_paste", "Paste", Btn::Tonal);
    paste.set_tooltip_text(Some("Paste from the clipboard"));
    buttons.append(&paste);
    let open = w::button_icon("file_open", "Open file…", Btn::Tonal);
    buttons.append(&open);
    inner.append(&buttons);

    let preview = w::vbox(8);
    inner.append(&preview);

    // where the fits go
    let doctrines: Vec<(String, String)> =
        doc.all_doctrines().into_iter().map(|(p, d)| (d.id.clone(), format!("{} \u{203a} {}", p.join(" \u{203a} "), d.name))).collect();
    let groups: Vec<(String, String)> = doc.all_groups().into_iter().map(|(p, g)| (g.id.clone(), p.join(" \u{203a} "))).collect();
    let open_doctrine = match lib.selected() {
        Sel::Doctrine(id) => Some(id),
        _ => None,
    };
    let open_group = match lib.selected() {
        Sel::Group(id) => Some(id),
        Sel::Doctrine(id) => doc.doctrine(&id).map(|(_, g, _)| g.id.clone()),
        Sel::Library => None,
    };
    let wanted = doctrine_id.or(open_doctrine);
    let mut labels: Vec<String> = doctrines.iter().map(|d| d.1.clone()).collect();
    labels.push("New doctrine\u{2026}".into());
    let start = match &wanted {
        Some(id) => doctrines.iter().position(|d| d.0 == *id).unwrap_or(doctrines.len()),
        // nothing chosen: a new doctrine in the open group, or the first doctrine
        None if open_group.is_some() || doctrines.is_empty() => doctrines.len(),
        None => 0,
    };
    let target = shared::dropdown(&labels, start as u32);
    target.set_hexpand(true);
    let target_box = w::vbox(12);
    target_box.append(&labelled("Add to", &target));
    let new_box = w::vbox(12);
    let new_name = gtk::Entry::new();
    new_name.set_placeholder_text(Some("e.g. Home Defense"));
    new_box.append(&labelled("New doctrine's name", &new_name));
    let group_name = gtk::Entry::new();
    let group = if groups.is_empty() {
        group_name.set_placeholder_text(Some("e.g. your alliance"));
        new_box.append(&labelled("In a new group", &group_name));
        None
    } else {
        let labels: Vec<String> = groups.iter().map(|g| g.1.clone()).collect();
        let at = open_group.as_ref().and_then(|id| groups.iter().position(|g| g.0 == *id)).unwrap_or(0);
        let dd = shared::dropdown(&labels, at as u32);
        dd.set_hexpand(true);
        new_box.append(&labelled("In group", &dd));
        Some(dd)
    };
    target_box.append(&new_box);
    inner.append(&target_box);

    let progress = w::vbox(6);
    let bar = gtk::ProgressBar::new();
    let bar_label = w::line("", &["body-small", "dim"]);
    progress.append(&bar);
    progress.append(&bar_label);
    progress.set_visible(false);
    inner.append(&progress);
    let result = w::vbox(8);
    inner.append(&result);

    let cancel = w::button("Cancel", Btn::Text);
    let go = w::button("Add fits", Btn::Filled);
    actions.append(&cancel);
    actions.append(&go);

    let st = Rc::new(Import {
        lib: lib.clone(),
        win: win.clone(),
        buffer: buffer.clone(),
        preview,
        target_box,
        progress,
        bar,
        bar_label,
        result,
        go: go.clone(),
        cancel: cancel.clone(),
        doctrines,
        groups,
        target: target.clone(),
        new_box,
        new_name: new_name.clone(),
        group,
        group_name: group_name.clone(),
        busy: Cell::new(false),
        closed: Cell::new(false),
        found: RefCell::new(Rc::new(Found::Nothing)),
        created: RefCell::default(),
    });

    let s = st.clone();
    buffer.connect_changed(move |b| {
        hint.set_visible(b.char_count() == 0);
        show_preview(&s);
    });
    for e in [&new_name, &group_name] {
        let s = st.clone();
        e.connect_changed(move |_| s.update());
    }
    let s = st.clone();
    target.connect_selected_notify(move |_| s.update());
    if let Some(dd) = &st.group {
        let s = st.clone();
        dd.connect_selected_notify(move |_| s.update());
    }
    let s = st.clone();
    paste.connect_clicked(move |_| {
        let s = s.clone();
        s.lib.ctx.window().clipboard().read_text_async(gio::Cancellable::NONE, move |r| match r {
            Ok(Some(t)) if !t.trim().is_empty() => s.buffer.set_text(&t),
            _ => s.lib.ctx.toast("The clipboard has no text"),
        });
    });
    let s = st.clone();
    open.connect_clicked(move |_| open_into(&s));
    let wc = win.clone();
    cancel.connect_clicked(move |_| wc.close());
    let s = st.clone();
    win.connect_close_request(move |_| {
        s.closed.set(true);
        gtk::glib::Propagation::Proceed
    });
    let s = st.clone();
    go.connect_clicked(move |_| run(&s));

    buffer.set_text(text.as_deref().unwrap_or(""));
    show_preview(&st);
    win.present();
    view.grab_focus();
}

/// Opens a file into the paste dialog.
fn open_into(st: &Rc<Import>) {
    let d = gtk::FileDialog::new();
    d.set_title("Open fits");
    d.set_modal(true);
    let s = st.clone();
    d.open(Some(&st.win), gio::Cancellable::NONE, move |r| {
        let Ok(file) = r else { return };
        let Some(path) = file.path() else { return };
        match std::fs::read(&path) {
            Ok(b) => s.buffer.set_text(&String::from_utf8_lossy(&b)),
            Err(e) => s.lib.ctx.toast(&format!("Couldn't read the file: {e}")),
        }
    });
}

/// A parsed fit's ship id, when the name is a known hull (for its icon before it's looked up).
fn ship_id(f: &ParsedFit) -> Option<i64> {
    match &f.ship {
        podstack_core::features::fitting::ItemRef::TypeId(id) => Some(*id),
        podstack_core::features::fitting::ItemRef::Name(n) => SHIPS.by_id.values().find(|s| s.name.eq_ignore_ascii_case(n)).map(|s| s.type_id),
    }
}

fn show_preview(st: &Rc<Import>) {
    shared::clear(&st.preview);
    shared::clear(&st.result);
    let found = Rc::new(read(&buffer_text(&st.buffer)));
    match &*found {
        Found::Nothing => {}
        Found::Error(e) => st.preview.append(&w::banner("error_outline", e, BannerKind::Error)),
        Found::Shared(groups) => {
            let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
            st.preview.append(&w::banner(
                "folder_shared",
                &format!(
                    "A PodStack doctrines file: {} ({}). Groups and doctrines with the same name as yours are merged, adding only the fits you don't have.",
                    shared_summary(groups),
                    names.join(", ")
                ),
                BannerKind::Info,
            ));
        }
        Found::Fits(fits) => {
            st.preview.append(&w::line(&format!("Found {}", plural(fits.len(), "fit", "fits")), &["title-small"]));
            let list = w::vbox(2);
            list.add_css_class("doclib-preview");
            for (f, _) in fits {
                let row = w::hbox(12);
                row.set_margin_start(12);
                row.set_margin_end(12);
                row.set_margin_top(4);
                row.set_margin_bottom(4);
                match ship_id(f) {
                    Some(id) => row.append(&ship_icon(id, 32)),
                    None => {
                        let i = w::icon_sized("help_outline", 20);
                        i.set_size_request(32, 32);
                        row.append(&i);
                    }
                }
                let texts = w::vbox(0);
                texts.set_hexpand(true);
                let ship = dc::parsed_ship_label(f);
                texts.append(&w::line(f.fit_name.as_deref().filter(|n| !n.trim().is_empty()).unwrap_or(&ship), &["body-medium"]));
                texts.append(&w::line(&format!("{ship} \u{00b7} {} \u{00b7} {}", f.format, plural(f.items.len(), "item", "items")), &["body-small", "dim"]));
                row.append(&texts);
                list.append(&row);
            }
            st.preview.append(&list);
            // a new doctrine is named after the fits' header when they share one ("[Ishtar, Home Defense]")
            if st.new_name.text().trim().is_empty() {
                if let Some(n) = fits.first().and_then(|(f, _)| f.fit_name.clone()).filter(|n| fits.iter().all(|(f, _)| f.fit_name.as_deref() == Some(n.as_str()))) {
                    st.new_name.set_text(&n);
                }
            }
        }
    }
    *st.found.borrow_mut() = found;
    st.update();
}

fn set_busy(st: &Import, busy: bool) {
    st.busy.set(busy);
    st.progress.set_visible(busy);
    st.target_box.set_sensitive(!busy);
    st.update();
}

fn run(st: &Rc<Import>) {
    let found = st.found.borrow().clone();
    match &*found {
        Found::Shared(groups) => {
            merge(&st.lib, groups.clone());
            st.win.close();
        }
        Found::Fits(fits) => {
            let Some(target) = st.target() else { return };
            set_busy(st, true);
            shared::clear(&st.result);
            resolve_next(st.clone(), Rc::new(fits.clone()), 0, Vec::new(), Vec::new(), target);
        }
        _ => {}
    }
}

/// Looks the fits up one after the other (so the bar moves), then adds them.
fn resolve_next(st: Rc<Import>, fits: Rc<Vec<(ParsedFit, String)>>, i: usize, mut done: Vec<DoctrineFit>, failed: Vec<(String, String)>, target: Target) {
    if st.closed.get() {
        return;
    }
    let n = fits.len();
    if i >= n {
        finish(&st, done, failed, target);
        return;
    }
    st.bar.set_fraction(i as f64 / n as f64);
    st.bar_label.set_text(&format!("Looking up fit {} of {n}\u{2026}", i + 1));
    let (parsed, source) = fits[i].clone();
    let source_text = source.clone();
    let label = parsed.fit_name.clone().unwrap_or_else(|| dc::parsed_ship_label(&parsed));
    let core = st.lib.ctx.core().clone();
    rt::run(async move { dc::resolve(&core, &parsed, &source).await }, move |r| {
        let mut failed = failed;
        match r {
            Ok(f) => done.push(f),
            Err(e) => failed.push((format!("{label}: {e}"), source_text)),
        }
        resolve_next(st, fits, i + 1, done, failed, target);
    });
}

fn finish(st: &Rc<Import>, done: Vec<DoctrineFit>, failed: Vec<(String, String)>, target: Target) {
    st.bar.set_fraction(1.0);
    let found = done.len();
    let mut skipped = 0;
    let mut doctrine_id = None;
    let mut doctrine_name = String::new();
    if !done.is_empty() {
        let (added_to, name, dupes) = st.lib.edit(|doc| {
            let id = match &target {
                Target::Existing(id) => id.clone(),
                Target::New { name, group } => {
                    let d = Doctrine { id: dc::new_id(), name: name.clone(), notes: String::new(), fits: vec![] };
                    let id = d.id.clone();
                    match group {
                        Ok(gid) => {
                            if let Some(g) = doc.group_mut(gid) {
                                g.doctrines.push(d);
                            }
                        }
                        Err(new_group) => doc.groups.push(Group { id: dc::new_id(), name: new_group.clone(), doctrines: vec![d], ..Default::default() }),
                    }
                    id
                }
            };
            let mut dupes = 0;
            let mut name = String::new();
            if let Some(d) = doc.doctrine_mut(&id) {
                name = d.name.clone();
                for f in done {
                    if d.has_fit(&f.name, f.ship_id) {
                        dupes += 1;
                    } else {
                        d.fits.push(f);
                    }
                }
            }
            (id, name, dupes)
        });
        skipped = dupes;
        doctrine_id = Some(added_to);
        doctrine_name = name;
    }
    let added = found - skipped;
    if let Some(id) = &doctrine_id {
        st.lib.select(Sel::Doctrine(id.clone()));
        if matches!(target, Target::New { .. }) {
            *st.created.borrow_mut() = Some(id.clone());
        }
    }
    let mut msg = match added {
        0 if found > 0 => format!("{doctrine_name} already has {}", if found == 1 { "this fit" } else { "these fits" }),
        0 => String::new(),
        n => format!("Added {} to {doctrine_name}", plural(n, "fit", "fits")),
    };
    if added > 0 && skipped > 0 {
        msg.push_str(&format!(" ({skipped} already there)"));
    }
    if failed.is_empty() {
        if !msg.is_empty() {
            st.lib.ctx.toast(&msg);
        }
        st.win.close();
        return;
    }
    // some couldn't be read: keep only those in the text to fix and try again, and say why
    set_busy(st, false);
    let sources: Vec<&str> = failed.iter().map(|f| f.1.as_str()).filter(|s| !s.is_empty()).collect();
    if sources.len() == failed.len() {
        st.buffer.set_text(&sources.join("\n\n"));
    }
    if !msg.is_empty() {
        st.result.append(&w::banner("check_circle", &msg, BannerKind::Info));
    }
    let reasons: Vec<&str> = failed.iter().map(|f| f.0.as_str()).collect();
    st.result.append(&w::banner("error_outline", &format!("Couldn't add {}:\n{}", plural(failed.len(), "fit", "fits"), reasons.join("\n")), BannerKind::Error));
    st.cancel.set_label("Close");
}
