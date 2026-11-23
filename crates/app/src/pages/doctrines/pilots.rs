//! The Pilots tab: every character with the doctrine fits it can fly (by group › doctrine), the fits it has the
//! hull for, and the ones closest to train, with their skill plans. Or, as a matrix, characters × doctrines.

use super::common::{self, fit_chip, plan_buttons, ROSTER_CSS};
use super::Tab;
use crate::ctx::{Ctx, Page};
use crate::pages::skill_browser::shared as browser;
use crate::widgets::{self as w, Btn, Status};
use gtk::prelude::*;
use podstack_core::features::doctrines::{self as dc, Doctrine, DoctrineDoc, DoctrineFit, Readiness, Standing};
use podstack_core::{config, fmt, Character};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

const VIEW_KEY: &str = "doctrines.pilots.view";
const GROUP_KEY: &str = "doctrines.pilots.group";
const SEARCH_KEY: &str = "doctrines.pilots.search";

/// How many fits each of "Hull only" and "Next to train" lists per character before folding the rest away.
const SHOWN: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Pilots,
    Matrix,
}

impl View {
    const ALL: [View; 2] = [View::Pilots, View::Matrix];
    fn label(self) -> &'static str {
        match self {
            View::Pilots => "Pilots",
            View::Matrix => "Matrix",
        }
    }
}

/// The view last chosen, or `PODSTACK_PILOTS_VIEW` (for UI testing), or Pilots.
fn current_view(ctx: &Ctx) -> View {
    let name = ctx.recall(VIEW_KEY).or_else(|| std::env::var("PODSTACK_PILOTS_VIEW").ok()).unwrap_or_default();
    View::ALL.into_iter().find(|v| v.label().eq_ignore_ascii_case(name.trim())).unwrap_or(View::Pilots)
}

struct State {
    ctx: Ctx,
    doc: DoctrineDoc,
    chars: Vec<Arc<Character>>,
    view: RefCell<View>,
    group: RefCell<Option<String>>,
    search: RefCell<String>,
    holder: gtk::Box,
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(ROSTER_CSS);
    let body = w::page_body();
    body.append(&super::tab_bar(ctx, Tab::Pilots));

    let doc = dc::load();
    if doc.fit_count() == 0 {
        let empty = w::empty_state(
            "military_tech",
            "No doctrines yet",
            "Add your groups' doctrines and the fits they accept, and this tab shows what each of your characters can fly, and what each is closest to.",
        );
        let go = w::button_icon("add", "Go to Doctrines", Btn::Filled);
        go.set_halign(gtk::Align::Center);
        go.set_margin_top(8);
        let c = ctx.clone();
        go.connect_clicked(move |_| super::open(&c, Tab::Doctrines));
        empty.append(&go);
        body.append(&empty);
        return Page::new("Doctrines", body);
    }
    let mut chars = ctx.core().characters();
    if chars.is_empty() {
        body.append(&w::empty_state("group_add", "No characters", "Add a character to see which doctrine fits it can fly."));
        return Page::new("Doctrines", body);
    }
    chars.sort_by_key(|c| c.name().to_lowercase());

    // a remembered group that has since been deleted means all groups
    let group = ctx.recall(GROUP_KEY).filter(|g| doc.group(g).is_some());
    let state = Rc::new(State {
        ctx: ctx.clone(),
        view: RefCell::new(current_view(ctx)),
        group: RefCell::new(group),
        search: RefCell::new(ctx.recall(SEARCH_KEY).unwrap_or_default()),
        holder: w::vbox(16),
        doc,
        chars,
    });
    body.append(&toolbar(&state));
    body.append(&state.holder);
    refill(&state);
    Page::new("Doctrines", body)
}

fn toolbar(state: &Rc<State>) -> gtk::Widget {
    let bar = w::hbox(12);
    bar.add_css_class("doc-toolbar");
    let labels: Vec<&str> = View::ALL.iter().map(|v| v.label()).collect();
    let selected = View::ALL.iter().position(|v| *v == *state.view.borrow()).unwrap_or(0);
    let s = state.clone();
    bar.append(&w::segmented(&labels, selected, move |i| {
        *s.view.borrow_mut() = View::ALL[i];
        s.ctx.remember(VIEW_KEY, View::ALL[i].label());
        refill(&s);
    }));

    let groups: Vec<(String, String)> = state.doc.all_groups().iter().map(|(p, g)| (g.id.clone(), p.join(" \u{203a} "))).collect();
    let mut names = vec!["All groups".to_string()];
    names.extend(groups.iter().map(|(_, n)| n.clone()));
    let index = state.group.borrow().as_ref().and_then(|id| groups.iter().position(|(g, _)| g == id)).map(|i| i as u32 + 1).unwrap_or(0);
    let dd = browser::dropdown(&names, index);
    dd.set_tooltip_text(Some("Only the doctrines of this group and its subgroups"));
    let s = state.clone();
    dd.connect_selected_notify(move |d| {
        let id = (d.selected() as usize).checked_sub(1).and_then(|i| groups.get(i)).map(|(g, _)| g.clone());
        s.ctx.remember(GROUP_KEY, id.clone().unwrap_or_default());
        *s.group.borrow_mut() = id;
        refill(&s);
    });
    bar.append(&dd);

    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let search = w::search_entry("Search characters");
    search.set_hexpand(false);
    search.set_width_chars(22);
    search.set_text(&state.search.borrow());
    let s = state.clone();
    search.connect_search_changed(move |e| {
        *s.search.borrow_mut() = e.text().to_string();
        s.ctx.remember(SEARCH_KEY, e.text().to_string());
        refill(&s);
    });
    bar.append(&search);
    bar.upcast()
}

fn refill(state: &Rc<State>) {
    browser::clear(&state.holder);
    let group = state.group.borrow().clone();
    let fits = dc::fits_in(&state.doc, group.as_deref());
    let doctrines = dc::doctrines_in(&state.doc, group.as_deref());
    if fits.is_empty() {
        state.holder.append(&w::empty_state("folder_open", "No fits in this group", "Pick another group, or add fits to this one on the Doctrines tab."));
        return;
    }
    let q = state.search.borrow().trim().to_lowercase();
    let chars: Vec<&Arc<Character>> = state.chars.iter().filter(|c| q.is_empty() || c.name().to_lowercase().contains(&q)).collect();
    if chars.is_empty() {
        state.holder.append(&w::empty_state("person_search", "No characters match", "Try another name."));
        return;
    }
    let standings: Vec<(&Arc<Character>, dc::PilotStandings)> = chars.iter().map(|c| (*c, dc::pilot_standings(c, &fits))).collect();

    state.holder.append(&summary(&standings, doctrines.len(), fits.len()));
    if let Some(n) = common::unloaded_note(standings.iter().filter(|(c, _)| c.skills.is_none()).count()) {
        state.holder.append(&n);
    }
    match *state.view.borrow() {
        View::Pilots => {
            let mut sorted = standings;
            // most doctrines first, then most fits, then by name (the list is already in name order)
            sorted.sort_by_key(|(c, s)| (c.skills.is_none(), std::cmp::Reverse(s.doctrines_ready()), std::cmp::Reverse(s.ready.len())));
            for (c, s) in &sorted {
                state.holder.append(&pilot_card(&state.ctx, c, s, doctrines.len(), fits.len()));
            }
        }
        View::Matrix => state.holder.append(&matrix(&standings, &doctrines)),
    }
}

fn summary(standings: &[(&Arc<Character>, dc::PilotStandings)], doctrines: usize, fits: usize) -> gtk::Widget {
    let loaded = standings.iter().filter(|(c, _)| c.skills.is_some()).count();
    let flying = standings.iter().filter(|(_, s)| !s.ready.is_empty()).count();
    let doctrines_covered = covered(standings, |s| s.doctrine.id.as_str());
    let fits_covered = covered(standings, |s| s.fit.id.as_str());
    let tile = |label: &str, value: String, sub: String| w::stat_tile(label, &value, Some(&sub));
    let first = tile("Pilots ready", flying.to_string(), format!("of {loaded} with a doctrine fit"));
    first.add_css_class("accent");
    w::tile_row(vec![
        first,
        tile("Doctrines covered", doctrines_covered.to_string(), format!("of {doctrines} have a pilot")),
        tile("Fits covered", fits_covered.to_string(), format!("of {fits} have a pilot")),
    ])
    .upcast()
}

/// How many distinct doctrines (or fits, by `key`) at least one character can fly.
fn covered<'a>(standings: &'a [(&Arc<Character>, dc::PilotStandings)], key: impl Fn(&'a Standing) -> &'a str) -> usize {
    standings.iter().flat_map(|(_, s)| s.ready.iter().map(&key)).collect::<std::collections::BTreeSet<_>>().len()
}

/// "Init › Capitals › Dreads" with the doctrine's name stressed.
fn doctrine_markup(path: &[&str], d: &Doctrine) -> String {
    let mut s = String::new();
    for p in path {
        s.push_str(&fmt::escape(p));
        s.push_str(" \u{203a} ");
    }
    format!("{s}<b>{}</b>", fmt::escape(&d.name))
}

fn fit_tooltip(f: &DoctrineFit) -> String {
    let mut t = f.ship_name.clone();
    if !f.role.trim().is_empty() {
        t.push_str(&format!(" \u{00b7} {}", f.role.trim()));
    }
    t
}

fn pilot_card(ctx: &Ctx, c: &Arc<Character>, s: &dc::PilotStandings, doctrines: usize, fits: usize) -> gtk::Widget {
    let card = w::vbox(12);
    card.add_css_class("card");
    card.add_css_class("doc-pilot-card");

    let head = w::hbox(16);
    head.add_css_class("doc-pilot-head");
    head.append(&w::avatar(&config::character_portrait(c.id, 128), 48));
    let names = w::vbox(2);
    names.set_hexpand(true);
    names.set_valign(gtk::Align::Center);
    names.append(&w::line(&c.name(), &["title-large"]));
    let sub = if c.skills.is_none() {
        "Skills not loaded yet".to_string()
    } else {
        format!("Flies {} of {fits} fit{} \u{00b7} {} of {doctrines} doctrine{}", s.ready.len(), plural(fits), s.doctrines_ready(), plural(doctrines))
    };
    names.append(&w::line(&sub, &["body-medium", "dim"]));
    head.append(&names);
    let pill = if c.skills.is_none() {
        w::status("Loading", Status::Neutral)
    } else if !s.ready.is_empty() {
        w::status(&format!("{} ready", s.doctrines_ready()), Status::Good)
    } else if !s.hull_only.is_empty() {
        w::status("Hulls only", Status::Warn)
    } else {
        w::status("Nothing yet", Status::Neutral)
    };
    head.append(&pill);
    card.append(&head);
    if c.skills.is_none() {
        return card.upcast();
    }

    if !s.ready.is_empty() {
        card.append(&section_label("CAN FLY"));
        // one line per doctrine, in the document's order
        let mut by_doctrine: Vec<(&Standing, Vec<&Standing>)> = Vec::new();
        for r in &s.ready {
            match by_doctrine.iter_mut().find(|(first, _)| first.doctrine.id == r.doctrine.id) {
                Some((_, v)) => v.push(r),
                None => by_doctrine.push((r, vec![r])),
            }
        }
        let list = w::vbox(10);
        for (first, fits) in by_doctrine {
            let row = w::vbox(6);
            row.append(&w::markup(&doctrine_markup(&first.path, first.doctrine), &["doc-doctrine-path"]));
            let chips: Vec<gtk::Widget> = fits.iter().map(|f| fit_chip(f.fit.ship_id, &f.fit.name, true, Some(&fit_tooltip(f.fit)))).collect();
            row.append(&browser::chip_row(chips));
            list.append(&row);
        }
        card.append(&list);
    }
    if !s.hull_only.is_empty() {
        card.append(&section_label("HULL ONLY"));
        card.append(&train_list(ctx, c, &s.hull_only));
    }
    if !s.missing.is_empty() {
        card.append(&section_label(if s.ready.is_empty() && s.hull_only.is_empty() { "CLOSEST TO TRAIN" } else { "NEXT TO TRAIN" }));
        card.append(&train_list(ctx, c, &s.missing));
    }
    card.upcast()
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn section_label(t: &str) -> gtk::Label {
    w::line(t, &["doc-section-label"])
}

/// The first few fits to train for, and the rest folded in an expander.
fn train_list(ctx: &Ctx, c: &Arc<Character>, list: &[Standing]) -> gtk::Widget {
    let b = w::vbox(0);
    for s in list.iter().take(SHOWN) {
        b.append(&train_row(ctx, c, s));
    }
    if list.len() > SHOWN {
        let rest = w::vbox(0);
        for s in &list[SHOWN..] {
            rest.append(&train_row(ctx, c, s));
        }
        let more = list.len() - SHOWN;
        b.append(&common::more_expander(more, &rest));
    }
    b.upcast()
}

fn train_row(ctx: &Ctx, c: &Arc<Character>, s: &Standing) -> gtk::Widget {
    let row = w::hbox(12);
    row.add_css_class("doc-train-row");
    let chip = fit_chip(s.fit.ship_id, &s.fit.name, false, Some(&fit_tooltip(s.fit)));
    chip.set_valign(gtk::Align::Center);
    row.append(&chip);
    let texts = w::vbox(2);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    let path = w::markup(&doctrine_markup(&s.path, s.doctrine), &["doc-doctrine-path"]);
    path.set_ellipsize(pango::EllipsizeMode::End);
    path.set_wrap(false);
    texts.append(&path);
    if s.check.readiness == Readiness::HullOnly && !s.check.missing_items.is_empty() {
        let n = s.check.missing_items.len();
        let l = w::line(&format!("Can't use {}", s.check.missing_items.join(", ")), &["body-small", "dim"]);
        l.set_tooltip_text(Some(&format!("{n} item{} not usable yet:\n{}", plural(n), s.check.missing_items.join("\n"))));
        texts.append(&l);
    }
    row.append(&texts);
    let time = w::line(&fmt::duration(s.check.time_to_fit, 2), &["body-medium", "numeric"]);
    time.set_tooltip_text(Some(&if s.check.readiness == Readiness::Missing {
        format!("Hull in {}, whole fit in {}", fmt::duration(s.check.time_to_hull, 3), fmt::duration(s.check.time_to_fit, 3))
    } else {
        format!("Whole fit in {}", fmt::duration(s.check.time_to_fit, 3))
    }));
    row.append(&time);
    let fit = s.fit.clone();
    row.append(&plan_buttons(ctx, c, &format!("{} ({})", s.fit.name, s.doctrine.name), move |c| dc::plan_for(c, &fit)));
    row.upcast()
}

/// Characters × doctrines: fits flyable per doctrine, or how close the character is.
fn matrix(standings: &[(&Arc<Character>, dc::PilotStandings)], doctrines: &[(Vec<&str>, &Doctrine)]) -> gtk::Widget {
    let grid = gtk::Grid::new();
    grid.add_css_class("doc-matrix");
    grid.set_row_spacing(4);
    grid.set_column_spacing(4);

    for (col, (path, d)) in doctrines.iter().enumerate() {
        let head = w::vbox(0);
        head.add_css_class("doc-matrix-head");
        head.set_valign(gtk::Align::End);
        if !path.is_empty() {
            let p = w::line(&path.join(" \u{203a} "), &["label-small", "dim"]);
            p.set_max_width_chars(14);
            head.append(&p);
        }
        let n = w::line(&d.name, &["label-large"]);
        n.set_max_width_chars(14);
        head.append(&n);
        head.set_tooltip_text(Some(&format!("{} \u{203a} {} ({} fit{})", path.join(" \u{203a} "), d.name, d.fits.len(), plural(d.fits.len()))));
        grid.attach(&head, col as i32 + 1, 0, 1, 1);
    }
    let last = doctrines.len() as i32 + 1;
    let h = w::line("Doctrines", &["label-large", "dim"]);
    h.add_css_class("doc-matrix-head");
    h.set_valign(gtk::Align::End);
    grid.attach(&h, last, 0, 1, 1);

    let mut pilots: BTreeMap<&str, usize> = BTreeMap::new();
    for (row, (c, s)) in standings.iter().enumerate() {
        let r = row as i32 + 1;
        let name = w::hbox(10);
        name.add_css_class("doc-matrix-name");
        name.append(&w::avatar(&config::character_portrait(c.id, 64), 28));
        let l = w::line(&c.name(), &["body-medium"]);
        l.set_max_width_chars(22);
        l.set_valign(gtk::Align::Center);
        name.append(&l);
        grid.attach(&name, 0, r, 1, 1);

        let all: Vec<&Standing> = s.ready.iter().chain(&s.hull_only).chain(&s.missing).collect();
        for (col, (_, d)) in doctrines.iter().enumerate() {
            let mine: Vec<&Standing> = all.iter().copied().filter(|x| x.doctrine.id == d.id).collect();
            let cell = dc::doctrine_cell(mine.iter().map(|x| &x.check));
            if cell.ready > 0 {
                *pilots.entry(d.id.as_str()).or_default() += 1;
            }
            let widget = matrix_cell(c, &cell);
            if c.skills.is_some() && !mine.is_empty() {
                let lines: Vec<String> = d
                    .fits
                    .iter()
                    .filter_map(|f| mine.iter().find(|x| x.fit.id == f.id))
                    .map(|x| match x.check.readiness {
                        Readiness::Ready => format!("\u{2713} {}", x.fit.name),
                        r => format!("\u{2022} {}: {} ({})", x.fit.name, r.label().to_lowercase(), fmt::duration(x.check.time_to_fit, 2)),
                    })
                    .collect();
                widget.set_tooltip_text(Some(&format!("{} \u{00b7} {}\n{}", c.name(), d.name, lines.join("\n"))));
            }
            grid.attach(&widget, col as i32 + 1, r, 1, 1);
        }
        let n = if c.skills.is_some() { format!("{} / {}", s.doctrines_ready(), doctrines.len()) } else { "\u{2014}".into() };
        let total = w::line(&n, &["body-medium", "numeric"]);
        total.add_css_class("doc-matrix-cell");
        grid.attach(&total, last, r, 1, 1);
    }

    let r = standings.len() as i32 + 1;
    let label = w::line("Pilots ready", &["title-small"]);
    label.add_css_class("doc-matrix-total");
    grid.attach(&label, 0, r, 1, 1);
    for (col, (_, d)) in doctrines.iter().enumerate() {
        let n = pilots.get(d.id.as_str()).copied().unwrap_or(0);
        let l = w::line(&n.to_string(), &["title-medium", "numeric", if n > 0 { "primary-text" } else { "dim" }]);
        l.add_css_class("doc-matrix-total");
        l.add_css_class("doc-matrix-cell");
        grid.attach(&l, col as i32 + 1, r, 1, 1);
    }
    let filler = w::line("", &["doc-matrix-total"]);
    grid.attach(&filler, last, r, 1, 1);

    // wide documents scroll sideways inside the card instead of widening the window
    let scroll = gtk::ScrolledWindow::new();
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Never);
    scroll.set_child(Some(&grid));
    let legend = w::hbox(8);
    legend.append(&w::status("2/3  fits ready", Status::Good));
    legend.append(&w::status("Hull  · time to the fit", Status::Warn));
    legend.append(&w::status("Time to the closest fit", Status::Neutral));
    let body = w::vbox(12);
    body.append(&scroll);
    body.append(&legend);
    w::card_with("Pilots \u{00d7} doctrines", Some("Fits each character can fly per doctrine, or how long until it can"), &[], &body).upcast()
}

fn matrix_cell(c: &Character, cell: &dc::DoctrineCell) -> gtk::Widget {
    let (text, kind) = match (c.skills.is_some(), cell.best) {
        (false, _) | (true, None) => ("\u{2014}".to_string(), None),
        (true, Some(Readiness::Ready)) => (format!("{}/{}", cell.ready, cell.fits), Some(Status::Good)),
        (true, Some(Readiness::HullOnly)) => (format!("Hull \u{00b7} {}", fmt::duration(cell.closest.unwrap_or_default(), 1)), Some(Status::Warn)),
        (true, Some(Readiness::Missing)) => (fmt::duration(cell.closest.unwrap_or_default(), 2), Some(Status::Neutral)),
    };
    let b = w::hbox(0);
    b.add_css_class("doc-matrix-cell");
    match kind {
        Some(k) => {
            let s = w::status(&text, k);
            s.add_css_class("numeric");
            b.append(&s);
        }
        None => b.append(&w::line(&text, &["dim"])),
    }
    b.upcast()
}
