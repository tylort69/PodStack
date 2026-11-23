//! The Ships tab: how many of the characters can fly each class of hull (CCP's ship groups: Dreadnought,
//! Battleship, Heavy Assault Cruiser, …), in sections from small to big; a class opens to its pilots per race and
//! per hull, and for everyone else the training to get in, with its skill plan.

use super::common::{self, pilot_chip, plan_buttons, ROSTER_CSS};
use super::Tab;
use crate::ctx::{Ctx, Page};
use crate::pages::skill_browser::shared as browser;
use crate::widgets::{self as w, Status};
use gtk::prelude::*;
use podstack_core::features::doctrines::{self as dc, ClassPilots, HullGrid};
use podstack_core::features::fitting::{ship_training, total_time};
use podstack_core::sde::{ShipInfo, SHIPS};
use podstack_core::{config, fmt, Character};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

const CLASS_KEY: &str = "doctrines.ships.class";
const TECH_KEY: &str = "doctrines.ships.tech";
const HIDE_KEY: &str = "doctrines.ships.hide_empty";
const SEARCH_KEY: &str = "doctrines.ships.search";

/// Characters listed per hull as "not yet" before folding the rest away.
const SHOWN: usize = 3;

struct State {
    ctx: Ctx,
    chars: Vec<Arc<Character>>,
    /// Tech levels to include (none picked: all).
    tech: RefCell<BTreeSet<String>>,
    hide_empty: RefCell<bool>,
    search: RefCell<String>,
    /// The class drilled into.
    class: RefCell<Option<String>>,
    holder: gtk::Box,
}

impl State {
    fn include(&self, s: &ShipInfo) -> bool {
        let tech = self.tech.borrow();
        tech.is_empty() || tech.contains(&s.meta)
    }

    fn open(self: &Rc<Self>, class: Option<String>) {
        self.ctx.remember(CLASS_KEY, class.clone().unwrap_or_default());
        *self.class.borrow_mut() = class;
        refill(self);
        if let Some(s) = self.holder.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>() {
            s.vadjustment().set_value(0.0);
        }
    }
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(ROSTER_CSS);
    let body = w::page_body();
    body.append(&super::tab_bar(ctx, Tab::Ships));
    let mut chars = ctx.core().characters();
    if chars.is_empty() {
        body.append(&w::empty_state("group_add", "No characters", "Add a character to see which hull classes it can fly."));
        return Page::new("Doctrines", body);
    }
    chars.sort_by_key(|c| c.name().to_lowercase());

    // the class last opened, or `PODSTACK_SHIP_CLASS` (for UI testing); "" is the grid
    let class = ctx
        .recall(CLASS_KEY)
        .or_else(|| std::env::var("PODSTACK_SHIP_CLASS").ok())
        .map(|c| c.trim().to_string())
        .and_then(|c| SHIPS.by_id.values().find(|s| s.group.eq_ignore_ascii_case(&c)).map(|s| s.group.clone()));
    let tech: BTreeSet<String> = ctx.recall(TECH_KEY).unwrap_or_default().split(',').filter(|t| !t.is_empty()).map(str::to_string).collect();
    let state = Rc::new(State {
        ctx: ctx.clone(),
        chars,
        tech: RefCell::new(tech),
        hide_empty: RefCell::new(ctx.recall(HIDE_KEY).as_deref() == Some("1")),
        search: RefCell::new(ctx.recall(SEARCH_KEY).unwrap_or_default()),
        class: RefCell::new(class),
        holder: w::vbox(16),
    });
    body.append(&toolbar(&state));
    body.append(&state.holder);
    refill(&state);
    Page::new("Doctrines", body)
}

fn toolbar(state: &Rc<State>) -> gtk::Widget {
    let bar = w::hbox(12);
    bar.add_css_class("doc-toolbar");
    let mut chips: Vec<gtk::Widget> = Vec::new();
    for t in dc::tech_levels() {
        let chip = w::chip(&t, state.tech.borrow().contains(&t));
        chip.set_tooltip_text(Some("Only hulls of the picked tech levels (none picked: all)"));
        let s = state.clone();
        chip.connect_toggled(move |c| {
            {
                let mut tech = s.tech.borrow_mut();
                if c.is_active() {
                    tech.insert(t.clone());
                } else {
                    tech.remove(&t);
                }
                s.ctx.remember(TECH_KEY, tech.iter().cloned().collect::<Vec<_>>().join(","));
            }
            refill(&s);
        });
        chips.push(chip.upcast());
    }
    let hide = w::chip("Hide classes nobody flies", *state.hide_empty.borrow());
    let s = state.clone();
    hide.connect_toggled(move |c| {
        *s.hide_empty.borrow_mut() = c.is_active();
        s.ctx.remember(HIDE_KEY, if c.is_active() { "1" } else { "0" });
        refill(&s);
    });
    chips.push(hide.upcast());
    // the chips wrap on narrow windows instead of widening the page
    let flow = browser::chip_row(chips);
    flow.set_hexpand(true);
    bar.append(&flow);

    let search = w::search_entry("Search classes or hulls");
    search.set_hexpand(false);
    search.set_valign(gtk::Align::Start);
    search.set_width_chars(22);
    search.set_text(&state.search.borrow());
    let s = state.clone();
    search.connect_search_changed(move |e| {
        *s.search.borrow_mut() = e.text().to_string();
        s.ctx.remember(SEARCH_KEY, e.text().to_string());
        // searching goes back to the grid, where the matches are
        if s.class.borrow().is_some() && !e.text().is_empty() {
            s.open(None);
        } else {
            refill(&s);
        }
    });
    bar.append(&search);
    bar.upcast()
}

fn loaded(state: &State) -> Vec<Arc<Character>> {
    state.chars.iter().filter(|c| c.skills.is_some()).cloned().collect()
}

fn refill(state: &Rc<State>) {
    browser::clear(&state.holder);
    let class = state.class.borrow().clone();
    match class {
        Some(class) => drill(state, &class),
        None => classes(state),
    }
    if let Some(n) = common::unloaded_note(state.chars.len() - loaded(state).len()) {
        state.holder.append(&n);
    }
}

// ---------- the grid of classes ----------

fn classes(state: &Rc<State>) {
    let q = state.search.borrow().trim().to_lowercase();
    let chars = loaded(state);
    let grid = dc::hull_grid(&chars, |s| state.include(s) && (q.is_empty() || s.group.to_lowercase().contains(&q) || s.name.to_lowercase().contains(&q)));
    let hide = *state.hide_empty.borrow();

    let flown = grid.classes.values().filter(|c| !c.pilots.is_empty()).count();
    let hulls: usize = grid.classes.values().map(|c| c.by_hull.len()).sum();
    let hulls_flown: usize = grid.classes.values().map(|c| c.by_hull.values().filter(|p| !p.is_empty()).count()).sum();
    let first = w::stat_tile("Pilots", &chars.len().to_string(), Some(&format!("of {} characters", state.chars.len())));
    first.add_css_class("accent");
    state.holder.append(&w::tile_row(vec![
        first,
        w::stat_tile("Classes flown", &flown.to_string(), Some(&format!("of {} classes", grid.classes.len()))),
        w::stat_tile("Hulls flown", &hulls_flown.to_string(), Some(&format!("of {hulls} hulls"))),
    ]));

    let mut shown = 0;
    for section in dc::hull_sections() {
        let tiles: Vec<gtk::Widget> = section
            .classes
            .iter()
            .filter_map(|name| grid.classes.get(name).map(|c| (name, c)))
            .filter(|(_, c)| !hide || !c.pilots.is_empty())
            .map(|(name, c)| class_tile(state, name, c, chars.len()))
            .collect();
        if tiles.is_empty() {
            continue;
        }
        shown += tiles.len();
        let head = w::hbox(8);
        head.add_css_class("doc-section-title");
        head.append(&w::line(&section.name, &["title-medium"]));
        let n = w::line(&format!("{} class{}", tiles.len(), if tiles.len() == 1 { "" } else { "es" }), &["body-medium", "dim"]);
        n.set_valign(gtk::Align::Baseline);
        head.append(&n);
        state.holder.append(&head);
        let flow = w::flow(tiles, 6);
        flow.set_min_children_per_line(1);
        flow.set_row_spacing(12);
        flow.set_column_spacing(12);
        state.holder.append(&flow);
    }
    if shown == 0 {
        let (title, body) = if !q.is_empty() {
            ("Nothing matches", "No class or hull has that name.")
        } else if chars.is_empty() {
            ("No skills loaded yet", "Classes show up once the characters' skills have loaded.")
        } else {
            ("No classes", "None of the characters can fly a class at these tech levels.")
        };
        state.holder.append(&w::empty_state("rocket_launch", title, body));
    }
}

/// The hull standing for a class on its tile: the one most characters fly, else the one first in the data.
fn face_of(c: &ClassPilots) -> Option<i64> {
    c.by_hull.iter().max_by(|a, b| a.1.len().cmp(&b.1.len()).then(b.0.cmp(a.0))).map(|(id, _)| *id)
}

fn class_tile(state: &Rc<State>, name: &str, c: &ClassPilots, pilots: usize) -> gtk::Widget {
    let b = gtk::Button::new();
    b.add_css_class("doc-class-tile");
    if c.pilots.is_empty() {
        b.add_css_class("nobody");
    }
    let row = w::hbox(14);
    if let Some(id) = face_of(c) {
        let frame = w::vbox(0);
        frame.add_css_class("doc-class-render");
        frame.set_overflow(gtk::Overflow::Hidden);
        frame.set_valign(gtk::Align::Center);
        frame.append(&w::eve_image(&config::type_render(id, 128), 64));
        row.append(&frame);
    }
    let texts = w::vbox(0);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    let n = w::line(name, &["title-small"]);
    n.set_max_width_chars(18);
    texts.append(&n);
    let count = w::line(&c.pilots.len().to_string(), &["doc-class-count", "numeric"]);
    texts.append(&count);
    texts.append(&w::line(
        &format!("of {pilots} pilot{} \u{00b7} {} hull{}", if pilots == 1 { "" } else { "s" }, c.by_hull.len(), if c.by_hull.len() == 1 { "" } else { "s" }),
        &["body-small", "dim"],
    ));
    row.append(&texts);
    b.set_child(Some(&row));
    let mut races: Vec<(&String, usize)> = c.by_race.iter().map(|(r, p)| (r, p.len())).collect();
    races.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let tip: Vec<String> = races.iter().map(|(r, n)| format!("{r}: {n}")).collect();
    b.set_tooltip_text(Some(&if tip.is_empty() { format!("Nobody flies a {name} yet") } else { tip.join("\n") }));
    let (s, name) = (state.clone(), name.to_string());
    b.connect_clicked(move |_| {
        let s = s.clone();
        let name = name.clone();
        // once the click handler has returned: the refill removes the button being clicked
        gtk::glib::idle_add_local_once(move || s.open(Some(name)));
    });
    b.upcast()
}

// ---------- one class ----------

fn drill(state: &Rc<State>, class: &str) {
    let chars = loaded(state);
    let grid: HullGrid = dc::hull_grid(&chars, |s| s.group == class && state.include(s));
    let section = dc::hull_sections().into_iter().find(|s| s.classes.iter().any(|c| c == class)).map(|s| s.name).unwrap_or_default();

    let head = w::hbox(12);
    let back = w::icon_button("arrow_back", "All classes");
    let s = state.clone();
    back.connect_clicked(move |_| {
        let s = s.clone();
        gtk::glib::idle_add_local_once(move || s.open(None));
    });
    head.append(&back);
    let titles = w::vbox(0);
    titles.set_hexpand(true);
    titles.append(&w::line(&section, &["label-medium", "dim"]));
    titles.append(&w::line(class, &["headline-small"]));
    head.append(&titles);
    state.holder.append(&head);

    let Some(cp) = grid.classes.get(class) else {
        state.holder.append(&w::empty_state("filter_alt_off", "No hulls at these tech levels", "Pick other tech levels above to see this class's hulls."));
        return;
    };

    // per race: pilots, and the hulls of that race
    let hulls: Vec<&ShipInfo> = cp.by_hull.keys().filter_map(|id| SHIPS.by_id.get(id)).collect();
    let mut races: BTreeMap<String, Vec<&ShipInfo>> = BTreeMap::new();
    for h in &hulls {
        races.entry(dc::race_name(h)).or_default().push(h);
    }
    let pilots_of_race = |r: &str| cp.by_race.get(r).map(|p| p.len()).unwrap_or(0);
    let mut order: Vec<String> = races.keys().cloned().collect();
    order.sort_by(|a, b| pilots_of_race(b).cmp(&pilots_of_race(a)).then(a.cmp(b)));

    let mut tiles = vec![{
        let t = w::stat_tile("All races", &cp.pilots.len().to_string(), Some(&format!("of {} pilots", chars.len())));
        t.add_css_class("accent");
        t
    }];
    for r in &order {
        let n = races[r].len();
        tiles.push(w::stat_tile(r, &pilots_of_race(r).to_string(), Some(&format!("pilot{} \u{00b7} {n} hull{}", if pilots_of_race(r) == 1 { "" } else { "s" }, if n == 1 { "" } else { "s" }))));
    }
    state.holder.append(&w::tile_row(tiles));

    for r in &order {
        let mut list = races[r].clone();
        let sp: BTreeMap<i64, i64> = list.iter().map(|h| (h.type_id, dc::sp_to_fly(h))).collect();
        list.sort_by(|a, b| sp[&a.type_id].cmp(&sp[&b.type_id]).then(a.name.cmp(&b.name)));
        let label = w::line(&r.to_uppercase(), &["doc-section-label"]);
        label.add_css_class("doc-section-title");
        state.holder.append(&label);
        for h in list {
            let who = cp.by_hull.get(&h.type_id).cloned().unwrap_or_default();
            state.holder.append(&hull_card(state, h, &who, &chars));
        }
    }
}

fn hull_card(state: &Rc<State>, h: &ShipInfo, who: &BTreeSet<i64>, chars: &[Arc<Character>]) -> gtk::Widget {
    let card = w::hbox(20);
    card.add_css_class("card");
    let frame = w::vbox(0);
    frame.add_css_class("doc-hull-render");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_valign(gtk::Align::Start);
    frame.append(&w::eve_image(&config::type_render(h.type_id, 256), 112));
    card.append(&frame);

    let body = w::vbox(10);
    body.set_hexpand(true);
    let title = w::hbox(8);
    let name = w::line(&h.name, &["title-large"]);
    name.set_selectable(true);
    title.append(&name);
    if !h.meta.is_empty() {
        let m = browser::info_chip(&h.meta);
        title.append(&m);
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    title.append(&spacer);
    title.append(&if who.is_empty() {
        w::status("Nobody yet", Status::Neutral)
    } else {
        w::status(&format!("{} can fly", who.len()), Status::Good)
    });
    body.append(&title);

    let flyers: Vec<gtk::Widget> = chars.iter().filter(|c| who.contains(&c.id)).map(|c| pilot_chip(c)).collect();
    if !flyers.is_empty() {
        body.append(&browser::chip_row(flyers));
    }

    // everyone else, quickest to get in first
    let mut rest: Vec<(&Arc<Character>, chrono::Duration)> =
        chars.iter().filter(|c| !who.contains(&c.id)).map(|c| (c, total_time(&ship_training(c, h)))).collect();
    rest.sort_by_key(|(_, t)| *t);
    if !rest.is_empty() {
        body.append(&w::line(if who.is_empty() { "CLOSEST TO FLY IT" } else { "NOT YET" }, &["doc-section-label"]));
        let list = w::vbox(0);
        for (c, t) in rest.iter().take(SHOWN) {
            list.append(&train_row(state, c, h, *t));
        }
        if rest.len() > SHOWN {
            let more = w::vbox(0);
            for (c, t) in &rest[SHOWN..] {
                more.append(&train_row(state, c, h, *t));
            }
            list.append(&common::more_expander(rest.len() - SHOWN, &more));
        }
        body.append(&list);
    }
    card.append(&body);
    card.upcast()
}

fn train_row(state: &Rc<State>, c: &Arc<Character>, h: &ShipInfo, time: chrono::Duration) -> gtk::Widget {
    let row = w::hbox(12);
    row.add_css_class("doc-train-row");
    row.append(&w::avatar(&config::character_portrait(c.id, 64), 28));
    let n = w::line(&c.name(), &["body-medium"]);
    n.set_hexpand(true);
    n.set_valign(gtk::Align::Center);
    row.append(&n);
    let t = w::line(&fmt::duration(time, 2), &["body-medium", "numeric"]);
    t.set_valign(gtk::Align::Center);
    row.append(&t);
    let ship = h.clone();
    row.append(&plan_buttons(&state.ctx, c, &format!("Fly {}", h.name), move |c| ship_training(c, &ship)));
    row.upcast()
}
