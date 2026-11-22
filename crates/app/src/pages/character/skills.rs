//! Character tab: skills. Headline counts, then every known skill by group (SP per group), with level pips (queued
//! levels too), a search and filters.

use super::summary::parts::{self, clear, collapsible};
use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::domains::skills::Skill;
use podstack_core::{fmt, Character};
use std::collections::HashMap;

const FILTERS: [&str; 5] = ["All", "Trained", "Partly trained", "In queue", "Level V"];
const COLUMNS: usize = 2;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Skills", body) };
    if c.skill_list().is_empty() {
        if c.skills.is_some() {
            body.append(&w::empty_state("library_books", "No skills", "This character's skill sheet is empty."));
        } else {
            body.append(&parts::pending_page(&c, "skills", "esi-skills.read_skills.v1", "skills", "library_books"));
        }
        return Page::new("Skills", body);
    }

    let skills = c.skill_list();
    let tree = c.skill_tree();
    let trained = skills.iter().filter(|s| s.trained_skill_level > 0).count();
    let maxed = skills.iter().filter(|s| s.trained_skill_level == 5).count();
    let v_tile = w::stat_tile("At level V", &fmt::int(maxed as i64), None);
    v_tile.add_css_class("accent");
    body.append(&w::tile_row(vec![
        w::stat_tile("Total SP", &fmt::int(c.total_sp()), (c.unallocated_sp() > 0).then(|| format!("+{} unallocated", fmt::int(c.unallocated_sp()))).as_deref()),
        w::stat_tile("Skills known", &fmt::int(skills.len() as i64), None),
        w::stat_tile("Trained", &fmt::int(trained as i64), None),
        v_tile,
        w::stat_tile("Skill groups", &fmt::int(tree.len() as i64), None),
    ]));

    let toolbar = w::hbox(12);
    let search = w::search_entry("Search skills or groups…");
    search.set_text(&ctx.recall("skills.search").unwrap_or_default());
    toolbar.append(&search);
    let filter = ctx.recall("skills.filter").and_then(|f| f.parse::<usize>().ok()).unwrap_or(0).min(FILTERS.len() - 1);
    let grid = w::hbox(16);
    grid.set_homogeneous(true);
    {
        let (ctx2, grid2) = (ctx.clone(), grid.clone());
        let seg = w::segmented(&FILTERS, filter, move |i| {
            ctx2.remember("skills.filter", i.to_string());
            render(&ctx2, &grid2, id);
        });
        toolbar.append(&seg);
    }
    {
        let (ctx2, grid2) = (ctx.clone(), grid.clone());
        search.connect_search_changed(move |s| {
            ctx2.remember("skills.search", s.text().to_string());
            render(&ctx2, &grid2, id);
        });
    }
    body.append(&toolbar);
    body.append(&grid);
    render(ctx, &grid, id);
    Page::new("Skills", body)
}

/// Highest level of each skill in the queue.
fn queued_levels(c: &Character) -> HashMap<i64, u8> {
    let mut m = HashMap::new();
    for q in c.queue() {
        let e = m.entry(q.skill_id).or_insert(0u8);
        *e = (*e).max(q.finished_level);
    }
    m
}

fn passes(filter: usize, s: &Skill, queued: &HashMap<i64, u8>) -> bool {
    match filter {
        1 => s.trained_skill_level > 0,
        2 => s.half_trained,
        3 => queued.get(&s.skill_id).map(|q| *q > s.trained_skill_level).unwrap_or(false),
        4 => s.trained_skill_level == 5,
        _ => true,
    }
}

fn render(ctx: &Ctx, grid: &gtk::Box, id: i64) {
    clear(grid);
    let Some(c) = ctx.core().character(id) else { return };
    let query = ctx.recall("skills.search").unwrap_or_default().trim().to_lowercase();
    let filter = ctx.recall("skills.filter").and_then(|f| f.parse::<usize>().ok()).unwrap_or(0);
    let queued = queued_levels(&c);

    let mut groups = vec![];
    for (name, list, total) in c.skill_tree() {
        let group_match = !query.is_empty() && name.to_lowercase().contains(&query);
        let shown: Vec<&Skill> = list
            .into_iter()
            .filter(|s| passes(filter, s, &queued))
            .filter(|s| query.is_empty() || group_match || s.skill_name.to_lowercase().contains(&query))
            .collect();
        if !shown.is_empty() {
            groups.push((name, shown, total));
        }
    }
    if groups.is_empty() {
        grid.set_homogeneous(false);
        grid.append(&w::empty_state("search_off", "Nothing matches", "Try another search or filter."));
        return;
    }
    grid.set_homogeneous(true);

    // masonry: each group goes to the shortest column
    let cols: Vec<gtk::Box> = (0..COLUMNS).map(|_| w::vbox(16)).collect();
    let mut heights = [0usize; COLUMNS];
    for (name, shown, total) in groups {
        let i = (0..COLUMNS).min_by_key(|i| heights[*i]).unwrap_or(0);
        heights[i] += shown.len() + 3;
        cols[i].append(&group_card(ctx, &name, &shown, total, &queued, !query.is_empty() || filter != 0));
    }
    for col in cols {
        col.set_valign(gtk::Align::Start);
        grid.append(&col);
    }
}

fn group_card(ctx: &Ctx, name: &str, skills: &[&Skill], total: i64, queued: &HashMap<i64, u8>, force_open: bool) -> gtk::Box {
    let rows = gtk::Grid::new();
    rows.set_column_spacing(16);
    rows.set_row_spacing(8);
    for (i, s) in skills.iter().enumerate() {
        let r = i as i32;
        let mut classes = vec!["body-medium"];
        if s.trained_skill_level == 0 {
            classes.push("dim");
        }
        let n = w::line(&s.skill_name, &classes);
        n.set_hexpand(true);
        rows.attach(&n, 0, r, 1, 1);
        let q = queued.get(&s.skill_id).copied().unwrap_or(0);
        let pips = w::skill_pips(s.trained_skill_level, s.half_trained, q);
        if q > s.trained_skill_level {
            pips.set_tooltip_text(Some(&format!("Queued to level {q}")));
        }
        rows.attach(&pips, 1, r, 1, 1);
        let lvl = if s.trained_skill_level > 0 { format!("Level {}", s.trained_skill_level) } else { "Untrained".into() };
        rows.attach(&parts::fit(&lvl, &["body-small", "dim"]), 2, r, 1, 1);
        let sp = parts::fit(&fmt::int(s.skillpoints_in_skill), &["body-small", "numeric", "dim"]);
        sp.set_xalign(1.0);
        sp.set_width_chars(10);
        rows.attach(&sp, 3, r, 1, 1);
    }

    let head = w::hbox(8);
    let t = w::line(name, &["title-medium"]);
    t.set_hexpand(true);
    head.append(&t);
    head.append(&w::line(&fmt::sp(total), &["body-medium", "numeric", "dim"]));
    let key = format!("skills.group.{name}");
    let card = w::vbox(0);
    card.add_css_class("card");
    let section = collapsible(ctx, &key, true, 8, &head.upcast(), &rows.upcast());
    if force_open {
        // a search or filter shows its results even in collapsed groups
        if let Some(rev) = section.last_child().and_then(|r| r.downcast::<gtk::Revealer>().ok()) {
            rev.set_reveal_child(true);
        }
        if let Some(chev) = section.first_child().and_then(|h| h.first_child()).and_then(|c| c.downcast::<gtk::Label>().ok()) {
            chev.set_text("expand_more");
        }
    }
    card.append(&section);
    card
}
