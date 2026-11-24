//! The stock targets dialog: how many ships of each doctrine fit to keep, for everyone or for one character, set for
//! everything, per doctrine or per fit. Each place shows the target that applies there and where it comes from; a
//! value set there can be cleared to inherit again. Each doctrine can also be given the places its ships must be
//! (stations, structures or whole systems; none means anywhere). Changes save at once and the page behind updates
//! itself.

use crate::ctx::Ctx;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::doctrine_inventory::{self as inv, Level, Source, StockPlace};
use podstack_core::sde::MAP;
use std::collections::BTreeMap;
use podstack_core::features::doctrines as dc;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const CSS: &str = "
.dst-list { padding-right: 8px; }
.dst-doctrine { margin-top: 12px; }
.dst-fit { margin-left: 24px; }
.dst-spin { min-width: 96px; }
.dst-suggestions { min-width: 420px; }
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
    /// A doctrine's places, under its name.
    places: Option<gtk::Label>,
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
            if let (Some(l), Some(d)) = (&r.places, &r.doctrine) {
                let places = t.places(d);
                l.set_text(&places_summary(places));
                l.set_tooltip_text((!places.is_empty()).then(|| places.iter().map(|p| p.label()).collect::<Vec<_>>().join("\n")).as_deref());
                l.set_css_classes(if places.is_empty() { &["body-small", "dim"] } else { &["body-small", "primary-text"] });
            }
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
    let (win, content, actions) = w::dialog(ctx, "Stock targets", 760);
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
        let places = w::line("", &["body-small", "dim"]);
        places.set_xalign(0.0);
        places.set_ellipsize(gtk::pango::EllipsizeMode::End);
        places.set_max_width_chars(40);
        if let Some(titles) = head.first_child().and_downcast::<gtk::Box>() {
            titles.append(&places);
        }
        let edit = w::icon_button("place", "Where this doctrine's ships must be");
        edit.set_valign(gtk::Align::Center);
        let (s, id, name) = (Rc::downgrade(&state), d.id.clone(), d.name.clone());
        edit.connect_clicked(move |_| {
            if let Some(s) = s.upgrade() {
                let s2 = Rc::downgrade(&s);
                open_places(&s.ctx, &id, &name, move || {
                    if let Some(s) = s2.upgrade() {
                        s.fill();
                    }
                });
            }
        });
        head.insert_child_after(&edit, head.first_child().as_ref());
        if let Some(r) = state.rows.borrow_mut().last_mut() {
            r.places = Some(places);
        }
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
    state.rows.borrow_mut().push(Row { doctrine: d, fit: f, spin, from, reset, places: None });
    r
}

/// "Ships count anywhere", or where they must be.
pub(super) fn places_summary(places: &[StockPlace]) -> String {
    match places {
        [] => "Ships count anywhere".into(),
        [p] => format!("At {}", p.label()),
        list => format!("At {} places: {}", list.len(), list.iter().map(|p| p.label()).collect::<Vec<_>>().join("; ")),
    }
}

/// A place to offer in the search: systems from the static data, stations and structures where any character has
/// assets (CCP offers no public search for those).
#[derive(Clone)]
struct Offer {
    place: StockPlace,
    sub: String,
}

fn offers(ctx: &Ctx, query: &str) -> Vec<Offer> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut known: BTreeMap<i64, Offer> = BTreeMap::new();
    for c in ctx.core().characters().iter() {
        let Some(a) = &c.assets else { continue };
        for p in a.locations.values().filter(|p| matches!(p.kind.as_str(), "station" | "structure")) {
            if p.name.to_lowercase().contains(&q) {
                let sub = format!("{} · {}", if p.kind == "station" { "Station" } else { "Structure" }, p.system_name.clone().unwrap_or_default());
                known.entry(p.id).or_insert(Offer { place: StockPlace { id: p.id, name: p.name.clone(), system: false }, sub });
            }
        }
    }
    let mut out: Vec<Offer> = MAP
        .search_systems(query, 6)
        .into_iter()
        .map(|s| Offer {
            place: StockPlace { id: s.id, name: s.name.clone(), system: true },
            sub: format!("Whole system · {:.1} · {}", s.display_security(), MAP.region_name(s.region).unwrap_or("")),
        })
        .collect();
    let mut stations: Vec<Offer> = known.into_values().collect();
    stations.sort_by_key(|o| (!o.place.name.to_lowercase().starts_with(&q), o.place.name.len()));
    out.extend(stations.into_iter().take(10));
    out
}

/// Draws the places list again (held by the dialog while it's open).
type Redraw = Box<dyn Fn()>;

/// The places editor for one doctrine; `changed` runs after each change.
fn open_places(ctx: &Ctx, doctrine: &str, name: &str, changed: impl Fn() + 'static) {
    let (win, content, actions) = w::dialog(ctx, &format!("Where {name} ships must be"), 520);
    content.append(&w::text(
        "With no places, a pilot's ships count wherever they are. Add stations, structures or whole systems, and each fit's \
         stock target applies at each of them: only ships there count, and the shopping list says where to take what's \
         short.",
        &["body-small", "dim"],
    ));
    let list = w::vbox(4);
    content.append(&list);

    let entry = w::search_entry("Add a system, or a station or structure your characters use");
    let results = gtk::ListBox::new();
    results.set_selection_mode(gtk::SelectionMode::None);
    results.add_css_class("dst-suggestions");
    let pop = gtk::Popover::new();
    pop.set_autohide(true);
    pop.set_has_arrow(false);
    pop.set_position(gtk::PositionType::Bottom);
    pop.set_child(Some(&results));
    pop.set_parent(&entry);
    // popovers keep the keyboard focus: give it back to the entry so typing continues
    pop.set_can_focus(false);
    content.append(&entry);

    let changed: Rc<dyn Fn()> = Rc::new(changed);
    let redraw: Rc<RefCell<Option<Redraw>>> = Rc::default();
    let draw = {
        let (ctx, doctrine, list, changed, redraw2) = (ctx.clone(), doctrine.to_string(), list.clone(), changed.clone(), Rc::downgrade(&redraw));
        move || {
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            let t = inv::load_targets();
            if t.places(&doctrine).is_empty() {
                list.append(&w::text("Anywhere (no places set)", &["body-medium", "dim"]));
            }
            for p in t.places(&doctrine) {
                let remove = w::icon_button("close", "Remove this place");
                let (ctx, d, id, system, changed, redraw) = (ctx.clone(), doctrine.clone(), p.id, p.system, changed.clone(), redraw2.clone());
                remove.connect_clicked(move |_| {
                    let mut t = inv::load_targets();
                    t.remove_place(&d, id, system);
                    inv::save_targets(ctx.core(), &t);
                    changed();
                    if let Some(r) = redraw.upgrade() {
                        if let Some(f) = r.borrow().as_ref() {
                            f();
                        }
                    }
                });
                let icon = w::icon(if p.system { "public" } else { "location_city" });
                let sub = if p.system { "Anywhere in this system" } else { "This station or structure" };
                list.append(&w::list_row(Some(icon.upcast_ref()), &p.name, Some(sub), Some(remove.upcast_ref())));
            }
        }
    };
    draw();
    *redraw.borrow_mut() = Some(Box::new(draw));

    let hits: Rc<RefCell<Vec<Offer>>> = Rc::default();
    {
        let (ctx, results, pop, hits) = (ctx.clone(), results.clone(), pop.clone(), hits.clone());
        entry.connect_search_changed(move |e| {
            while let Some(c) = results.first_child() {
                results.remove(&c);
            }
            let found = offers(&ctx, &e.text());
            for o in &found {
                let row = w::vbox(0);
                row.set_margin_start(8);
                row.set_margin_end(8);
                row.set_margin_top(4);
                row.set_margin_bottom(4);
                row.append(&w::text(&o.place.name, &["body-medium"]));
                row.append(&w::text(&o.sub, &["body-small", "dim"]));
                results.append(&row);
            }
            *hits.borrow_mut() = found;
            if hits.borrow().is_empty() {
                pop.popdown();
            } else {
                pop.popup();
            }
        });
    }
    let add = {
        let (ctx, doctrine, changed, redraw) = (ctx.clone(), doctrine.to_string(), changed.clone(), redraw.clone());
        Rc::new(move |o: &Offer| {
            let mut t = inv::load_targets();
            t.add_place(&doctrine, o.place.clone());
            inv::save_targets(ctx.core(), &t);
            changed();
            if let Some(f) = redraw.borrow().as_ref() {
                f();
            }
        })
    };
    {
        let (hits, pop, entry2, add) = (hits.clone(), pop.clone(), entry.clone(), add.clone());
        results.connect_row_activated(move |_, row| {
            let picked = hits.borrow().get(row.index() as usize).cloned();
            if let Some(o) = picked {
                pop.popdown();
                entry2.set_text("");
                add(&o);
            }
        });
    }
    {
        let (hits, pop, add) = (hits.clone(), pop.clone(), add.clone());
        entry.connect_activate(move |e| {
            let picked = hits.borrow().first().cloned();
            if let Some(o) = picked {
                pop.popdown();
                e.set_text("");
                add(&o);
            }
        });
    }

    let done = w::button("Done", Btn::Filled);
    let w2 = win.clone();
    done.connect_clicked(move |_| w2.close());
    actions.append(&done);
    // the popover is parented to the entry: unparent it with the window, and drop the redraw closure (it holds the list)
    win.connect_close_request(move |_| {
        pop.unparent();
        redraw.borrow_mut().take();
        gtk::glib::Propagation::Proceed
    });
    win.present();
}
