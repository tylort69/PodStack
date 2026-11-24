//! The stock targets dialog: how many ships of each doctrine fit to keep, for everyone or for one character, set for
//! everything, per doctrine or per fit. Each place shows the target that applies there and where it comes from; a
//! value set there can be cleared to inherit again. Changes save at once and the page behind updates itself.

use crate::ctx::Ctx;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::doctrine_inventory::{self as inv, Level, Source};
use podstack_core::features::doctrines as dc;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const CSS: &str = "
.dst-list { padding-right: 8px; }
.dst-doctrine { margin-top: 12px; }
.dst-fit { margin-left: 24px; }
.dst-spin { min-width: 96px; }
";

/// Where a target comes from, as the page and dialog say it.
pub(super) fn source_label(src: Source) -> String {
    let level = |l: Level| match l {
        Level::Overall => "overall",
        Level::Doctrine => "doctrine",
        Level::Fit => "fit",
    };
    match src {
        Source::Default => "default".into(),
        Source::Everyone(l) => format!("everyone's {}", level(l)),
        Source::Character(l) => format!("this pilot's {}", level(l)),
    }
}

/// One place a target can be set: everything (no doctrine), a doctrine (no fit) or a fit.
struct Row {
    doctrine: Option<String>,
    fit: Option<String>,
    spin: gtk::SpinButton,
    from: gtk::Label,
    reset: gtk::Button,
}

struct State {
    ctx: Ctx,
    /// None edits everyone's targets.
    scope: Cell<Option<i64>>,
    rows: RefCell<Vec<Row>>,
    /// Set while the rows are filled in, so that doesn't count as the user changing them.
    filling: Cell<bool>,
    clear: gtk::Button,
}

impl State {
    /// Shows every row's target for the scope, and where it comes from.
    fn fill(&self) {
        let t = inv::load_targets();
        let scope = self.scope.get();
        self.filling.set(true);
        for r in self.rows.borrow().iter() {
            let (v, src) = t.resolve(scope, r.doctrine.as_deref(), r.fit.as_deref());
            let here = t.exact(scope, r.doctrine.as_deref(), r.fit.as_deref()).is_some();
            r.spin.set_value(v as f64);
            r.from.set_text(&if here { "Set here".to_string() } else { format!("From {}", source_label(src)) });
            r.from.set_css_classes(if here { &["body-small", "primary-text"] } else { &["body-small", "dim"] });
            r.reset.set_visible(here);
        }
        self.filling.set(false);
        self.clear.set_visible(scope.is_some_and(|c| t.characters.contains_key(&c)));
    }

    fn set(&self, doctrine: Option<&str>, fit: Option<&str>, v: Option<u32>) {
        let mut t = inv::load_targets();
        t.set(self.scope.get(), doctrine, fit, v);
        inv::save_targets(self.ctx.core(), &t);
        self.fill();
    }
}

/// Opens the dialog on `scope` (None: everyone), listing the doctrines of `group` (None: all).
pub(super) fn open(ctx: &Ctx, scope: Option<i64>, group: Option<&str>) {
    crate::theme::add_css(CSS);
    let (win, content, actions) = w::dialog(ctx, "Stock targets", 640);
    win.set_default_height(720);

    let mut chars: Vec<(i64, String)> = ctx.core().characters().iter().map(|c| (c.id, c.name())).collect();
    chars.sort_by(|a, b| podstack_core::features::accounts::natural_cmp(&a.1, &b.1));
    let mut names = vec!["Everyone".to_string()];
    names.extend(chars.iter().map(|(_, n)| n.clone()));
    let at = scope.and_then(|s| chars.iter().position(|(id, _)| *id == s)).map(|i| i as u32 + 1).unwrap_or(0);
    let pick = crate::pages::skill_browser::shared::dropdown(&names, at);
    pick.set_halign(gtk::Align::Start);
    pick.set_tooltip_text(Some("Whose targets to set"));
    content.append(&pick);
    content.append(&w::text(
        "How many ships of each fit a pilot should keep. The most specific target set wins: for one pilot a fit, then a \
         doctrine, then everything that pilot flies; for everyone a fit, then a doctrine, then everything. 0 means \
         don't stock it.",
        &["body-small", "dim"],
    ));

    let clear = w::button("Clear this pilot's targets", Btn::Text);
    let state = Rc::new(State { ctx: ctx.clone(), scope: Cell::new(scope), rows: RefCell::new(Vec::new()), filling: Cell::new(false), clear: clear.clone() });

    let list = w::vbox(4);
    list.add_css_class("dst-list");
    list.append(&row(&state, "Every fit", Some("Unless a doctrine or fit says otherwise"), None, None, &[]));
    let doc = dc::load();
    for (path, d) in dc::doctrines_in(&doc, group) {
        let head = row(&state, &d.name, (!path.is_empty()).then(|| path.join(" \u{203a} ")).as_deref(), Some(&d.id), None, &["title-small"]);
        head.add_css_class("dst-doctrine");
        list.append(&head);
        for f in &d.fits {
            let sub = if f.role.is_empty() { f.ship_name.clone() } else { format!("{} · {}", f.ship_name, f.role) };
            let r = row(&state, &f.name, Some(&sub), Some(&d.id), Some(&f.id), &[]);
            r.add_css_class("dst-fit");
            list.append(&r);
        }
    }
    let scroll = w::scrolled(&list);
    scroll.set_min_content_height(420);
    content.append(&scroll);

    // the rows' and buttons' handlers hold the state weakly: it holds the widgets
    let s = Rc::downgrade(&state);
    pick.connect_selected_notify(move |d| {
        let Some(s) = s.upgrade() else { return };
        s.scope.set((d.selected() as usize).checked_sub(1).and_then(|i| chars.get(i)).map(|(id, _)| *id));
        s.fill();
    });
    let s = Rc::downgrade(&state);
    clear.connect_clicked(move |_| {
        let Some(s) = s.upgrade() else { return };
        let Some(c) = s.scope.get() else { return };
        let mut t = inv::load_targets();
        t.clear_character(c);
        inv::save_targets(s.ctx.core(), &t);
        s.fill();
    });
    actions.append(&clear);
    let done = w::button("Done", Btn::Filled);
    let w2 = win.clone();
    done.connect_clicked(move |_| w2.close());
    actions.append(&done);
    state.fill();
    // the window owns the state from here on
    win.connect_close_request(move |_| {
        state.rows.borrow_mut().clear();
        gtk::glib::Propagation::Proceed
    });
    win.present();
}

/// A titled row with the target spin, where the target comes from, and a reset when it's set here.
fn row(state: &Rc<State>, title: &str, sub: Option<&str>, doctrine: Option<&str>, fit: Option<&str>, title_classes: &[&str]) -> gtk::Box {
    let r = w::hbox(12);
    let titles = w::vbox(0);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    let classes: Vec<&str> = if title_classes.is_empty() { vec!["body-large"] } else { title_classes.to_vec() };
    titles.append(&w::text(title, &classes));
    if let Some(s) = sub {
        titles.append(&w::text(s, &["body-small", "dim"]));
    }
    r.append(&titles);
    let from = w::line("", &["body-small", "dim"]);
    from.set_xalign(1.0);
    r.append(&from);
    let reset = w::icon_button("undo", "Clear: inherit the target instead");
    reset.set_valign(gtk::Align::Center);
    r.append(&reset);
    let spin = gtk::SpinButton::with_range(0.0, 999.0, 1.0);
    spin.set_digits(0);
    spin.set_valign(gtk::Align::Center);
    spin.add_css_class("dst-spin");
    r.append(&spin);

    let (d, f) = (doctrine.map(str::to_string), fit.map(str::to_string));
    let s = Rc::downgrade(state);
    let (d2, f2) = (d.clone(), f.clone());
    spin.connect_value_changed(move |sp| {
        let Some(s) = s.upgrade() else { return };
        if !s.filling.get() {
            s.set(d2.as_deref(), f2.as_deref(), Some(sp.value_as_int().max(0) as u32));
        }
    });
    let s = Rc::downgrade(state);
    let (d3, f3) = (d.clone(), f.clone());
    reset.connect_clicked(move |_| {
        if let Some(s) = s.upgrade() {
            s.set(d3.as_deref(), f3.as_deref(), None);
        }
    });
    state.rows.borrow_mut().push(Row { doctrine: d, fit: f, spin, from, reset });
    r
}
