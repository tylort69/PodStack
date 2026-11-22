//! The plan editor (the open plan's items, reorderable by drag and drop or the selection bar) and the read-only
//! combined training queue.

use super::{dialogs, dur, dur_compare, plural, skillbooks, Columns, P};
use crate::widgets::{self as w, Btn};
use gtk::{gdk, glib, prelude::*};
use podstack_core::features::plan_tools::{order, profile, Item, SkillItem};
use podstack_core::{fmt, sde, training::roman};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

const W_HANDLE: i32 = 28;
const W_TIME: i32 = 112;
const W_COMPARE: i32 = 104;
const W_CHANGE: i32 = 96;
const W_GROUP: i32 = 150;
const W_ATTRS: i32 = 92;
const W_SPH: i32 = 70;
const W_REMAP: i32 = 112;
const W_FINISH: i32 = 128;
const W_PLAN: i32 = 150;
const W_ACTIONS: i32 = 132;

/// Per-skill times under the profiler's setup: (id, level) → ms, and the total.
struct Compare {
    times: HashMap<(i64, u8), f64>,
    total: f64,
    label: String,
}

fn comparison(p: &P) -> Option<Compare> {
    let st = p.st.borrow();
    let prof = st.compare()?;
    if !st.planner.queue.iter().any(Item::is_skill) {
        return None;
    }
    let c = p.character()?;
    let items = profile::simulate_items(&c, &p.base(), &st.planner.queue, &prof);
    Some(Compare {
        total: items.iter().map(|x| x.2).sum(),
        times: items.into_iter().map(|(id, l, t)| ((id, l), t)).collect(),
        label: prof.label(),
    })
}

fn cell(t: &str, width: i32, numeric: bool, classes: &[&str]) -> gtk::Label {
    let l = w::line(t, classes);
    l.set_width_chars(1);
    l.set_size_request(width, -1);
    l.set_max_width_chars(1);
    if numeric {
        l.set_xalign(1.0);
        l.add_css_class("numeric");
    }
    l.set_tooltip_text(if t.chars().count() > 18 { Some(t) } else { None });
    l
}

/// "−2h 3m" in green / "+1d" in amber, or a dash under 30 seconds.
fn change_cell(base: f64, compared: f64, width: i32) -> gtk::Label {
    let delta = compared - base;
    if delta.abs() < 30_000.0 {
        return cell("—", width, true, &["dim"]);
    }
    let t = format!("{}{}", if delta < 0.0 { "−" } else { "+" }, dur_compare(delta.abs()));
    cell(&t, width, true, &[if delta < 0.0 { "success-text" } else { "warning-text" }])
}

pub(super) fn card(p: &P) -> gtk::Widget {
    let queue_view = p.st.borrow().open.is_none();
    let compare = comparison(p);
    let (title, mut subtitle) = {
        let st = p.st.borrow();
        let title = if queue_view { "Training Queue".to_string() } else { st.name.clone() };
        let first = match &compare {
            Some(c) => Some(format!("Compared with: {}", c.label)),
            None if queue_view => Some("Switched-on plans, highest priority first".into()),
            None => None,
        };
        (title, first)
    };
    if let Some(a) = p.accelerator_note() {
        subtitle = Some(match subtitle {
            Some(s) => format!("{s} · {a}"),
            None => a,
        });
    }

    let actions: Vec<gtk::Widget> = if queue_view { queue_actions(p) } else { plan_actions(p) };
    let body = w::vbox(0);
    body.append(&skillbooks::strip(p));
    if queue_view {
        body.append(&queue_table(p, compare.as_ref()));
    } else {
        body.append(&plan_table(p, compare.as_ref()));
    }
    let c = w::card_with(&title, subtitle.as_deref(), &actions, &body);
    c.add_css_class("flush");
    if let Some(head) = c.first_child() {
        head.set_margin_start(20);
        head.set_margin_end(12);
        head.set_margin_top(14);
    }
    body.set_margin_bottom(4);
    c.upcast()
}

fn queue_actions(p: &P) -> Vec<gtk::Widget> {
    let copy = w::button_icon("content_copy", "Copy", Btn::Tonal);
    copy.set_tooltip_text(Some("Copy as an EVE skill plan, to import in EVE's Skill Plans window"));
    copy.set_sensitive(p.st.borrow().planner.skill_count() > 0);
    let pp = p.clone();
    copy.connect_clicked(move |_| super::io::copy_eve(&pp));
    vec![copy.upcast()]
}

fn plan_actions(p: &P) -> Vec<gtk::Widget> {
    let n = p.st.borrow().planner.queue.len();
    let sort = w::button_icon("sort", "Sort", Btn::Text);
    sort.set_tooltip_text(Some("Fastest to train first, prerequisites kept in order"));
    sort.set_sensitive(n >= 2);
    {
        let pp = p.clone();
        sort.connect_clicked(move |_| pp.edit(order::sort_by_training_time));
    }
    let opt = w::button_icon("auto_fix_high", "Optimise", Btn::Text);
    opt.set_tooltip_text(Some("Reorder to finish the whole plan as soon as possible, prerequisites kept in order"));
    opt.set_sensitive(n >= 2);
    {
        let pp = p.clone();
        opt.connect_clicked(move |_| dialogs::prompt_optimise(&pp, false));
    }
    let remap = w::button_icon("tune", "Remap", Btn::Text);
    remap.set_tooltip_text(Some("Add an attribute remap at the start of the plan"));
    {
        let pp = p.clone();
        remap.connect_clicked(move |_| dialogs::remap(&pp, None));
    }
    let note = w::button_icon("sticky_note_2", "Note", Btn::Text);
    note.set_tooltip_text(Some("Add a note at the end of the plan"));
    {
        let pp = p.clone();
        note.connect_clicked(move |_| dialogs::note(&pp, None));
    }
    let cols = gtk::MenuButton::new();
    cols.set_child(Some(&w::icon("view_column")));
    cols.add_css_class("icon-button");
    cols.set_tooltip_text(Some("Columns"));
    cols.set_valign(gtk::Align::Center);
    let pop = gtk::Popover::new();
    let bx = w::vbox(2);
    bx.set_margin_start(8);
    bx.set_margin_end(12);
    let current = p.st.borrow().columns;
    type Setter = fn(&mut Columns, bool);
    let items: [(&str, bool, Setter); 6] = [
        ("Training time", current.time, |c, v| c.time = v),
        ("Group", current.group, |c, v| c.group = v),
        ("Attributes", current.attributes, |c, v| c.attributes = v),
        ("SP/h", current.sp_hour, |c, v| c.sp_hour = v),
        ("Since remap", current.since_remap, |c, v| c.since_remap = v),
        ("Finishes", current.finishes, |c, v| c.finishes = v),
    ];
    for (label, on, set) in items {
        let cb = gtk::CheckButton::with_label(label);
        cb.set_active(on);
        let pp = p.clone();
        cb.connect_toggled(move |cb| {
            let mut c = pp.st.borrow().columns;
            set(&mut c, cb.is_active());
            pp.set_columns(c);
        });
        bx.append(&cb);
    }
    pop.set_child(Some(&bx));
    cols.set_popover(Some(&pop));
    vec![sort.upcast(), opt.upcast(), remap.upcast(), note.upcast(), cols.upcast()]
}

/// The combined training queue: read-only, with the plan each level came from and when it's done.
fn queue_table(p: &P, compare: Option<&Compare>) -> gtk::Widget {
    let st = p.st.borrow();
    let queue = &st.planner.queue;
    let out = w::vbox(0);
    if queue.is_empty() {
        let t = w::text("Nothing to train. Switch on a plan in the Plans list, or create one.", &["body-medium", "dim"]);
        t.set_margin_start(20);
        t.set_margin_top(12);
        t.set_margin_bottom(16);
        out.append(&t);
        return out.upcast();
    }
    let head = w::hbox(8);
    head.add_css_class("plan-head-row");
    let name = cell("Skill", 100, false, &[]);
    name.set_hexpand(true);
    head.append(&name);
    head.append(&cell("Plan", W_PLAN, false, &[]));
    head.append(&cell("Training time", W_TIME, true, &[]));
    if let Some(c) = compare {
        let h = cell("With setup", W_COMPARE, true, &[]);
        h.set_tooltip_text(Some(&c.label));
        head.append(&h);
        head.append(&cell("Change", W_CHANGE, true, &[]));
    }
    head.append(&cell("Done in", W_TIME, true, &[]));
    head.append(&cell("Finishes", W_FINISH, true, &[]));
    out.append(&head);

    let list = gtk::ListBox::new();
    list.add_css_class("plan-rows");
    list.set_selection_mode(gtk::SelectionMode::None);
    let now = chrono::Utc::now();
    let mut elapsed = 0.0;
    for it in queue {
        let row = w::hbox(8);
        row.add_css_class("plan-row");
        row.set_margin_start(8);
        let lrow = gtk::ListBoxRow::new();
        lrow.set_activatable(false);
        match it {
            Item::Skill(s) => {
                elapsed += s.time;
                let name = cell(&s.title(), 100, false, &["body-medium"]);
                name.set_hexpand(true);
                row.append(&name);
                row.append(&cell(it.plan().map(|x| x.1.as_str()).unwrap_or(""), W_PLAN, false, &["dim"]));
                row.append(&cell(&dur(s.time), W_TIME, true, &[]));
                if let Some(c) = compare {
                    let t = c.times.get(&(s.id, s.level)).copied().unwrap_or(0.0);
                    row.append(&cell(&dur_compare(t), W_COMPARE, true, &[]));
                    row.append(&change_cell(s.time, t, W_CHANGE));
                }
                row.append(&cell(&dur(elapsed), W_TIME, true, &["dim"]));
                row.append(&cell(&fmt::datetime(now + chrono::Duration::milliseconds(elapsed as i64)), W_FINISH, true, &["dim"]));
            }
            Item::Remap { .. } | Item::Note { .. } => {
                lrow.add_css_class("marker");
                row.append(&w::icon_sized(if matches!(it, Item::Remap { .. }) { "tune" } else { "sticky_note_2" }, 18));
                let t = match it {
                    Item::Note { title, .. } => title.clone(),
                    _ => it.title(),
                };
                let l = w::line(&t, &["body-medium"]);
                l.set_hexpand(true);
                row.append(&l);
                if let Some(pl) = it.plan() {
                    row.append(&cell(&pl.1, W_PLAN, false, &["dim"]));
                }
            }
        }
        lrow.set_child(Some(&row));
        list.append(&lrow);
    }
    out.append(&list);

    let n = st.planner.skill_count();
    let foot = w::hbox(8);
    foot.add_css_class("plan-foot-row");
    let l = cell(&format!("{n} skill{}", plural(n)), 100, false, &[]);
    l.set_hexpand(true);
    foot.append(&l);
    foot.append(&cell("", W_PLAN, false, &[]));
    foot.append(&cell(&dur(st.planner.time), W_TIME, true, &[]));
    if let Some(c) = compare {
        foot.append(&cell(&dur_compare(c.total), W_COMPARE, true, &[]));
        foot.append(&change_cell(st.planner.time, c.total, W_CHANGE));
    }
    foot.append(&cell("", W_TIME, true, &[]));
    foot.append(&cell(&fmt::datetime(now + chrono::Duration::milliseconds(st.planner.time as i64)), W_FINISH, true, &[]));
    out.append(&foot);
    out.upcast()
}

/// Widgets the selection updates in place (without rebuilding the rows).
struct Sel {
    rows: RefCell<Vec<gtk::ListBoxRow>>,
    bar: gtk::Revealer,
    bar_label: gtk::Label,
    foot_label: gtk::Label,
    /// first row of a shift-click range
    anchor: Cell<Option<usize>>,
}

fn selected_time(p: &P) -> f64 {
    let st = p.st.borrow();
    st.selection.iter().filter_map(|i| st.planner.queue.get(*i)).filter_map(|i| i.skill()).map(|s| s.time).sum()
}

fn foot_text(p: &P) -> String {
    let st = p.st.borrow();
    let n = st.planner.skill_count();
    if st.selection.len() > 1 {
        drop(st);
        let st2 = p.st.borrow();
        format!("{n} skill{} ({} selected · {})", plural(n), st2.selection.len(), dur(selected_time(p)))
    } else {
        format!("{n} skill{}", plural(n))
    }
}

fn update_selection(p: &P, sel: &Sel) {
    let picked = p.st.borrow().selection.clone();
    for (i, r) in sel.rows.borrow().iter().enumerate() {
        if picked.contains(&i) {
            r.add_css_class("picked");
        } else {
            r.remove_css_class("picked");
        }
    }
    sel.bar.set_reveal_child(!picked.is_empty());
    sel.bar_label.set_text(&format!("{} selected · {}", picked.len(), dur(selected_time(p))));
    sel.foot_label.set_text(&foot_text(p));
}

/// A click on row `i`: plain picks it (or unpicks it when it's the only one), Ctrl toggles, Shift picks a range.
fn click(p: &P, sel: &Sel, i: usize, state: gdk::ModifierType) {
    {
        let mut st = p.st.borrow_mut();
        let s = &mut st.selection;
        if state.contains(gdk::ModifierType::SHIFT_MASK) && sel.anchor.get().is_some() {
            let a = sel.anchor.get().unwrap_or(i);
            let (lo, hi) = if a < i { (a, i) } else { (i, a) };
            for k in lo..=hi {
                if !s.contains(&k) {
                    s.push(k);
                }
            }
        } else if state.contains(gdk::ModifierType::CONTROL_MASK) {
            match s.iter().position(|x| *x == i) {
                Some(pos) => {
                    s.remove(pos);
                }
                None => s.push(i),
            }
            sel.anchor.set(Some(i));
        } else if s.len() == 1 && s[0] == i {
            s.clear();
            sel.anchor.set(None);
        } else {
            *s = vec![i];
            sel.anchor.set(Some(i));
        }
        s.sort_unstable();
    }
    update_selection(p, sel);
}

/// Runs an edit, then picks `after` (positions) again.
fn edit_keep(p: &P, f: impl FnOnce(&mut podstack_core::features::plan_tools::Planner), after: Vec<usize>) {
    {
        let mut st = p.st.borrow_mut();
        f(&mut st.planner);
    }
    p.save();
    let len = p.st.borrow().planner.queue.len();
    p.st.borrow_mut().selection = after.into_iter().filter(|i| *i < len).collect();
    p.render();
}

fn move_selection(p: &P, up: bool) {
    let picked = p.st.borrow().selection.clone();
    let len = p.st.borrow().planner.queue.len();
    if picked.is_empty() {
        return;
    }
    let first = picked[0];
    let last = *picked.last().unwrap_or(&first);
    if (up && first == 0) || (!up && last + 1 >= len) {
        return;
    }
    let after: Vec<usize> = picked.iter().map(|i| if up { i - 1 } else { i + 1 }).collect();
    if picked.len() == 1 {
        let to = if up { first - 1 } else { first + 1 };
        edit_keep(p, |pl| pl.move_item(first, to), after);
    } else if up {
        edit_keep(p, |pl| pl.move_items(first, first - 1, &picked), after);
    } else {
        edit_keep(p, |pl| pl.move_items(last, last + 1, &picked), after);
    }
}

/// Drops the dragged row `from` above or below row `target`.
fn drop_on(p: &P, from: usize, target: usize, below: bool) {
    // the dragged item's position once moved ("remove, then insert")
    let new = match (from < target, below) {
        (true, true) => target,
        (true, false) => target.saturating_sub(1),
        (false, true) => target + 1,
        (false, false) => target,
    };
    let picked = p.st.borrow().selection.clone();
    if picked.len() > 1 && picked.contains(&from) {
        p.edit(|pl| pl.move_items(from, new, &picked));
    } else if new != from {
        let len = p.st.borrow().planner.queue.len();
        edit_keep(p, |pl| pl.move_item(from, new), vec![new.min(len.saturating_sub(1))]);
    }
}

/// The open plan: header, rows and footer.
fn plan_table(p: &P, compare: Option<&Compare>) -> gtk::Widget {
    let cols = p.st.borrow().columns;
    let show_group = cols.group && compare.is_none();
    let out = w::vbox(0);

    // selection bar
    let bar_label = w::line("", &["label-large"]);
    bar_label.set_hexpand(true);
    let bar = w::hbox(4);
    bar.add_css_class("plan-selection-bar");
    bar.append(&bar_label);
    let up = w::button_icon("arrow_upward", "Up", Btn::Text);
    let down = w::button_icon("arrow_downward", "Down", Btn::Text);
    let remove = w::button_icon("delete", "Remove", Btn::Text);
    let clear = w::icon_button("close", "Clear selection");
    bar.append(&up);
    bar.append(&down);
    bar.append(&remove);
    bar.append(&clear);
    let revealer = gtk::Revealer::new();
    revealer.set_child(Some(&bar));
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    out.append(&revealer);

    let queue = p.st.borrow().planner.queue.clone();
    if queue.is_empty() {
        let t = w::text(
            "This plan is empty. Add skills from the Skill Catalogue, import a plan, or use the fit planner.",
            &["body-medium", "dim"],
        );
        t.set_margin_start(20);
        t.set_margin_top(12);
        t.set_margin_bottom(16);
        out.append(&t);
        return out.upcast();
    }

    // header
    let head = w::hbox(8);
    head.add_css_class("plan-head-row");
    head.append(&cell("", W_HANDLE - 8, false, &[]));
    let name = cell("Skill", 100, false, &[]);
    name.set_hexpand(true);
    head.append(&name);
    if cols.time {
        head.append(&cell("Training time", W_TIME, true, &[]));
    }
    if let Some(c) = compare {
        let h = cell("With setup", W_COMPARE, true, &[]);
        h.set_tooltip_text(Some(&c.label));
        head.append(&h);
        head.append(&cell("Change", W_CHANGE, true, &[]));
    }
    if show_group {
        head.append(&cell("Group", W_GROUP, false, &[]));
    }
    if cols.attributes {
        head.append(&cell("Attributes", W_ATTRS, false, &[]));
    }
    if cols.sp_hour {
        head.append(&cell("SP/h", W_SPH, true, &[]));
    }
    if cols.since_remap {
        head.append(&cell("Since remap", W_REMAP, true, &[]));
    }
    if cols.finishes {
        head.append(&cell("Finishes", W_FINISH, true, &[]));
    }
    head.append(&cell("", W_ACTIONS, false, &[]));
    out.append(&head);

    let foot_label = cell("", 100, false, &[]);
    let sel = Rc::new(Sel { rows: RefCell::new(vec![]), bar: revealer.clone(), bar_label, foot_label: foot_label.clone(), anchor: Cell::new(None) });
    {
        let (pp, s) = (p.clone(), sel.clone());
        let pp1 = pp.clone();
        up.connect_clicked(move |_| move_selection(&pp1, true));
        let pp2 = pp.clone();
        down.connect_clicked(move |_| move_selection(&pp2, false));
        let pp3 = pp.clone();
        remove.connect_clicked(move |_| {
            let picked = pp3.st.borrow().selection.clone();
            pp3.edit(|pl| pl.remove_items(&picked));
        });
        clear.connect_clicked(move |_| {
            pp.st.borrow_mut().selection.clear();
            update_selection(&pp, &s);
        });
    }

    let list = gtk::ListBox::new();
    list.add_css_class("plan-rows");
    list.set_selection_mode(gtk::SelectionMode::None);
    let now = chrono::Utc::now();
    let mut elapsed = 0.0;
    let mut rows = vec![];
    for (i, it) in queue.iter().enumerate() {
        let row = w::hbox(8);
        row.add_css_class("plan-row");
        let handle = w::icon_sized("drag_indicator", 20);
        handle.add_css_class("drag-handle");
        handle.set_size_request(W_HANDLE - 8, -1);
        handle.set_tooltip_text(Some("Drag to move"));
        row.append(&handle);
        let lrow = gtk::ListBoxRow::new();
        lrow.set_activatable(false);
        match it {
            Item::Skill(s) => {
                elapsed += s.time;
                skill_cells(p, &row, &queue, i, s, cols, show_group, compare, now, elapsed);
            }
            Item::Remap { .. } | Item::Note { .. } => {
                lrow.add_css_class("marker");
                marker_cells(p, &row, i, it, cols, show_group, compare);
            }
        }
        lrow.set_child(Some(&row));
        add_row_controllers(p, &sel, &lrow, &row, i);
        list.append(&lrow);
        rows.push(lrow);
    }
    *sel.rows.borrow_mut() = rows;
    {
        // Delete removes the picked items, Escape unpicks them
        let (pp, s) = (p.clone(), sel.clone());
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, k, _, _| {
            if k == gdk::Key::Delete && !pp.st.borrow().selection.is_empty() {
                let picked = pp.st.borrow().selection.clone();
                pp.edit(|pl| pl.remove_items(&picked));
                glib::Propagation::Stop
            } else if k == gdk::Key::Escape && !pp.st.borrow().selection.is_empty() {
                pp.st.borrow_mut().selection.clear();
                update_selection(&pp, &s);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        list.add_controller(key);
    }
    out.append(&list);

    // footer
    let st = p.st.borrow();
    let foot = w::hbox(8);
    foot.add_css_class("plan-foot-row");
    foot.append(&cell("", W_HANDLE - 8, false, &[]));
    foot_label.set_hexpand(true);
    foot.append(&foot_label);
    if cols.time {
        foot.append(&cell(&dur(st.planner.time), W_TIME, true, &[]));
    }
    if let Some(c) = compare {
        foot.append(&cell(&dur_compare(c.total), W_COMPARE, true, &[]));
        foot.append(&change_cell(st.planner.time, c.total, W_CHANGE));
    }
    if show_group {
        foot.append(&cell("", W_GROUP, false, &[]));
    }
    if cols.attributes {
        foot.append(&cell("", W_ATTRS, false, &[]));
    }
    if cols.sp_hour {
        foot.append(&cell("", W_SPH, true, &[]));
    }
    if cols.since_remap {
        foot.append(&cell("", W_REMAP, true, &[]));
    }
    if cols.finishes {
        foot.append(&cell(&fmt::datetime(now + chrono::Duration::milliseconds(st.planner.time as i64)), W_FINISH, true, &[]));
    }
    let total = if cols.time { String::new() } else { dur(st.planner.time) };
    foot.append(&cell(&total, W_ACTIONS, true, &[]));
    drop(st);
    out.append(&foot);
    update_selection(p, &sel);
    out.upcast()
}

#[allow(clippy::too_many_arguments)]
fn skill_cells(
    p: &P,
    row: &gtk::Box,
    queue: &[Item],
    i: usize,
    s: &SkillItem,
    cols: Columns,
    show_group: bool,
    compare: Option<&Compare>,
    now: chrono::DateTime<chrono::Utc>,
    elapsed: f64,
) {
    let name = cell(&s.title(), 100, false, &["body-medium"]);
    name.set_hexpand(true);
    row.append(&name);
    if cols.time {
        row.append(&cell(&dur(s.time), W_TIME, true, &[]));
    }
    if let Some(c) = compare {
        let t = c.times.get(&(s.id, s.level)).copied().unwrap_or(0.0);
        row.append(&cell(&dur_compare(t), W_COMPARE, true, &[]));
        row.append(&change_cell(s.time, t, W_CHANGE));
    }
    if show_group {
        let g = sde::SKILLS.get(s.id).map(|x| x.market_group_name.clone()).unwrap_or_default();
        row.append(&cell(&g, W_GROUP, false, &["dim"]));
    }
    if cols.attributes {
        row.append(&cell(&s.attribute_title(), W_ATTRS, false, &["dim"]));
    }
    if cols.sp_hour {
        row.append(&cell(&fmt::int(s.sp_hour as i64), W_SPH, true, &["dim"]));
    }
    if cols.since_remap {
        row.append(&cell(&dur(s.last_remap), W_REMAP, true, &["dim"]));
    }
    if cols.finishes {
        row.append(&cell(&fmt::datetime(now + chrono::Duration::milliseconds(elapsed as i64)), W_FINISH, true, &["dim"]));
    }

    let actions = w::hbox(2);
    actions.set_size_request(W_ACTIONS, -1);
    actions.set_halign(gtk::Align::End);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    actions.append(&spacer);
    // offer the next level unless the skill is at V or that level is already planned
    let can_add = s.level < 5 && !queue.iter().any(|q| matches!(q, Item::Skill(o) if o.id == s.id && o.level == s.level + 1));
    if can_add {
        let next = s.level + 1;
        let add = w::button_icon("add", &format!("Level {}", roman(next)), Btn::Tonal);
        add.add_css_class("add-level");
        add.set_valign(gtk::Align::Center);
        add.set_tooltip_text(Some(&format!("Add next level: {} {}", s.name, roman(next))));
        let (pp, id, name) = (p.clone(), s.id, s.name.clone());
        add.connect_clicked(move |b| {
            let label = format!("{name} {}", roman(next));
            let pop = popover_menu(
                b,
                vec![
                    (format!("Add {label} right after this level"), {
                        let pp = pp.clone();
                        Box::new(move || add_next_level(&pp, i, id, next, true)) as Box<dyn Fn()>
                    }),
                    (format!("Add {label} at the end of the plan"), {
                        let pp = pp.clone();
                        Box::new(move || add_next_level(&pp, i, id, next, false))
                    }),
                ],
            );
            pop.popup();
        });
        actions.append(&add);
    }
    actions.append(&delete_button(p, i, Some(s)));
    row.append(&actions);
}

fn marker_cells(p: &P, row: &gtk::Box, i: usize, it: &Item, cols: Columns, show_group: bool, compare: Option<&Compare>) {
    let texts = w::hbox(8);
    texts.set_hexpand(true);
    let (icon, title, details) = match it {
        Item::Remap { .. } => ("tune", it.title(), String::new()),
        Item::Note { title, details, .. } => ("sticky_note_2", title.clone(), details.clone()),
        Item::Skill(_) => unreachable!(),
    };
    texts.append(&w::icon_sized(icon, 18));
    let t = w::line(&title, &["body-medium", "title-small"]);
    texts.append(&t);
    if !details.is_empty() {
        let d = w::line(&details.replace('\n', " · "), &["body-small", "dim"]);
        d.set_hexpand(true);
        d.set_tooltip_text(Some(&details));
        texts.append(&d);
    }
    row.append(&texts);
    // keep the action buttons in line with the skill rows
    let mut width = 0;
    if cols.time {
        width += W_TIME + 8;
    }
    if compare.is_some() {
        width += W_COMPARE + W_CHANGE + 16;
    }
    if show_group {
        width += W_GROUP + 8;
    }
    if cols.attributes {
        width += W_ATTRS + 8;
    }
    if cols.sp_hour {
        width += W_SPH + 8;
    }
    if cols.since_remap {
        width += W_REMAP + 8;
    }
    if cols.finishes {
        width += W_FINISH + 8;
    }
    if width > 8 {
        row.append(&cell("", width - 8, false, &[]));
    }
    let actions = w::hbox(2);
    actions.set_size_request(W_ACTIONS, -1);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    actions.append(&spacer);
    let edit = w::icon_button("edit", if matches!(it, Item::Remap { .. }) { "Edit remap" } else { "Edit note" });
    {
        let pp = p.clone();
        let is_remap = matches!(it, Item::Remap { .. });
        edit.connect_clicked(move |_| {
            if is_remap {
                dialogs::remap(&pp, Some(i));
            } else {
                dialogs::note(&pp, Some(i));
            }
        });
    }
    actions.append(&edit);
    actions.append(&delete_button(p, i, None));
    row.append(&actions);
}

/// Plans the next level of the skill at `index`, right after it or at the end of the plan.
fn add_next_level(p: &P, index: usize, id: i64, level: u8, right_after: bool) {
    p.edit(|pl| {
        let before = pl.queue.len();
        pl.plan_skill(id, level, None);
        if right_after && pl.queue.len() > before {
            let added: Vec<Item> = pl.queue[before..].to_vec();
            let mut items = pl.queue[..before].to_vec();
            let at = (index + 1).min(items.len());
            for (k, it) in added.into_iter().enumerate() {
                items.insert(at + k, it);
            }
            pl.rebuild(&items);
        }
    });
}

/// The remove button. Ctrl+click removes a skill level with everything that needs it; a plain click on a level
/// other levels need asks which.
fn delete_button(p: &P, i: usize, skill: Option<&SkillItem>) -> gtk::Button {
    let b = w::icon_button("delete", "Remove (Ctrl+click: with the skills that need it)");
    let ctrl = Rc::new(Cell::new(false));
    {
        let g = gtk::GestureClick::new();
        g.set_propagation_phase(gtk::PropagationPhase::Capture);
        let ctrl = ctrl.clone();
        g.connect_pressed(move |g, _, _, _| ctrl.set(g.current_event_state().contains(gdk::ModifierType::CONTROL_MASK)));
        b.add_controller(g);
    }
    let pp = p.clone();
    let title = skill.map(|s| s.title());
    b.connect_clicked(move |b| {
        let Some(title) = title.clone() else {
            pp.edit(|pl| pl.remove_item(i, false));
            return;
        };
        if ctrl.get() {
            pp.edit(|pl| pl.remove_item(i, true));
            return;
        }
        // how much a forced removal would take out
        let (len, gone) = {
            let st = pp.st.borrow();
            let mut trial = st.planner.clone();
            trial.remove_item(i, true);
            (st.planner.queue.len(), st.planner.queue.len() - trial.queue.len())
        };
        let _ = len;
        if gone <= 1 {
            pp.edit(|pl| pl.remove_item(i, false));
            return;
        }
        let others = gone - 1;
        let pop = popover_menu(
            b,
            vec![
                (format!("Remove {title} and the {others} level{} that need it", plural(others)), {
                    let pp = pp.clone();
                    Box::new(move || pp.edit(|pl| pl.remove_item(i, true))) as Box<dyn Fn()>
                }),
                (format!("Move {title} down to where it's needed"), {
                    let pp = pp.clone();
                    Box::new(move || pp.edit(|pl| pl.remove_item(i, false)))
                }),
            ],
        );
        pop.popup();
    });
    b
}

/// A small menu popover on `anchor`; it removes itself once closed.
pub(super) fn popover_menu(anchor: &impl IsA<gtk::Widget>, items: Vec<(String, Box<dyn Fn()>)>) -> gtk::Popover {
    let pop = gtk::Popover::new();
    pop.set_has_arrow(false);
    let bx = w::vbox(0);
    for (label, f) in items {
        let b = gtk::Button::with_label(&label);
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
    pop.set_parent(anchor);
    pop.connect_closed(|p| {
        let p = p.clone();
        glib::idle_add_local_once(move || p.unparent());
    });
    pop
}

/// Makes a plan row pickable (click), draggable and a drop target.
fn add_row_controllers(p: &P, sel: &Rc<Sel>, lrow: &gtk::ListBoxRow, row: &gtk::Box, i: usize) {
    let click_g = gtk::GestureClick::new();
    click_g.set_button(gdk::BUTTON_PRIMARY);
    {
        let (pp, s) = (p.clone(), sel.clone());
        click_g.connect_released(move |g, n, x, y| {
            // clicks on the row's buttons are theirs
            if let Some(w) = g.widget() {
                if let Some(target) = w.pick(x, y, gtk::PickFlags::DEFAULT) {
                    if target.ancestor(gtk::Button::static_type()).is_some() {
                        return;
                    }
                }
            }
            if n == 1 {
                click(&pp, &s, i, g.current_event_state());
            }
        });
    }
    lrow.add_controller(click_g);

    let drag = gtk::DragSource::new();
    drag.set_actions(gdk::DragAction::MOVE);
    drag.connect_prepare(move |_, _, _| Some(gdk::ContentProvider::for_value(&(i as u32).to_value())));
    {
        let row = row.clone();
        drag.connect_drag_begin(move |src, _| {
            let paint = gtk::WidgetPaintable::new(Some(&row));
            src.set_icon(Some(&paint), 0, 0);
        });
    }
    lrow.add_controller(drag);

    let drop = gtk::DropTarget::new(u32::static_type(), gdk::DragAction::MOVE);
    {
        let lrow2 = lrow.clone();
        drop.connect_motion(move |_, _, y| {
            let below = y > lrow2.height() as f64 / 2.0;
            lrow2.remove_css_class(if below { "drop-above" } else { "drop-below" });
            lrow2.add_css_class(if below { "drop-below" } else { "drop-above" });
            gdk::DragAction::MOVE
        });
    }
    {
        let lrow2 = lrow.clone();
        drop.connect_leave(move |_| {
            lrow2.remove_css_class("drop-above");
            lrow2.remove_css_class("drop-below");
        });
    }
    {
        let (pp, lrow2) = (p.clone(), lrow.clone());
        drop.connect_drop(move |_, v, _, y| {
            lrow2.remove_css_class("drop-above");
            lrow2.remove_css_class("drop-below");
            let Ok(from) = v.get::<u32>() else { return false };
            let below = y > lrow2.height() as f64 / 2.0;
            let pp = pp.clone();
            // after the drag finishes: the rows are rebuilt
            glib::idle_add_local_once(move || drop_on(&pp, from as usize, i, below));
            true
        });
    }
    lrow.add_controller(drop);
}
