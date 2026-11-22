//! Skill Browser: every skill by market group, with its attributes, rank and description, the training time of
//! each level for a chosen pilot (alone and with prerequisites), the prerequisite tree, and "add to plan".

pub(crate) mod shared;

use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Status};
use gtk::prelude::*;
use podstack_core::features::fitting::{level_time, plan_levels, total_time};
use podstack_core::features::plans;
use podstack_core::sde::{SkillInfo, SHIPS, SKILLS};
use podstack_core::{fmt, training, training::roman, Character, Event};
use shared::{Catalogue, Row};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;

const SELECTED_KEY: &str = "skill_browser.selected";
const SEARCH_KEY: &str = "skill_browser.search";
const OPEN_KEY: &str = "skill_browser.open";

struct State {
    ctx: Ctx,
    pilot: Cell<Option<i64>>,
    selected: Cell<Option<i64>>,
    /// Level the plan card trains to (0 = the next untrained one).
    target: Cell<u8>,
    search: RefCell<String>,
    open: RefCell<HashSet<String>>,
    catalogue: Catalogue,
    detail: gtk::Box,
}

impl State {
    fn character(&self) -> Option<Arc<Character>> {
        self.pilot.get().and_then(|id| self.ctx.core().character(id))
    }

    fn select(self: &Rc<Self>, id: i64) {
        if id < 0 || self.selected.get() == Some(id) {
            return;
        }
        self.selected.set(Some(id));
        self.target.set(0);
        self.ctx.remember(SELECTED_KEY, id.to_string());
        render_detail(self);
        if let Some(s) = self.detail.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>() {
            s.vadjustment().set_value(0.0);
        }
    }

    fn toggle(self: &Rc<Self>, group: &str) {
        {
            let mut open = self.open.borrow_mut();
            if !open.remove(group) {
                open.insert(group.to_string());
            }
            self.ctx.remember(OPEN_KEY, open.iter().cloned().collect::<Vec<_>>().join("\n"));
        }
        render_list(self);
    }
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(shared::CSS);

    let state: Rc<State> = Rc::new_cyclic(|weak: &std::rc::Weak<State>| {
        let (wg, wi) = (weak.clone(), weak.clone());
        State {
            ctx: ctx.clone(),
            pilot: Cell::new(shared::selected_pilot(ctx)),
            selected: Cell::new(ctx.recall(SELECTED_KEY).and_then(|s| s.parse().ok())),
            target: Cell::new(0),
            search: RefCell::new(ctx.recall(SEARCH_KEY).unwrap_or_default()),
            open: RefCell::new(ctx.recall(OPEN_KEY).map(|s| s.lines().map(String::from).collect()).unwrap_or_default()),
            catalogue: Catalogue::new(
                move |g| {
                    if let Some(s) = wg.upgrade() {
                        s.toggle(g)
                    }
                },
                move |id| {
                    if let Some(s) = wi.upgrade() {
                        s.select(id)
                    }
                },
            ),
            detail: w::vbox(16),
        }
    });

    let pane = w::vbox(12);
    pane.add_css_class("browser-pane");
    let s = state.clone();
    pane.append(&shared::pilot_picker(ctx, move |id| {
        s.pilot.set(id);
        render_list(&s);
        render_detail(&s);
    }));
    let search = w::search_entry("Search skills (\"drone|gunnery\" for either)");
    search.set_text(&state.search.borrow());
    search.set_hexpand(false);
    let s = state.clone();
    search.connect_search_changed(move |e| {
        *s.search.borrow_mut() = e.text().to_string();
        s.ctx.remember(SEARCH_KEY, e.text().to_string());
        render_list(&s);
    });
    pane.append(&search);
    pane.append(&state.catalogue.scroller);

    state.detail.set_margin_bottom(8);
    let body = shared::two_pane(ctx, &pane, &state.detail);
    render_list(&state);
    render_detail(&state);

    // the page is static (it keeps the search box focused), so follow the pilot's data and plans here
    let s = state.clone();
    ctx.on_event(&body, move |e| match e {
        Event::CharacterUpdated(id) if Some(*id) == s.pilot.get() => {
            render_list(&s);
            render_detail(&s);
        }
        Event::Doc(d) if d == plans::DOC => render_detail(&s),
        _ => {}
    });

    Page::new("Skill Browser", body).no_scroll().static_page()
}

fn skill_trailing(c: Option<&Character>, id: i64) -> Option<gtk::Widget> {
    let c = c?;
    let s = c.skill(id);
    let trained = s.map(|s| s.trained_skill_level).unwrap_or(0);
    let pips = w::skill_pips(trained, s.map(|s| s.half_trained).unwrap_or(false), 0);
    if s.is_none() {
        pips.set_opacity(0.5);
        pips.set_tooltip_text(Some("Not injected"));
    } else {
        pips.set_tooltip_text(Some(&format!("Level {}", roman(trained))));
    }
    Some(pips.upcast())
}

fn render_list(state: &Rc<State>) {
    let c = state.character();
    let query = state.search.borrow().trim().to_lowercase();
    let mut rows = Vec::new();
    let flat = query.chars().count() >= 2;
    if flat {
        let mut found: Vec<&SkillInfo> = SKILLS.by_id.values().filter(|s| shared::search_matches(&query, &s.name)).collect();
        found.sort_by(|a, b| a.name.cmp(&b.name));
        for s in found {
            rows.push(Row::Item { id: s.type_id, content: shared::item_content(None, &s.name, skill_trailing(c.as_deref(), s.type_id)) });
        }
        if rows.is_empty() {
            let l = w::text("No skills match.", &["body-medium", "dim"]);
            l.set_margin_start(16);
            l.set_margin_top(8);
            rows.push(Row::Item { id: -1, content: l.upcast() });
        }
    } else {
        let open = state.open.borrow();
        for g in &SKILLS.groups {
            let mut skills: Vec<&SkillInfo> = g.types.iter().filter_map(|id| SKILLS.get(*id)).collect();
            if skills.is_empty() {
                continue;
            }
            skills.sort_by(|a, b| a.name.cmp(&b.name));
            let count = match &c {
                Some(c) => format!("{}/{}", skills.iter().filter(|s| c.trained_skill_level(s.type_id) > 0).count(), skills.len()),
                None => skills.len().to_string(),
            };
            let is_open = open.contains(&g.name);
            rows.push(Row::Group { key: g.name.clone(), title: g.name.clone(), count, open: is_open });
            if is_open {
                for s in skills {
                    rows.push(Row::Item { id: s.type_id, content: shared::item_content(None, &s.name, skill_trailing(c.as_deref(), s.type_id)) });
                }
            }
        }
    }
    state.catalogue.set_rows(rows, state.selected.get(), flat);
}

fn render_detail(state: &Rc<State>) {
    let d = &state.detail;
    shared::clear(d);
    let Some(info) = state.selected.get().and_then(|id| SKILLS.get(id)) else {
        d.append(&w::empty_state(
            "school",
            "No skill selected",
            "Pick a skill from the catalogue to see its prerequisites and training times. Select a pilot to compare against their skill sheet.",
        ));
        return;
    };
    let c = state.character();
    d.append(&hero(info, c.as_deref()));
    if let Some(c) = &c {
        d.append(&levels_card(info, c));
        if c.trained_skill_level(info.type_id) < 5 {
            d.append(&plan_card(state, info, c));
        }
    } else {
        d.append(&levels_card_static(info));
    }
    d.append(&prerequisites_card(state, info, c.as_deref()));
    if let Some(card) = unlocks_card(state, info) {
        d.append(&card);
    }
}

fn hero(info: &SkillInfo, c: Option<&Character>) -> gtk::Widget {
    let hero = w::vbox(16);
    hero.add_css_class("browser-hero");
    let top = w::hbox(20);
    let icon = w::icon_sized("school", 36);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_hexpand(true);
    let tile = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    tile.add_css_class("browser-hero-icon");
    tile.set_size_request(64, 64);
    tile.set_valign(gtk::Align::Start);
    tile.append(&icon);
    top.append(&tile);

    let texts = w::vbox(6);
    texts.set_hexpand(true);
    let name = w::text(&info.name, &["headline-small"]);
    name.set_selectable(true);
    texts.append(&name);
    texts.append(&w::text(SKILLS.group_of(info.type_id).map(|g| g.name.as_str()).unwrap_or(&info.market_group_name), &["body-medium", "dim"]));

    let mut chips: Vec<gtk::Widget> = vec![
        shared::info_chip(&format!("{} / {}", info.primary_attribute.name(), info.secondary_attribute.name())).upcast(),
        shared::info_chip(&format!("Rank {}", info.training_time_multiplier)).upcast(),
    ];
    if let Some(c) = c {
        let attrs = training::AttributeSet::of(c);
        let omega = c.is_omega().unwrap_or(true);
        let rate = training::sp_per_hour(info, &attrs, omega);
        chips.push(shared::info_chip(&format!("{} SP/hour{}", fmt::int(rate.round() as i64), if omega { "" } else { " (Alpha)" })).upcast());
        let status = match c.skill(info.type_id) {
            Some(s) if s.trained_skill_level == 5 => w::status("Trained V", Status::Good),
            Some(s) if s.trained_skill_level > 0 => w::status(&format!("Trained {}", roman(s.trained_skill_level)), Status::Info),
            Some(_) => w::status("Injected", Status::Neutral),
            None => w::status("Not injected", Status::Neutral),
        };
        chips.push(status.upcast());
        let cap = podstack_core::sde::alpha_cap(&info.name);
        if !omega {
            chips.push(
                w::status(&if cap > 0 { format!("Alpha limit {}", roman(cap)) } else { "Omega only".into() }, if cap > 0 { Status::Info } else { Status::Warn })
                    .upcast(),
            );
        }
    }
    texts.append(&shared::pill_row(chips));
    top.append(&texts);
    hero.append(&top);

    if !info.description.is_empty() {
        let desc = w::text(&info.description, &["body-medium", "dim"]);
        desc.set_selectable(true);
        hero.append(&desc);
    }
    hero.upcast()
}

fn grid_label(t: &str, classes: &[&str], right: bool) -> gtk::Label {
    let l = w::line(t, classes);
    if right {
        l.set_xalign(1.0);
    }
    l
}

fn levels_card(info: &SkillInfo, c: &Character) -> gtk::Widget {
    let g = gtk::Grid::new();
    g.add_css_class("browser-levels-grid");
    g.set_column_spacing(24);
    g.set_row_spacing(2);
    for (i, h) in ["Level", "Skill points", "Training time", "With prerequisites", ""].iter().enumerate() {
        g.attach(&grid_label(h, &["browser-levels-head"], i > 0 && i < 4), i as i32, 0, 1, 1);
    }
    let trained = c.trained_skill_level(info.type_id);
    let mut previous = chrono::Duration::zero();
    for lvl in 1..=5u8 {
        let row = lvl as i32;
        let level = w::hbox(12);
        level.append(&w::skill_pips(lvl, false, 0));
        level.append(&w::line(&format!("Level {}", roman(lvl)), &["body-medium"]));
        g.attach(&level, 0, row, 1, 1);
        g.attach(&grid_label(&fmt::int(info.sp_for_level(lvl)), &["body-medium", "numeric", "dim"], true), 1, row, 1, 1);
        if trained >= lvl {
            g.attach(&grid_label("\u{2014}", &["body-medium", "dim"], true), 2, row, 1, 1);
            g.attach(&grid_label("\u{2014}", &["body-medium", "dim"], true), 3, row, 1, 1);
            g.attach(&w::status("Trained", Status::Good), 4, row, 1, 1);
        } else {
            let t = level_time(c, info.type_id, lvl);
            let total = total_time(&plan_levels(c, &[(info.type_id, lvl)]));
            g.attach(&grid_label(&fmt::duration(t, 3), &["body-medium", "numeric"], true), 2, row, 1, 1);
            let with = grid_label(&fmt::duration(total, 3), &["body-medium", "numeric"], true);
            with.set_tooltip_text(Some(&format!("{} more than the level before", fmt::duration(total - previous, 3))));
            g.attach(&with, 3, row, 1, 1);
            previous = total;
        }
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    g.attach(&spacer, 5, 0, 1, 1);
    w::card_with("Training time", Some(&format!("For {} at current attributes", c.name())), &[], &g).upcast()
}

fn levels_card_static(info: &SkillInfo) -> gtk::Widget {
    let g = gtk::Grid::new();
    g.add_css_class("browser-levels-grid");
    g.set_column_spacing(24);
    g.attach(&grid_label("Level", &["browser-levels-head"], false), 0, 0, 1, 1);
    g.attach(&grid_label("Skill points", &["browser-levels-head"], true), 1, 0, 1, 1);
    for lvl in 1..=5u8 {
        let level = w::hbox(12);
        level.append(&w::skill_pips(lvl, false, 0));
        level.append(&w::line(&format!("Level {}", roman(lvl)), &["body-medium"]));
        g.attach(&level, 0, lvl as i32, 1, 1);
        g.attach(&grid_label(&fmt::int(info.sp_for_level(lvl)), &["body-medium", "numeric"], true), 1, lvl as i32, 1, 1);
    }
    w::card_with("Skill points", Some("Pick a pilot to see training times"), &[], &g).upcast()
}

fn plan_card(state: &Rc<State>, info: &SkillInfo, c: &Character) -> gtk::Widget {
    let trained = c.trained_skill_level(info.type_id);
    let target = match state.target.get() {
        0 => trained + 1,
        t => t.max(trained + 1),
    }
    .min(5);
    let body = w::vbox(12);

    let options: Vec<u8> = (trained + 1..=5).collect();
    let labels: Vec<String> = options.iter().map(|l| roman(*l).to_string()).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let s = state.clone();
    let opts = options.clone();
    let seg = w::segmented(&refs, options.iter().position(|l| *l == target).unwrap_or(0), move |i| {
        s.target.set(opts[i]);
        render_detail(&s);
    });
    body.append(&seg);

    let levels = plan_levels(c, &[(info.type_id, target)]);
    let total = total_time(&levels);
    let summary = w::text(
        &format!("{} level{} to train \u{00b7} {}", levels.len(), if levels.len() == 1 { "" } else { "s" }, fmt::duration(total, 3)),
        &["title-small"],
    );
    body.append(&summary);
    if levels.len() > 1 {
        let exp = gtk::Expander::new(Some("Individual skill breakdown"));
        exp.set_child(Some(&shared::levels_list(&levels)));
        body.append(&exp);
    }
    body.append(&shared::add_to_plan_row(&state.ctx, c.id, &format!("{} {}", info.name, roman(target)), None, &levels));
    w::card_with("Train to level", Some("Adds the missing prerequisites too, in training order"), &[], &body).upcast()
}

fn prerequisites_card(state: &Rc<State>, info: &SkillInfo, c: Option<&Character>) -> gtk::Widget {
    let body = w::vbox(0);
    if info.required_skills.is_empty() {
        body.append(&w::text("No prerequisites: any pilot can train this skill.", &["body-medium", "dim"]));
    } else {
        let mut seen = 0;
        for r in &info.required_skills {
            tree_rows(state, &body, r.id, r.level, 0, c, &mut seen);
        }
    }
    let missing = c.map(|c| training::missing_levels(c, &info.required_skills.iter().map(|r| (r.id, r.level)).collect::<Vec<_>>()));
    let subtitle = match (&missing, c) {
        (Some(m), Some(_)) if m.is_empty() => "All prerequisites trained".to_string(),
        (Some(m), Some(c)) => {
            let t: chrono::Duration = m.iter().fold(chrono::Duration::zero(), |t, &(id, l)| t + level_time(c, id, l));
            format!("{} level{} missing \u{00b7} {}", m.len(), if m.len() == 1 { "" } else { "s" }, fmt::duration(t, 3))
        }
        _ => "Click a skill to open it".to_string(),
    };
    w::card_with("Prerequisites", Some(&subtitle), &[], &body).upcast()
}

fn tree_rows(state: &Rc<State>, into: &gtk::Box, id: i64, level: u8, depth: i32, c: Option<&Character>, seen: &mut usize) {
    *seen += 1;
    if depth > 12 || *seen > 400 {
        return;
    }
    let Some(info) = SKILLS.get(id) else { return };
    let row = w::hbox(12);
    row.add_css_class("browser-tree-row");
    row.set_margin_start(depth * 28);
    let trained = c.map(|c| c.trained_skill_level(id));
    let (icon, class) = match trained {
        Some(t) if t >= level => ("check_circle", "success-text"),
        Some(_) => ("cancel", "error-text"),
        None => ("subdirectory_arrow_right", "dim"),
    };
    let i = w::icon_sized(icon, 20);
    i.add_css_class(class);
    row.append(&i);
    let name = w::markup(&format!("{} <b>{}</b>", fmt::escape(&info.name), roman(level)), &["body-medium"]);
    name.set_hexpand(true);
    row.append(&name);
    if let (Some(c), Some(t)) = (c, trained) {
        let txt = if t >= level {
            w::line(&format!("Trained {}", roman(t)), &["body-small", "success-text"])
        } else {
            let time = total_time(&plan_levels(c, &[(id, level)]));
            w::line(&format!("Has {} \u{00b7} {}", if t == 0 { "\u{2014}".into() } else { roman(t).to_string() }, fmt::duration(time, 2)), &["body-small", "warning-text"])
        };
        row.append(&txt);
    }
    let click = gtk::GestureClick::new();
    let s = state.clone();
    click.connect_released(move |_, _, _, _| s.select(id));
    row.add_controller(click);
    row.set_cursor_from_name(Some("pointer"));
    row.set_tooltip_text(Some(&format!("Open {}", info.name)));
    into.append(&row);
    for r in &info.required_skills {
        tree_rows(state, into, r.id, r.level, depth + 1, c, seen);
    }
}

/// Skills and ships that need this skill directly.
fn unlocks_card(state: &Rc<State>, info: &SkillInfo) -> Option<gtk::Widget> {
    let mut skills: Vec<(&SkillInfo, u8)> = SKILLS
        .by_id
        .values()
        .filter_map(|s| s.required_skills.iter().find(|r| r.id == info.type_id).map(|r| (s, r.level)))
        .collect();
    skills.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.name.cmp(&b.0.name)));
    let mut ships: Vec<(&str, u8)> =
        SHIPS.by_id.values().filter_map(|s| s.skills.iter().find(|r| r.id == info.type_id).map(|r| (s.name.as_str(), r.level))).collect();
    ships.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(b.0)));
    if skills.is_empty() && ships.is_empty() {
        return None;
    }
    let body = w::vbox(12);
    if !skills.is_empty() {
        body.append(&w::text("Skills", &["label-large", "dim"]));
        let chips: Vec<gtk::Widget> = skills
            .iter()
            .map(|(s, lvl)| {
                let b = w::button(&format!("{} \u{00b7} needs {}", s.name, roman(*lvl)), w::Btn::Outlined);
                b.add_css_class("chip");
                let st = state.clone();
                let id = s.type_id;
                b.connect_clicked(move |_| st.select(id));
                b.upcast()
            })
            .collect();
        body.append(&shared::chip_row(chips));
    }
    if !ships.is_empty() {
        body.append(&w::text("Ships", &["label-large", "dim"]));
        let chips: Vec<gtk::Widget> =
            ships.iter().map(|(n, lvl)| shared::info_chip(&format!("{n} \u{00b7} {}", roman(*lvl))).upcast()).collect();
        body.append(&shared::chip_row(chips));
    }
    let exp = gtk::Expander::new(None);
    exp.set_label_widget(Some(&w::text(
        &format!("Required by {} skill{} and {} ship{}", skills.len(), if skills.len() == 1 { "" } else { "s" }, ships.len(), if ships.len() == 1 { "" } else { "s" }),
        &["title-medium"],
    )));
    exp.set_child(Some(&body));
    body.set_margin_top(12);
    Some(w::card(&exp).upcast())
}
