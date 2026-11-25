//! Pieces the skill and ship browsers share: the pilot picker, the grouped catalogue list, the add-to-plan row and
//! the list of levels still to train.

use crate::ctx::Ctx;
use crate::widgets::{self as w, Btn};
use gtk::{glib, prelude::*};
use podstack_core::features::fitting::{total_time, PlannedLevel};
use podstack_core::features::plans::{self, PlanEntry};
use podstack_core::{fmt, training::roman, Character, Core};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) const CSS: &str = "
.browser-pane { background-color: @surface_container_low; border-radius: 24px; padding: 16px 12px 12px 12px; }
.browser-pane .browser-filters { padding: 0 4px 8px 4px; }
list.browser-list { background-color: transparent; }
list.browser-list > row { border-radius: 999px; margin: 1px 0; }
list.browser-list > row:selected { background-color: @secondary_container; color: @on_secondary_container; }
.browser-group { padding: 8px 12px; min-height: 32px; }
.browser-group .browser-group-name { font-size: 14px; font-weight: 500; letter-spacing: 0.1px; color: @on_surface; }
.browser-group .icon { color: @on_surface_variant; }
.browser-item { padding: 4px 16px 4px 44px; min-height: 36px; }
.browser-item.flat-item { padding-left: 16px; }
.browser-item .browser-item-name { font-size: 14px; }
.browser-hero { background-color: @surface_container_low; border-radius: 28px; padding: 24px; }
.browser-hero-icon { background-color: @primary_container; color: @on_primary_container; border-radius: 16px; min-width: 56px; min-height: 56px; }
.browser-info-chip { background-color: @surface_container_high; color: @on_surface_variant; border-radius: 8px; padding: 4px 12px; font-size: 13px; font-weight: 500; }
.browser-render { border-radius: 20px; background-color: @surface_container_highest; }
.browser-tree-row { min-height: 36px; padding: 2px 0; }
.browser-levels-grid > label { padding: 6px 0; }
.browser-levels-head { color: @on_surface_variant; font-size: 12px; font-weight: 500; letter-spacing: 0.5px; }
.browser-total { padding: 12px 16px 4px 16px; }
";

/// One remembered pilot for both browsers, so switching between them keeps it.
const PILOT_KEY: &str = "browser.pilot";

/// Characters to pick from, most SP first.
pub(crate) fn pilots(core: &Core) -> Vec<Arc<Character>> {
    let mut v: Vec<Arc<Character>> = core.characters().into_iter().filter(|c| c.skills.is_some() || c.has_basic_info()).collect();
    v.sort_by_key(|c| std::cmp::Reverse(c.total_sp()));
    v
}

/// The pilot picked earlier (None = "no pilot"); the first character when nothing was picked yet.
pub(crate) fn selected_pilot(ctx: &Ctx) -> Option<i64> {
    let list = pilots(ctx.core());
    match ctx.recall(PILOT_KEY).as_deref() {
        Some("none") => None,
        Some(id) => id.parse().ok().filter(|id| list.iter().any(|c| c.id == *id)).or_else(|| list.first().map(|c| c.id)),
        None => list.first().map(|c| c.id),
    }
}

/// A dropdown of characters with the chosen one's portrait. `on_change` gets the new pilot.
pub(crate) fn pilot_picker(ctx: &Ctx, on_change: impl Fn(Option<i64>) + 'static) -> gtk::Widget {
    let list = pilots(ctx.core());
    let selected = selected_pilot(ctx);
    let mut names: Vec<String> = vec!["No pilot".into()];
    names.extend(list.iter().map(|c| c.name()));
    let index = selected.and_then(|id| list.iter().position(|c| c.id == id)).map(|i| i as u32 + 1).unwrap_or(0);
    let dd = dropdown(&names, index);
    dd.set_hexpand(true);
    dd.set_tooltip_text(Some("Compare against this character's skills"));

    let row = w::hbox(12);
    row.set_margin_start(4);
    row.set_margin_end(4);
    let portrait = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let show_portrait = {
        let portrait = portrait.clone();
        move |id: Option<i64>| {
            while let Some(c) = portrait.first_child() {
                portrait.remove(&c);
            }
            match id {
                Some(id) => portrait.append(&w::avatar(&podstack_core::config::character_portrait(id, 64), 40)),
                None => {
                    let i = w::icon("person_off");
                    i.add_css_class("dim");
                    i.set_size_request(40, 40);
                    portrait.append(&i);
                }
            }
        }
    };
    show_portrait(selected);
    row.append(&portrait);
    row.append(&dd);

    let ctx = ctx.clone();
    let ids: Vec<i64> = list.iter().map(|c| c.id).collect();
    dd.connect_selected_notify(move |dd| {
        let id = (dd.selected() as usize).checked_sub(1).and_then(|i| ids.get(i).copied());
        ctx.remember(PILOT_KEY, id.map(|i| i.to_string()).unwrap_or_else(|| "none".into()));
        show_portrait(id);
        on_change(id);
    });
    row.upcast()
}

/// A row in a [`Catalogue`].
pub(crate) enum Row {
    /// A collapsible group heading.
    Group { key: String, title: String, count: String, open: bool },
    /// An entry (skill / ship id) with its widget content.
    Item { id: i64, content: gtk::Widget },
}

/// A scrolling list of collapsible groups of items, with one item selected. The owner rebuilds its rows with
/// [`Catalogue::set_rows`] (only open groups need item rows, so big catalogues stay light).
pub(crate) struct Catalogue {
    pub list: gtk::ListBox,
    pub scroller: gtk::ScrolledWindow,
    keys: Rc<RefCell<Vec<RowKey>>>,
}

#[derive(Clone)]
enum RowKey {
    Group(String),
    Item(i64),
}

impl Catalogue {
    pub fn new(on_group: impl Fn(&str) + 'static, on_item: impl Fn(i64) + 'static) -> Self {
        let list = gtk::ListBox::new();
        list.add_css_class("browser-list");
        list.set_selection_mode(gtk::SelectionMode::Single);
        list.set_activate_on_single_click(true);
        let scroller = w::scrolled(&list);
        let keys: Rc<RefCell<Vec<RowKey>>> = Rc::default();
        let k = keys.clone();
        list.connect_row_activated(move |list, row| {
            let key = k.borrow().get(row.index() as usize).cloned();
            match key {
                Some(RowKey::Group(g)) => {
                    list.unselect_row(row);
                    on_group(&g)
                }
                Some(RowKey::Item(id)) => on_item(id),
                None => {}
            }
        });
        Catalogue { list, scroller, keys }
    }

    pub fn set_rows(&self, rows: Vec<Row>, selected: Option<i64>, flat: bool) {
        // keep the scroll position across the rebuild
        let adj = self.scroller.vadjustment();
        let pos = adj.value();
        self.list.remove_all();
        let mut keys = Vec::with_capacity(rows.len());
        let mut select = None;
        for r in rows {
            match r {
                Row::Group { key, title, count, open } => {
                    let b = w::hbox(8);
                    b.add_css_class("browser-group");
                    b.append(&w::icon_sized(if open { "expand_more" } else { "chevron_right" }, 20));
                    let t = w::line(&title, &["browser-group-name"]);
                    t.set_hexpand(true);
                    b.append(&t);
                    let n = gtk::Label::new(Some(&count));
                    for c in ["label-medium", "dim", "numeric"] {
                        n.add_css_class(c);
                    }
                    b.append(&n);
                    let row = gtk::ListBoxRow::new();
                    row.set_child(Some(&b));
                    row.set_selectable(false);
                    row.update_property(&[gtk::accessible::Property::Label(&title)]);
                    self.list.append(&row);
                    keys.push(RowKey::Group(key));
                }
                Row::Item { id, content } => {
                    content.add_css_class("browser-item");
                    if flat {
                        content.add_css_class("flat-item");
                    }
                    let row = gtk::ListBoxRow::new();
                    row.set_child(Some(&content));
                    self.list.append(&row);
                    if Some(id) == selected {
                        select = Some(row);
                    }
                    keys.push(RowKey::Item(id));
                }
            }
        }
        *self.keys.borrow_mut() = keys;
        if let Some(r) = select {
            self.list.select_row(Some(&r));
        }
        glib::idle_add_local_once(move || adj.set_value(pos));
    }
}

/// An item row's content: optional leading widget, name, trailing widget.
pub(crate) fn item_content(leading: Option<gtk::Widget>, name: &str, trailing: Option<gtk::Widget>) -> gtk::Widget {
    let b = w::hbox(12);
    if let Some(l) = leading {
        l.set_valign(gtk::Align::Center);
        b.append(&l);
    }
    let n = w::line(name, &["browser-item-name"]);
    n.set_hexpand(true);
    b.append(&n);
    if let Some(t) = trailing {
        t.set_valign(gtk::Align::Center);
        b.append(&t);
    }
    b.upcast()
}

/// Whether a catalogue search matches `name`. Like the old browser, "|" separates alternatives ("drone|gunnery").
pub(crate) fn search_matches(query: &str, name: &str) -> bool {
    let name = name.to_lowercase();
    query.split('|').map(str::trim).filter(|q| !q.is_empty()).any(|q| name.contains(q))
}

/// A small tonal info chip (attributes, rank, SP/hour).
pub(crate) fn info_chip(t: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.add_css_class("browser-info-chip");
    l.set_valign(gtk::Align::Center);
    l
}

/// A row of a few small widgets (attribute chips, status pills).
pub(crate) fn pill_row(children: Vec<gtk::Widget>) -> gtk::Box {
    let b = w::hbox(8);
    for c in children {
        c.set_valign(gtk::Align::Center);
        b.append(&c);
    }
    b
}

/// A wrapping row of small widgets.
pub(crate) fn chip_row(children: Vec<gtk::Widget>) -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_column_spacing(8);
    f.set_row_spacing(8);
    f.set_max_children_per_line(30);
    f.set_valign(gtk::Align::Start);
    // a filling FlowBox spreads spare width between its children; keep them packed at the start
    f.set_halign(gtk::Align::Start);
    for c in children {
        c.set_halign(gtk::Align::Start);
        f.append(&c);
    }
    f
}

/// "Skill Name IV" list of levels to train with their times, and the running total.
pub(crate) fn levels_list(levels: &[PlannedLevel]) -> gtk::Widget {
    levels_list_noted(levels, &[])
}

/// [`levels_list`] with a note under each level (`notes[i]` for `levels[i]`; missing or empty means none).
pub(crate) fn levels_list_noted(levels: &[PlannedLevel], notes: &[String]) -> gtk::Widget {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("m3-list");
    let mut elapsed = chrono::Duration::zero();
    for (i, l) in levels.iter().enumerate() {
        elapsed += l.time;
        let times = w::vbox(0);
        let t = w::line(&fmt::duration(l.time, 3), &["body-medium", "numeric"]);
        t.set_xalign(1.0);
        times.append(&t);
        let d = w::line(&format!("done in {}", fmt::duration(elapsed, 2)), &["body-small", "dim", "numeric"]);
        d.set_xalign(1.0);
        times.append(&d);
        let note = notes.get(i).map(String::as_str).filter(|n| !n.is_empty());
        let row = w::list_row(None, &format!("{} {}", l.name(), roman(l.level)), note, Some(times.upcast_ref()));
        row.set_margin_start(0);
        list.append(&row);
    }
    list.upcast()
}

/// Appends skill levels to a plan, skipping levels it already has; `note` goes first when anything is added.
/// Returns how many levels were added.
fn append_levels(core: &Core, character_id: i64, plan_id: &str, note: Option<&str>, levels: &[(i64, u8)]) -> usize {
    let Some(mut plan) = plans::get(character_id, plan_id) else { return 0 };
    let new: Vec<(i64, u8)> = levels
        .iter()
        .copied()
        .filter(|&(id, l)| !plan.queue.iter().any(|e| matches!(e, PlanEntry::Skill { skill_id, level } if *skill_id == id && *level == l)))
        .collect();
    if new.is_empty() {
        return 0;
    }
    if let Some(n) = note {
        plan.queue.push(PlanEntry::Note { text: n.to_string() });
    }
    plan.queue.extend(new.iter().map(|&(skill_id, level)| PlanEntry::Skill { skill_id, level }));
    plans::save_plan(core, character_id, plan);
    new.len()
}

/// "Add to plan": a dropdown of the character's plans (first entry: a new plan named `new_name`) and a button
/// that appends `levels` (already in training order). `note` is put ahead of them in an existing plan.
pub(crate) fn add_to_plan_row(ctx: &Ctx, character_id: i64, new_name: &str, note: Option<String>, levels: &[PlannedLevel]) -> gtk::Widget {
    let row = w::hbox(12);
    row.set_margin_top(8);
    let existing = plans::plans_for(character_id);
    let key = format!("browser.plan.{character_id}");
    let remembered = ctx.recall(&key);
    let mut names = vec![format!("New plan \u{201c}{new_name}\u{201d}")];
    names.extend(existing.iter().map(|p| p.name.clone()));
    let index = remembered.and_then(|id| existing.iter().position(|p| p.id == id)).map(|i| i as u32 + 1).unwrap_or(0);
    let dd = dropdown(&names, index);
    dd.set_hexpand(true);
    row.append(&dd);

    let n = levels.len();
    let add = w::button_icon("playlist_add", &format!("Add {n} level{}", if n == 1 { "" } else { "s" }), Btn::Filled);
    add.set_tooltip_text(Some(&format!("Total {}", fmt::duration(total_time(levels), 3))));
    row.append(&add);

    let ctx = ctx.clone();
    let new_name = new_name.to_string();
    let wanted: Vec<(i64, u8)> = levels.iter().map(|l| (l.skill_id, l.level)).collect();
    let ids: Vec<(String, String)> = existing.iter().map(|p| (p.id.clone(), p.name.clone())).collect();
    add.connect_clicked(move |_| {
        let core = ctx.core();
        let (plan_id, plan_name, note) = match (dd.selected() as usize).checked_sub(1).and_then(|i| ids.get(i)) {
            Some((id, name)) => (id.clone(), name.clone(), note.as_deref()),
            None => (plans::create(core, character_id, &new_name), new_name.clone(), None),
        };
        let added = append_levels(core, character_id, &plan_id, note, &wanted);
        ctx.remember(&key, plan_id);
        ctx.toast(&match added {
            0 => format!("\u{201c}{plan_name}\u{201d} already has these skills"),
            1 => format!("Added 1 skill level to \u{201c}{plan_name}\u{201d}"),
            n => format!("Added {n} skill levels to \u{201c}{plan_name}\u{201d}"),
        });
    });
    row.upcast()
}

/// Removes every child of a box.
pub(crate) fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// A dropdown whose button ellipsizes long entries (plan and character names), so it never forces the page wider.
pub(crate) fn dropdown(items: &[String], selected: u32) -> gtk::DropDown {
    let refs: Vec<&str> = items.iter().map(String::as_str).collect();
    let dd = w::dropdown(&refs, selected);
    let factory = |ellipsize: bool| {
        let f = gtk::SignalListItemFactory::new();
        f.connect_setup(move |_, item| {
            let l = gtk::Label::new(None);
            l.set_xalign(0.0);
            if ellipsize {
                l.set_ellipsize(pango::EllipsizeMode::End);
                l.set_width_chars(6);
            }
            item.downcast_ref::<gtk::ListItem>().unwrap().set_child(Some(&l));
        });
        f.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let text = item.item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string()).unwrap_or_default();
            if let Some(l) = item.child().and_downcast::<gtk::Label>() {
                l.set_tooltip_text(Some(&text));
                l.set_text(&text);
            }
        });
        f
    };
    dd.set_factory(Some(&factory(true)));
    dd.set_list_factory(Some(&factory(false)));
    dd
}

/// Window width below which the catalogue goes above the details instead of beside them.
const SIDE_BY_SIDE_MIN: i32 = 1180;

/// The browser layout: a catalogue pane and a scrolled detail column, side by side on wide windows and stacked on
/// narrow ones (the drawer takes 280px, so a tiled window would otherwise squeeze the details).
pub(crate) fn two_pane(ctx: &Ctx, left: &impl IsA<gtk::Widget>, detail: &gtk::Box) -> gtk::Box {
    let body = w::hbox(24);
    body.set_margin_start(24);
    body.set_margin_end(24);
    body.set_margin_top(8);
    body.set_margin_bottom(24);
    body.set_vexpand(true);
    let left = left.as_ref().clone();
    left.set_hexpand(false);
    body.append(&left);
    let scroll = w::scrolled(detail);
    scroll.set_hexpand(true);
    body.append(&scroll);

    let window = ctx.window();
    let apply = {
        let (body, left) = (body.clone(), left.clone());
        move |wide: bool| {
            if wide {
                body.set_orientation(gtk::Orientation::Horizontal);
                left.set_size_request(340, -1);
                left.set_vexpand(true);
            } else {
                body.set_orientation(gtk::Orientation::Vertical);
                left.set_size_request(-1, 380);
                left.set_vexpand(false);
            }
        }
    };
    let wide = std::rc::Rc::new(std::cell::Cell::new(window.width() == 0 || window.width() >= SIDE_BY_SIDE_MIN));
    apply(wide.get());
    // GTK4 has no breakpoints without libadwaita: check the window width each frame (a cheap comparison)
    body.add_tick_callback(move |_, _| {
        let w = window.width();
        if w > 0 && (w >= SIDE_BY_SIDE_MIN) != wide.get() {
            wide.set(w >= SIDE_BY_SIDE_MIN);
            apply(wide.get());
        }
        glib::ControlFlow::Continue
    });
    body
}
