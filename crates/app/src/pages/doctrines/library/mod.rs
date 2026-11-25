//! The Doctrines tab: the library of groups (subgroups indented below them) and their doctrines in a tree on the
//! left, the chosen group or doctrine on the right (stacked in a narrow window).
//!
//! The page updates itself: it listens for the doctrines document and character changes and redraws the tree and
//! the open view in place, so the scroll position and the selection stay. Every edit loads the document, changes
//! it and saves it ([`Lib::edit`]), and the save's event redraws. The selection is remembered; for UI testing
//! `PODSTACK_DOCTRINE` opens a doctrine (or group) by name.
//!
//! - `group`: a group's view (and the whole library's), its subgroups and doctrines as cards;
//! - `doctrine`: a doctrine's view, its fits and who can fly what;
//! - `pilot`: one character against a doctrine (what's missing, the skill plan);
//! - `fit`: a fit's details dialog;
//! - `import`: pasting or opening fits and PodStack files;
//! - `dialogs`: naming, moving, deleting and saving files.

mod dialogs;
mod doctrine;
mod fit;
mod generate;
mod group;
mod import;
mod pilot;

use super::Tab;
use crate::ctx::{Ctx, Page};
use crate::rt;
use crate::widgets::{self as w, Btn};
use gtk::{glib, prelude::*};
use podstack_core::features::doctrines::{self as dc, DoctrineDoc, DoctrineFit, Matrix, Readiness, Tally};
use podstack_core::{Character, Event};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

const CSS: &str = "
.doclib-pane { background-color: @surface_container_low; border-radius: 24px; padding: 12px; }
list.doclib-tree { background-color: transparent; }
list.doclib-tree > row { border-radius: 999px; margin: 1px 0; padding: 0; }
list.doclib-tree > row:selected { background-color: @secondary_container; color: @on_secondary_container; }
.doclib-row { min-height: 40px; padding-right: 14px; }
.doclib-row .doclib-row-name { font-size: 14px; }
.doclib-row.group .doclib-row-name { font-weight: 500; }
.doclib-row .icon { color: @on_surface_variant; }
list.doclib-tree > row:selected .icon, list.doclib-tree > row:selected .doclib-count { color: @on_secondary_container; }
button.doclib-toggle { min-width: 28px; min-height: 28px; padding: 0; border-radius: 999px; background: none; box-shadow: none; border: none; }
button.doclib-toggle:hover { background-color: alpha(@on_surface, 0.08); }
.doclib-count { font-size: 12px; color: @on_surface_variant; }
.doclib-count.has { color: @success; font-weight: 500; }
.doclib-hero { background-color: @surface_container_low; border-radius: 28px; padding: 24px; }
.doclib-hero-icon { background-color: @primary_container; color: @on_primary_container; border-radius: 16px; min-width: 56px; min-height: 56px; }
.doclib-crumbs { color: @on_surface_variant; font-size: 13px; }
.doclib-notes { color: @on_surface_variant; }
button.doclib-card { background-color: @surface_container_low; border-radius: 20px; padding: 16px; border: none; color: @on_surface;
  box-shadow: 0 1px 2px alpha(@shadow, 0.30), 0 1px 3px 1px alpha(@shadow, 0.15); }
button.doclib-card:hover { background-color: mix(@surface_container_low, @on_surface, 0.06); }
.doclib-card-icon { background-color: @secondary_container; color: @on_secondary_container; border-radius: 12px; min-width: 40px; min-height: 40px; }
.doclib-ship { border-radius: 8px; background-color: @surface_container_highest; }
.doclib-ship-large { border-radius: 20px; background-color: @surface_container_highest; }
.doclib-head { color: @on_surface_variant; font-size: 12px; font-weight: 500; letter-spacing: 0.4px; }
button.doclib-cell, button.doclib-pilot, button.doclib-fit { background: none; box-shadow: none; border: none; color: @on_surface; }
button.doclib-cell { padding: 2px 4px; min-height: 0; border-radius: 10px; }
button.doclib-pilot { padding: 4px 8px 4px 4px; border-radius: 999px; }
button.doclib-fit { padding: 8px 12px; border-radius: 16px; }
button.doclib-cell:hover, button.doclib-pilot:hover, button.doclib-fit:hover { background-color: alpha(@on_surface, 0.08); }
.doclib-input-frame { border-radius: 12px; background-color: @surface_container_highest; }
.doclib-input-frame textview, .doclib-input-frame textview text { background-color: transparent; font-family: monospace; font-size: 13px; }
.doclib-input-frame textview { padding: 12px; }
.doclib-placeholder { color: @on_surface_variant; font-family: monospace; font-size: 13px; padding: 12px; }
popover button.doclib-menu-item { border-radius: 0; min-height: 40px; padding: 0 16px; color: @on_surface; font-weight: 400; background: none; box-shadow: none; border: none; }
popover button.doclib-menu-item:hover { background-color: alpha(@on_surface, 0.08); }
popover button.doclib-menu-item.danger { color: @error; }
popover button.doclib-menu-item:disabled { color: alpha(@on_surface, 0.38); }
.doclib-role { background-color: @tertiary_container; color: @on_tertiary_container; border-radius: 8px; padding: 2px 8px; font-size: 12px; font-weight: 500; }
.doclib-section { margin-top: 8px; }
.doclib-preview { background-color: @surface_container; border-radius: 12px; padding: 4px 0; }
menubutton.doclib-menu > button { background-color: @secondary_container; color: @on_secondary_container; padding: 0 12px 0 16px; }
menubutton.doclib-menu > button:hover { background-color: mix(@secondary_container, @on_secondary_container, 0.08); }
menubutton.doclib-menu > button:checked { background-color: mix(@secondary_container, @on_secondary_container, 0.12); }
menubutton.doclib-icon-menu > button { min-width: 40px; min-height: 40px; padding: 0; color: @on_surface_variant; }
menubutton.doclib-icon-menu > button:hover { background-color: alpha(@on_surface_variant, 0.08); }
menubutton.doclib-icon-menu > button:checked { background-color: alpha(@on_surface_variant, 0.12); }
";

const SELECTED_KEY: &str = "doctrines.library.selected";
const COLLAPSED_KEY: &str = "doctrines.library.collapsed";
const SEARCH_KEY: &str = "doctrines.library.search";

/// What the right side shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sel {
    /// The whole library.
    Library,
    Group(String),
    Doctrine(String),
}

impl Sel {
    fn key(&self) -> String {
        match self {
            Sel::Library => "library".into(),
            Sel::Group(id) => format!("g:{id}"),
            Sel::Doctrine(id) => format!("d:{id}"),
        }
    }

    fn parse(s: &str) -> Option<Sel> {
        match s.split_once(':') {
            Some(("g", id)) => Some(Sel::Group(id.into())),
            Some(("d", id)) => Some(Sel::Doctrine(id.into())),
            _ if s == "library" => Some(Sel::Library),
            _ => None,
        }
    }

    /// Whether it still exists in `doc`.
    fn exists(&self, doc: &DoctrineDoc) -> bool {
        match self {
            Sel::Library => true,
            Sel::Group(id) => doc.group(id).is_some(),
            Sel::Doctrine(id) => doc.doctrine(id).is_some(),
        }
    }
}

/// Every character against every fit in the library, worked out once per change.
///
/// Checking is a full skill plan per character and fit, too slow for the GTK thread in a big library: it runs in the
/// background ([`Lib::recheck`]) while the page shows the previous result.
pub(crate) struct Checks {
    pub matrix: Matrix,
    /// Characters whose skills have loaded, most SP first.
    pub pilots: Vec<Arc<Character>>,
    pub ids: Vec<i64>,
    /// Characters whose skills haven't loaded yet.
    pub loading: usize,
    /// False until the first check is done.
    pub ready: bool,
}

thread_local! {
    /// The last checks, so coming back to the page shows them at once (and redraws only if they changed).
    static LAST: RefCell<Option<Rc<Checks>>> = const { RefCell::new(None) };
}

impl Checks {
    fn pending() -> Checks {
        Checks { matrix: Matrix::new(), pilots: vec![], ids: vec![], loading: 0, ready: false }
    }

    fn compute(fits: &[DoctrineFit], mut all: Vec<Arc<Character>>) -> Checks {
        all.sort_by_key(|c| std::cmp::Reverse(c.total_sp()));
        let loading = all.iter().filter(|c| c.skills.is_none()).count();
        let pilots: Vec<Arc<Character>> = all.into_iter().filter(|c| c.skills.is_some()).collect();
        let fits: Vec<&DoctrineFit> = fits.iter().collect();
        let matrix = dc::matrix(&pilots, &fits);
        let ids = pilots.iter().map(|c| c.id).collect();
        Checks { matrix, pilots, ids, loading, ready: true }
    }

    /// Whether `other` would show the same.
    fn same(&self, other: &Checks) -> bool {
        self.ready == other.ready
            && self.ids == other.ids
            && self.loading == other.loading
            && self.matrix == other.matrix
            && self.pilots.iter().zip(&other.pilots).all(|(a, b)| a.name() == b.name())
    }

    /// A count for a tile: "…" until the first check is done.
    pub fn count(&self, n: usize) -> String {
        if self.ready {
            n.to_string()
        } else {
            "\u{2026}".into()
        }
    }

    pub fn tally(&self, fits: &[&DoctrineFit]) -> Tally {
        dc::tally(&self.matrix, &self.ids, fits)
    }

    /// How many characters can fly a fit as it is.
    pub fn fit_ready(&self, fit_id: &str) -> usize {
        self.ids.iter().filter(|id| self.check(**id, fit_id).is_some_and(|c| c.readiness == Readiness::Ready)).count()
    }

    pub fn check(&self, character_id: i64, fit_id: &str) -> Option<&dc::FitCheck> {
        self.matrix.get(&(character_id, fit_id.to_string()))
    }
}

/// The tab's state, shared by its views and dialogs.
pub(crate) struct Lib {
    pub ctx: Ctx,
    sel: RefCell<Sel>,
    collapsed: RefCell<HashSet<String>>,
    search: RefCell<String>,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    keys: RefCell<Vec<Sel>>,
    detail: gtk::Box,
    checks: RefCell<Rc<Checks>>,
    /// Bumped by every new check, so a slow earlier one doesn't overwrite a newer result.
    generation: Cell<u64>,
    /// A recheck is already waiting (character updates come in bursts).
    pending: Cell<bool>,
    was_empty: bool,
}

impl Lib {
    pub fn doc(&self) -> DoctrineDoc {
        dc::load()
    }

    /// Changes the document and saves it; the save's event redraws the page.
    pub fn edit<R>(&self, f: impl FnOnce(&mut DoctrineDoc) -> R) -> R {
        let mut d = dc::load();
        let r = f(&mut d);
        dc::save(self.ctx.core(), &d);
        r
    }

    /// The latest checks (until a recheck finishes, from before the last change).
    pub fn checks(&self) -> Rc<Checks> {
        self.checks.borrow().clone()
    }

    /// Checks every character against every fit again, in the background, and redraws if anything changed.
    fn recheck(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let fits: Vec<DoctrineFit> = self.doc().all_fits().into_iter().map(|(_, _, f)| f.clone()).collect();
        let chars = self.ctx.core().characters();
        let weak = Rc::downgrade(self);
        rt::run(async move { tokio::task::spawn_blocking(move || Checks::compute(&fits, chars)).await.ok() }, move |c| {
            let (Some(s), Some(c)) = (weak.upgrade(), c) else { return };
            if s.generation.get() != generation {
                return;
            }
            let same = s.checks.borrow().same(&c);
            let c = Rc::new(c);
            LAST.with(|l| *l.borrow_mut() = Some(c.clone()));
            *s.checks.borrow_mut() = c;
            if !same {
                render_list(&s);
                render_detail(&s, true);
            }
        });
    }

    pub fn selected(&self) -> Sel {
        self.sel.borrow().clone()
    }

    /// Shows a group, doctrine or the library, from the top.
    pub fn select(self: &Rc<Self>, sel: Sel) {
        // a doctrine or group inside a collapsed group gets its ancestors opened
        let doc = self.doc();
        let inner = match &sel {
            Sel::Group(id) => doc.parent_of(id).map(|g| g.id.clone()),
            Sel::Doctrine(id) => doc.doctrine(id).map(|(_, g, _)| g.id.clone()),
            Sel::Library => None,
        };
        if let Some(mut gid) = inner {
            let mut collapsed = self.collapsed.borrow_mut();
            loop {
                collapsed.remove(&gid);
                match doc.parent_of(&gid) {
                    Some(p) => gid = p.id.clone(),
                    None => break,
                }
            }
        }
        self.remember_collapsed();
        self.ctx.remember(SELECTED_KEY, sel.key());
        *self.sel.borrow_mut() = sel;
        // once the click that got here (a card, a tree row, a menu item) is handled: the redraw replaces its widget
        let s = self.clone();
        glib::idle_add_local_once(move || {
            render_list(&s);
            render_detail(&s, false);
        });
    }

    fn remember_collapsed(&self) {
        let v: Vec<String> = self.collapsed.borrow().iter().cloned().collect();
        self.ctx.remember(COLLAPSED_KEY, v.join(","));
    }

    fn toggle(self: &Rc<Self>, group_id: &str) {
        {
            let mut c = self.collapsed.borrow_mut();
            if !c.remove(group_id) {
                c.insert(group_id.to_string());
            }
        }
        self.remember_collapsed();
        render_list(self);
    }

    /// Redraws everything after the document changed, keeping the scroll positions, and checks again.
    fn refresh(self: &Rc<Self>) {
        let doc = self.doc();
        if doc.groups.is_empty() != self.was_empty {
            // between the first-run welcome and the library: build the page again
            let ctx = self.ctx.clone();
            glib::idle_add_local_once(move || ctx.rebuild());
            return;
        }
        let sel = self.selected();
        if !sel.exists(&doc) {
            *self.sel.borrow_mut() = Sel::Library;
            self.ctx.remember(SELECTED_KEY, Sel::Library.key());
        }
        render_list(self);
        render_detail(self, true);
        self.recheck();
    }

    /// Checks again a moment after characters change (their updates come in bursts).
    fn schedule_recheck(self: &Rc<Self>) {
        if self.pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
            if let Some(s) = weak.upgrade() {
                s.pending.set(false);
                s.recheck();
            }
        });
    }
}

/// The selection to start with: the one shown last, or the doctrine `PODSTACK_DOCTRINE` names (for UI testing).
fn initial_selection(ctx: &Ctx, doc: &DoctrineDoc) -> Sel {
    if let Some(s) = ctx.recall(SELECTED_KEY).and_then(|s| Sel::parse(&s)).filter(|s| s.exists(doc)) {
        return s;
    }
    if let Ok(name) = std::env::var("PODSTACK_DOCTRINE") {
        let name = name.trim();
        if let Some((_, d)) = doc.all_doctrines().into_iter().find(|(_, d)| d.name.eq_ignore_ascii_case(name)) {
            return Sel::Doctrine(d.id.clone());
        }
        if let Some((_, g)) = doc.all_groups().into_iter().find(|(_, g)| g.name.eq_ignore_ascii_case(name)) {
            return Sel::Group(g.id.clone());
        }
    }
    Sel::Library
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let doc = dc::load();

    let root = w::vbox(0);
    root.set_vexpand(true);
    // the tabs and the library's actions: side by side, the actions on a line of their own in a narrow window
    let header = gtk::FlowBox::new();
    header.set_selection_mode(gtk::SelectionMode::None);
    header.set_homogeneous(false);
    header.set_column_spacing(12);
    header.set_row_spacing(8);
    header.set_max_children_per_line(2);
    header.set_margin_start(24);
    header.set_margin_end(24);
    header.set_margin_top(8);
    header.append(&super::tab_bar(ctx, Tab::Doctrines));
    let actions = w::hbox(12);
    actions.set_halign(gtk::Align::End);
    actions.set_hexpand(true);
    header.append(&actions);
    root.append(&header);

    let list = gtk::ListBox::new();
    list.add_css_class("doclib-tree");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(true);
    let scroller = w::scrolled(&list);
    let collapsed: HashSet<String> = ctx.recall(COLLAPSED_KEY).unwrap_or_default().split(',').filter(|s| !s.is_empty()).map(String::from).collect();
    let lib = Rc::new(Lib {
        ctx: ctx.clone(),
        sel: RefCell::new(initial_selection(ctx, &doc)),
        collapsed: RefCell::new(collapsed),
        search: RefCell::new(ctx.recall(SEARCH_KEY).unwrap_or_default()),
        list,
        scroller,
        keys: RefCell::default(),
        detail: w::vbox(16),
        checks: RefCell::new(LAST.with(|l| l.borrow().clone()).unwrap_or_else(|| Rc::new(Checks::pending()))),
        generation: Cell::new(0),
        pending: Cell::new(false),
        was_empty: doc.groups.is_empty(),
    });

    let body: gtk::Widget = if doc.groups.is_empty() {
        welcome(&lib).upcast()
    } else {
        actions.append(&library_menu(&lib));
        let add = w::button_icon("create_new_folder", "New group", Btn::Tonal);
        add.set_tooltip_text(Some("A group of people you fly with: an alliance, a corporation, a fleet community"));
        let l = lib.clone();
        add.connect_clicked(move |_| dialogs::new_group(&l, None));
        actions.append(&add);

        let pane = w::vbox(8);
        pane.add_css_class("doclib-pane");
        let search = w::search_entry("Filter doctrines and fits");
        search.set_text(&lib.search.borrow());
        let l = lib.clone();
        search.connect_search_changed(move |e| {
            *l.search.borrow_mut() = e.text().to_string();
            l.ctx.remember(SEARCH_KEY, e.text().to_string());
            render_list(&l);
        });
        pane.append(&search);
        pane.append(&lib.scroller);
        let l = lib.clone();
        lib.list.connect_row_activated(move |_, row| {
            let sel = l.keys.borrow().get(row.index() as usize).cloned();
            if let Some(sel) = sel {
                if sel != l.selected() {
                    l.select(sel);
                }
            }
        });

        lib.detail.set_margin_bottom(8);
        let body = crate::pages::skill_browser::shared::two_pane(ctx, &pane, &lib.detail);
        render_list(&lib);
        render_detail(&lib, false);
        body.upcast()
    };
    root.append(&body);

    let l = lib.clone();
    ctx.on_event(&root, move |e| match e {
        Event::Doc(d) if d == dc::DOC => l.refresh(),
        Event::CharacterUpdated(_) | Event::CharactersChanged => l.schedule_recheck(),
        _ => {}
    });
    if !lib.was_empty {
        lib.recheck();
    }

    Page::new("Doctrines", root).no_scroll().static_page()
}

/// First run: what the tab is for and the two ways to start.
fn welcome(lib: &Rc<Lib>) -> gtk::Box {
    let body = w::vbox(24);
    body.set_vexpand(true);
    body.set_valign(gtk::Align::Center);
    body.set_halign(gtk::Align::Center);
    body.set_margin_start(24);
    body.set_margin_end(24);
    body.set_margin_bottom(48);
    body.append(&w::empty_state(
        "military_tech",
        "Build your doctrine library",
        "Make a group for each alliance, corporation or community you fly with (and, if you like, subgroups for its SIGs such as Capitals or Incursions). Give each group its doctrines and paste the fits they accept: PodStack shows which of your characters can fly what, and the skill plan for the rest.",
    ));
    let actions = w::hbox(12);
    actions.set_halign(gtk::Align::Center);
    let create = w::button_icon("create_new_folder", "Create a group", Btn::Filled);
    let l = lib.clone();
    create.connect_clicked(move |_| dialogs::new_group(&l, None));
    actions.append(&create);
    let open = w::button_icon("file_open", "Import a shared file", Btn::Tonal);
    open.set_tooltip_text(Some("A PodStack doctrines file (.json) from a corp mate, or fits (EFT .txt, EVE .xml)"));
    let l = lib.clone();
    open.connect_clicked(move |_| import::open_file(&l, None));
    actions.append(&open);
    let paste = w::button_icon("content_paste", "Paste fits", Btn::Outlined);
    let l = lib.clone();
    paste.connect_clicked(move |_| import::paste_dialog(&l, None, None));
    actions.append(&paste);
    body.append(&actions);
    let hint = w::text("Fits come from EVE's fitting window (Copy to clipboard), Pyfa or any EFT text; EVE's XML export and DNA links work too.", &["body-small", "dim"]);
    hint.set_xalign(0.5);
    hint.set_justify(gtk::Justification::Center);
    body.append(&hint);
    body
}

/// Import and export for the whole library.
fn library_menu(lib: &Rc<Lib>) -> gtk::MenuButton {
    let (a, b, c, d, e) = (lib.clone(), lib.clone(), lib.clone(), lib.clone(), lib.clone());
    menu_button(
        Some("import_export"),
        Some("Import / export"),
        "Bring in fits or shared files, or share your library",
        vec![
            MenuItem::new("content_paste", "Paste fits…", move || import::paste_dialog(&a, None, None)),
            MenuItem::new("file_open", "Open a file…", move || import::open_file(&b, None)),
            MenuItem::new("auto_awesome", "Doctrines from in-game fittings…", move || generate::open(&e)),
            MenuItem::Separator,
            MenuItem::new("save_alt", "Save the whole library…", move || {
                let doc = c.doc();
                dialogs::save_json(&c, "podstack-doctrines.json", &doc.groups);
            }),
            MenuItem::new("content_copy", "Copy every fit as EFT", move || {
                let doc = d.doc();
                let fits: Vec<DoctrineFit> = doc.all_fits().into_iter().map(|(_, _, f)| f.clone()).collect();
                copy_eft(&d.ctx, &fits);
            }),
        ],
    )
}

pub(crate) fn copy_eft(ctx: &Ctx, fits: &[DoctrineFit]) {
    if fits.is_empty() {
        ctx.toast("No fits to copy");
        return;
    }
    ctx.window().clipboard().set_text(&dc::export_eft(fits));
    ctx.toast(&match fits.len() {
        1 => "Copied the fit as EFT".to_string(),
        n => format!("Copied {n} fits as EFT"),
    });
}

// ---------- the tree ----------

/// Whether a search matches a doctrine (its name, or a fit's name, hull or role).
fn doctrine_matches(d: &dc::Doctrine, q: &str) -> bool {
    d.name.to_lowercase().contains(q)
        || d.fits.iter().any(|f| [&f.name, &f.ship_name, &f.role].iter().any(|t| t.to_lowercase().contains(q)))
}

/// Whether a search matches anything in a group.
fn group_matches(g: &dc::Group, q: &str) -> bool {
    g.name.to_lowercase().contains(q) || g.doctrines.iter().any(|d| doctrine_matches(d, q)) || g.groups.iter().any(|s| group_matches(s, q))
}

fn render_list(lib: &Rc<Lib>) {
    let doc = lib.doc();
    let checks = lib.checks();
    let query = lib.search.borrow().trim().to_lowercase();
    let selected = lib.selected();
    let adj = lib.scroller.vadjustment();
    let pos = adj.value();
    lib.list.remove_all();
    let mut keys = Vec::new();
    let mut select_row = None;

    let mut add = |sel: Sel, content: gtk::Widget, tooltip: Option<String>| {
        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&content));
        row.set_tooltip_text(tooltip.as_deref());
        lib.list.append(&row);
        if sel == selected {
            select_row = Some(row);
        }
        keys.push(sel);
    };

    let all: Vec<&DoctrineFit> = doc.all_fits().into_iter().map(|(_, _, f)| f).collect();
    add(
        Sel::Library,
        tree_row(lib, 0, None, "library_books", "All doctrines", true, counts(all.len(), None)),
        Some(format!("{} groups \u{00b7} {} doctrines \u{00b7} {} fits", doc.all_groups().len(), doc.all_doctrines().len(), all.len())),
    );

    fn walk(lib: &Rc<Lib>, g: &dc::Group, depth: i32, query: &str, checks: &Checks, add: &mut dyn FnMut(Sel, gtk::Widget, Option<String>)) {
        if !query.is_empty() && !group_matches(g, query) {
            return;
        }
        // a group whose name matches shows everything in it
        let whole = query.is_empty() || g.name.to_lowercase().contains(query);
        let open = !query.is_empty() || !lib.collapsed.borrow().contains(&g.id);
        let has_children = !g.groups.is_empty() || !g.doctrines.is_empty();
        let fits: Vec<&DoctrineFit> = g.all_doctrines().into_iter().flat_map(|d| d.fits.iter()).collect();
        let t = checks.tally(&fits);
        let toggle = has_children.then_some((g.id.clone(), open));
        add(
            Sel::Group(g.id.clone()),
            tree_row(lib, depth, toggle, if depth == 0 { "groups" } else { "folder" }, &g.name, true, counts(fits.len(), None)),
            Some(format!(
                "{} doctrines \u{00b7} {} fits \u{00b7} {} of {} characters can fly at least one",
                g.all_doctrines().len(),
                fits.len(),
                t.ready,
                checks.ids.len()
            )),
        );
        if !open {
            return;
        }
        for d in &g.doctrines {
            if !whole && !doctrine_matches(d, query) {
                continue;
            }
            let fits: Vec<&DoctrineFit> = d.fits.iter().collect();
            let t = checks.tally(&fits);
            add(
                Sel::Doctrine(d.id.clone()),
                tree_row(lib, depth + 1, None, "shield", &d.name, false, counts(d.fits.len(), checks.ready.then_some(t.ready))),
                Some(format!("{} fits \u{00b7} {} of {} characters can fly it", d.fits.len(), t.ready, checks.ids.len())),
            );
        }
        // a group's own doctrines first, then its subgroups, as the group view lists them
        for s in &g.groups {
            walk(lib, s, depth + 1, if whole { "" } else { query }, checks, add);
        }
    }
    for g in &doc.groups {
        walk(lib, g, 0, &query, &checks, &mut add);
    }
    let found = keys.len();
    *lib.keys.borrow_mut() = keys;
    if !query.is_empty() && found <= 1 {
        let row = gtk::ListBoxRow::new();
        row.set_selectable(false);
        row.set_activatable(false);
        let l = w::text("Nothing matches.", &["body-medium", "dim"]);
        l.set_margin_start(16);
        l.set_margin_top(12);
        row.set_child(Some(&l));
        lib.list.append(&row);
    }
    if let Some(r) = select_row {
        lib.list.select_row(Some(&r));
    }
    glib::idle_add_local_once(move || adj.set_value(pos));
}

/// The trailing counts of a tree row: fits, and (for doctrines) characters who can fly it.
fn counts(fits: usize, pilots: Option<usize>) -> gtk::Widget {
    let b = w::hbox(10);
    let item = |icon: &str, n: usize, has: bool| {
        let i = w::hbox(3);
        let ic = w::icon_sized(icon, 18);
        ic.add_css_class("doclib-count");
        let l = gtk::Label::new(Some(&n.to_string()));
        for c in ["doclib-count", "numeric"] {
            l.add_css_class(c);
        }
        if has {
            ic.add_css_class("has");
            l.add_css_class("has");
        }
        i.append(&ic);
        i.append(&l);
        i
    };
    b.append(&item("rocket_launch", fits, false));
    if let Some(p) = pilots {
        b.append(&item("person", p, p > 0));
    }
    b.upcast()
}

fn tree_row(lib: &Rc<Lib>, depth: i32, toggle: Option<(String, bool)>, icon: &str, name: &str, group: bool, trailing: gtk::Widget) -> gtk::Widget {
    let b = w::hbox(6);
    b.add_css_class("doclib-row");
    if group {
        b.add_css_class("group");
    }
    b.set_margin_start(4 + depth * 18);
    match toggle {
        Some((id, open)) => {
            let t = gtk::Button::new();
            t.set_child(Some(&w::icon_sized(if open { "expand_more" } else { "chevron_right" }, 20)));
            t.add_css_class("doclib-toggle");
            t.set_valign(gtk::Align::Center);
            t.set_tooltip_text(Some(if open { "Collapse" } else { "Expand" }));
            let l = lib.clone();
            t.connect_clicked(move |_| {
                let (l, id) = (l.clone(), id.clone());
                // after the click: the redraw removes this button
                glib::idle_add_local_once(move || l.toggle(&id));
            });
            b.append(&t);
        }
        None => {
            let s = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            s.set_size_request(28, -1);
            b.append(&s);
        }
    }
    let i = w::icon_sized(icon, 20);
    i.set_valign(gtk::Align::Center);
    b.append(&i);
    let n = w::line(name, &["doclib-row-name"]);
    n.set_hexpand(true);
    n.set_margin_start(4);
    b.append(&n);
    trailing.set_valign(gtk::Align::Center);
    b.append(&trailing);
    b.upcast()
}

// ---------- the detail ----------

fn render_detail(lib: &Rc<Lib>, keep_scroll: bool) {
    let scroll = lib.detail.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>();
    let pos = scroll.as_ref().map(|s| s.vadjustment().value()).unwrap_or(0.0);
    while let Some(c) = lib.detail.first_child() {
        lib.detail.remove(&c);
    }
    let doc = lib.doc();
    match lib.selected() {
        Sel::Library => group::library(lib, &doc),
        Sel::Group(id) => match doc.group(&id) {
            Some(g) => group::view(lib, &doc, g),
            None => group::library(lib, &doc),
        },
        Sel::Doctrine(id) => match doc.doctrine(&id) {
            Some((path, _, d)) => doctrine::view(lib, &path, d),
            None => group::library(lib, &doc),
        },
    }
    if let Some(s) = scroll {
        let adj = s.vadjustment();
        let to = if keep_scroll { pos } else { 0.0 };
        glib::idle_add_local_once(move || adj.set_value(to));
    }
}

/// The top of a view: icon, where it is, its name, notes and actions.
pub(crate) fn hero(icon: &str, crumbs: &str, title: &str, notes: &str, actions: &[gtk::Widget]) -> gtk::Box {
    let hero = w::vbox(16);
    hero.add_css_class("doclib-hero");
    let top = w::hbox(20);
    let i = w::icon_sized(icon, 36);
    i.add_css_class("doclib-hero-icon");
    i.set_valign(gtk::Align::Start);
    top.append(&i);
    let texts = w::vbox(4);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    if !crumbs.is_empty() {
        texts.append(&w::line(crumbs, &["doclib-crumbs"]));
    }
    let t = w::text(title, &["headline-small"]);
    t.set_selectable(true);
    texts.append(&t);
    if !notes.trim().is_empty() {
        let n = w::text(notes.trim(), &["body-medium", "doclib-notes"]);
        n.set_selectable(true);
        n.set_margin_top(4);
        texts.append(&n);
    }
    top.append(&texts);
    hero.append(&top);
    if !actions.is_empty() {
        let row = gtk::FlowBox::new();
        row.set_selection_mode(gtk::SelectionMode::None);
        row.set_column_spacing(8);
        row.set_row_spacing(8);
        row.set_max_children_per_line(12);
        row.set_halign(gtk::Align::Start);
        for a in actions {
            row.append(a);
        }
        hero.append(&row);
    }
    hero
}

/// A section heading in a view.
pub(crate) fn section(title: &str, subtitle: Option<&str>) -> gtk::Box {
    let b = w::vbox(2);
    b.add_css_class("doclib-section");
    b.append(&w::text(title, &["title-medium"]));
    if let Some(s) = subtitle {
        b.append(&w::text(s, &["body-medium", "dim"]));
    }
    b
}

/// "1 fit", "3 fits".
pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// A ship's icon on a rounded tile.
pub(crate) fn ship_icon(ship_id: i64, size: i32) -> gtk::Widget {
    let f = gtk::Box::new(gtk::Orientation::Vertical, 0);
    f.add_css_class("doclib-ship");
    f.set_overflow(gtk::Overflow::Hidden);
    f.set_valign(gtk::Align::Center);
    f.set_halign(gtk::Align::Start);
    f.append(&w::eve_image(&podstack_core::config::type_icon(ship_id, 64), size));
    f.upcast()
}

// ---------- menus ----------

pub(crate) enum MenuItem {
    Item { icon: &'static str, label: String, danger: bool, enabled: bool, action: Box<dyn Fn()> },
    Separator,
}

impl MenuItem {
    pub fn new(icon: &'static str, label: &str, action: impl Fn() + 'static) -> MenuItem {
        MenuItem::Item { icon, label: label.into(), danger: false, enabled: true, action: Box::new(action) }
    }
    pub fn danger(icon: &'static str, label: &str, action: impl Fn() + 'static) -> MenuItem {
        MenuItem::Item { icon, label: label.into(), danger: true, enabled: true, action: Box::new(action) }
    }
    /// Greyed out unless `on`.
    pub fn enabled(mut self, on: bool) -> MenuItem {
        if let MenuItem::Item { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }
}

/// A button opening a menu: with a label (and optional icon) it's a text button with a drop-down arrow, without one
/// an icon button.
pub(crate) fn menu_button(icon: Option<&str>, label: Option<&str>, tooltip: &str, items: Vec<MenuItem>) -> gtk::MenuButton {
    let mb = gtk::MenuButton::new();
    match label {
        Some(label) => {
            let row = w::hbox(8);
            if let Some(i) = icon {
                row.append(&w::icon_sized(i, 18));
            }
            row.append(&gtk::Label::new(Some(label)));
            row.append(&w::icon_sized("arrow_drop_down", 18));
            mb.set_child(Some(&row));
            mb.add_css_class("doclib-menu");
        }
        None => {
            mb.set_child(Some(&w::icon(icon.unwrap_or("more_vert"))));
            mb.add_css_class("doclib-icon-menu");
        }
    }
    mb.set_valign(gtk::Align::Center);
    mb.set_tooltip_text(Some(tooltip));
    let pop = gtk::Popover::new();
    pop.set_has_arrow(false);
    let bx = w::vbox(0);
    bx.set_margin_top(4);
    bx.set_margin_bottom(4);
    for item in items {
        match item {
            MenuItem::Separator => {
                let s = gtk::Separator::new(gtk::Orientation::Horizontal);
                s.set_margin_top(4);
                s.set_margin_bottom(4);
                bx.append(&s);
            }
            MenuItem::Item { icon, label, danger, enabled, action } => {
                let b = gtk::Button::new();
                let row = w::hbox(12);
                row.append(&w::icon_sized(icon, 20));
                let l = gtk::Label::new(Some(&label));
                l.set_xalign(0.0);
                l.set_hexpand(true);
                row.append(&l);
                b.set_child(Some(&row));
                b.add_css_class("doclib-menu-item");
                if danger {
                    b.add_css_class("danger");
                }
                b.set_sensitive(enabled);
                let popw = pop.downgrade();
                b.connect_clicked(move |_| {
                    if let Some(p) = popw.upgrade() {
                        p.popdown();
                    }
                    action();
                });
                bx.append(&b);
            }
        }
    }
    pop.set_child(Some(&bx));
    mb.set_popover(Some(&pop));
    mb
}
