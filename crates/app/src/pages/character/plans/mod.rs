//! Character tab: skill plans.
//!
//! Left: the character's plans in priority order (switch each in or out of the combined training queue, move them
//! up and down) and the skill catalogue to add skills from. Right: the toolbar, the optional fit planner and
//! implant profiler panels, and the open plan's editor (or the combined training queue).
//!
//! The page is static: it keeps the open plan, the selection and the scroll position itself and redraws its own
//! widgets when the plans document or the character changes.

mod catalogue;
mod dialogs;
mod editor;
mod io;
pub mod panels;
mod profiler;
mod skillbooks;

use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Btn};
use gtk::{glib, prelude::*};
use podstack_core::features::plan_tools::{accelerator, profile::Profile, sim, Item, PlanBase, Planner};
use podstack_core::features::plans::{self, SkillPlan};
use podstack_core::{fmt, Event};
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

const CSS: &str = r#"
.plans-pane-left { min-width: 320px; }
.plans-list row { border-radius: 12px; margin: 2px 8px; }
.plans-list row.off .plan-name { color: @on_surface_variant; }
.plan-priority { min-width: 28px; min-height: 28px; border-radius: 999px; background-color: @surface_container_highest; color: @on_surface_variant; font-size: 12px; font-weight: 600; }
.plans-list row:selected .plan-priority { background-color: @primary; color: @on_primary; }
.plans-toolbar { background-color: @surface_container_low; border-radius: 28px; padding: 8px 12px; }
.plans-toolbar button { min-height: 36px; padding: 0 14px; }
.plans-toolbar button.icon-button { padding: 0; min-width: 36px; }
.plans-toolbar-sep { min-width: 1px; background-color: @outline_variant; margin: 6px 4px; }
button.accel-toggle { border: 1px solid @outline; padding: 0 12px; color: @on_surface_variant; }
button.accel-toggle.on { background-color: @tertiary_container; color: @on_tertiary_container; border-color: @tertiary_container; }
.plan-head-row { padding: 6px 12px; border-bottom: 1px solid @outline_variant; color: @on_surface_variant; font-size: 12px; font-weight: 500; }
.plan-foot-row { padding: 10px 12px; border-top: 1px solid @outline_variant; color: @on_surface_variant; font-size: 13px; font-weight: 500; }
.plan-rows row { border-bottom: 1px solid alpha(@outline_variant, 0.5); }
.plan-rows row:hover { background-color: alpha(@on_surface, 0.05); }
.plan-row { padding: 4px 8px 4px 4px; min-height: 40px; }
.plan-rows row.picked { background-color: alpha(@secondary_container, 0.85); color: @on_secondary_container; }
.plan-rows row.drop-above { box-shadow: inset 0 2px 0 0 @primary; }
.plan-rows row.drop-below { box-shadow: inset 0 -2px 0 0 @primary; }
.plan-rows row.marker { background-color: alpha(@surface_container_highest, 0.6); }
.plan-row .drag-handle { color: @on_surface_variant; opacity: 0.6; }
.plan-row button.icon-button { min-width: 32px; min-height: 32px; }
.plan-row button.add-level { min-height: 28px; padding: 0 10px 0 6px; font-size: 12px; }
.plan-selection-bar { background-color: @secondary_container; color: @on_secondary_container; border-radius: 16px; padding: 4px 8px 4px 16px; margin: 8px 8px 0 8px; }
.plan-selection-bar button { color: @on_secondary_container; min-height: 36px; padding: 0 12px; }
.skillbook-strip { background-color: @surface_container; border-radius: 12px; padding: 8px 8px 8px 16px; margin: 0 8px 8px 8px; }
.skillbook-strip.done { padding: 10px 16px; }
.skillbook-best { color: @success; font-weight: 600; }
.catalogue-group > title { padding: 8px 12px; border-radius: 12px; }
.catalogue-group > title:hover { background-color: alpha(@on_surface, 0.08); }
.catalogue-group > title label { font-size: 12px; font-weight: 600; letter-spacing: 0.8px; color: @on_surface_variant; }
.catalogue-skill { padding: 8px 12px 8px 24px; border-radius: 12px; }
.catalogue-skill:hover { background-color: alpha(@on_surface, 0.08); }
.catalogue-flat .catalogue-skill { padding-left: 12px; }
.profiler-attr { background-color: @surface_container; border-radius: 12px; padding: 8px 14px; min-width: 96px; }
.profiler-row { padding: 8px 12px; border-radius: 12px; }
.profiler-row:hover { background-color: alpha(@on_surface, 0.08); }
.profiler-row.selected { background-color: @secondary_container; color: @on_secondary_container; }
.profiler-active { background-color: @tertiary_container; color: @on_tertiary_container; border-radius: 12px; padding: 8px 12px; }
.remap-second { background-color: @primary_container; color: @on_primary_container; border-radius: 16px; padding: 12px 16px; }
.remap-grid label.head { color: @on_surface_variant; font-size: 12px; font-weight: 500; }
.remap-grid scale { min-width: 220px; }
.paste-text { min-height: 220px; }
popover button.menu-item { border-radius: 0; min-height: 44px; padding: 0 16px; color: @on_surface; font-weight: 400; }
popover button.menu-item:hover { background-color: alpha(@on_surface, 0.08); }
"#;

/// Which optional columns the plan editor shows.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
struct Columns {
    pub time: bool,
    pub group: bool,
    pub attributes: bool,
    pub sp_hour: bool,
    pub since_remap: bool,
    pub finishes: bool,
}

impl Default for Columns {
    fn default() -> Self {
        Columns { time: true, group: true, attributes: false, sp_hour: false, since_remap: false, finishes: false }
    }
}

const COLUMNS_SETTING: &str = "plans.columns";

/// Market prices of a skillbook: lowest sell in The Forge, EVE's average.
#[derive(Clone, Copy, Debug, Default)]
struct Price {
    pub forge: Option<f64>,
    pub average: Option<f64>,
}

struct State {
    /// the open plan; None = the combined training queue
    pub open: Option<String>,
    pub name: String,
    pub planner: Planner,
    /// positions picked in the editor
    pub selection: Vec<usize>,
    pub columns: Columns,
    pub profiler: Option<profiler::ProfilerState>,
    pub fit_open: bool,
    pub books_expanded: bool,
    pub prices: HashMap<i64, Price>,
    pub prices_pending: HashSet<i64>,
    pub prices_error: bool,
}

impl State {
    /// The setup the implant profiler compares against, if any.
    pub fn compare(&self) -> Option<Profile> {
        self.profiler.as_ref().and_then(|p| p.profile())
    }
}

struct Plans {
    pub ctx: Ctx,
    pub id: i64,
    pub st: RefCell<State>,
    list: gtk::ListBox,
    merge: gtk::Button,
    right: gtk::Box,
    right_scroll: gtk::ScrolledWindow,
    toolbar_slot: gtk::Box,
    fit_slot: gtk::Box,
    profiler_slot: gtk::Box,
    editor_slot: gtk::Box,
    profiler_panel: RefCell<Option<Rc<dyn Fn()>>>,
    pub catalogue_hint: gtk::Label,
    fit_panel: RefCell<Option<gtk::Widget>>,
    /// set while the plan list is filled, so its switches don't save
    filling: Cell<bool>,
    /// Doc events caused by our own saves, which need no reload
    own_saves: Cell<u32>,
    refresh_pending: Cell<bool>,
}

type P = Rc<Plans>;
/// What the fit planner calls with the skills to add.
type AddToPlan = Rc<dyn Fn(Vec<(i64, u8)>)>;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    crate::theme::add_css(CSS);
    accelerator::refresh(ctx.core(), id);
    if ctx.core().character(id).is_none() {
        return Page::new("Plans", w::empty_state("person_off", "Character not found", ""));
    }

    let list = gtk::ListBox::new();
    list.add_css_class("plans-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    let merge = w::button_icon("call_merge", "Merge", Btn::Text);
    let right = w::vbox(16);
    right.set_margin_end(4);
    let (toolbar_slot, fit_slot, profiler_slot, editor_slot) = (w::vbox(0), w::vbox(0), w::vbox(0), w::vbox(0));
    for s in [&toolbar_slot, &fit_slot, &profiler_slot, &editor_slot] {
        right.append(s);
    }
    let right_scroll = w::scrolled(&right);
    right_scroll.set_hexpand(true);
    let catalogue_hint = w::text("", &["body-small", "dim"]);

    let open = ctx.recall(&format!("plans.{id}.open")).filter(|s| !s.is_empty());
    let p: P = Rc::new(Plans {
        ctx: ctx.clone(),
        id,
        st: RefCell::new(State {
            open,
            name: String::new(),
            planner: Planner::new(&PlanBase::default()),
            selection: vec![],
            columns: ctx.core().setting(COLUMNS_SETTING),
            profiler: None,
            fit_open: false,
            books_expanded: false,
            prices: HashMap::new(),
            prices_pending: HashSet::new(),
            prices_error: false,
        }),
        list: list.clone(),
        merge: merge.clone(),
        right: right.clone(),
        right_scroll: right_scroll.clone(),
        toolbar_slot,
        fit_slot,
        profiler_slot,
        editor_slot,
        profiler_panel: RefCell::new(None),
        catalogue_hint: catalogue_hint.clone(),
        fit_panel: RefCell::new(None),
        filling: Cell::new(false),
        own_saves: Cell::new(0),
        refresh_pending: Cell::new(false),
    });
    // open on the training queue when there are plans to combine, unless a plan was open last time
    p.load();

    // left pane: plans and the skill catalogue
    let left = w::vbox(16);
    left.add_css_class("plans-pane-left");
    left.set_size_request(340, -1);
    left.set_hexpand(false);
    {
        let pp = p.clone();
        merge.connect_clicked(move |_| dialogs::merge(&pp));
    }
    let plans_card = w::card_with("Plans", None, &[merge.clone().upcast()], &list);
    plans_card.add_css_class("flush");
    plans_card.set_margin_top(0);
    if let Some(head) = plans_card.first_child() {
        head.set_margin_start(16);
        head.set_margin_end(8);
        head.set_margin_top(12);
    }
    list.set_margin_bottom(8);
    {
        let pp = p.clone();
        list.connect_row_activated(move |_, row| {
            let name = row.widget_name();
            let target = if name == "__queue__" { None } else { Some(name.to_string()) };
            if pp.st.borrow().open != target {
                let pp = pp.clone();
                // after the click: the list is rebuilt
                glib::idle_add_local_once(move || pp.open(target));
            }
        });
    }
    left.append(&plans_card);
    let cat = catalogue::card(&p);
    cat.set_vexpand(true);
    left.append(&cat);

    let root = w::hbox(16);
    root.set_vexpand(true);
    root.append(&left);
    root.append(&right_scroll);

    {
        let pp = p.clone();
        ctx.on_event(&root, move |e| match e {
            Event::Doc(d) if d == plans::DOC => {
                if pp.own_saves.get() > 0 {
                    pp.own_saves.set(pp.own_saves.get() - 1);
                    pp.render_list();
                } else {
                    pp.schedule_refresh();
                }
            }
            Event::CharacterUpdated(cid) if *cid == pp.id => pp.schedule_refresh(),
            _ => {}
        });
    }
    p.render();
    Page::new("Plans", root).no_scroll().static_page()
}

impl Plans {
    pub fn core(&self) -> &std::sync::Arc<podstack_core::Core> {
        self.ctx.core()
    }

    /// The character's plan base right now.
    pub fn base(&self) -> PlanBase {
        match self.core().character(self.id) {
            Some(c) => PlanBase::of(&c, chrono::Utc::now(), true),
            None => PlanBase::default(),
        }
    }

    pub fn character(&self) -> Option<std::sync::Arc<podstack_core::Character>> {
        self.core().character(self.id)
    }

    pub fn plans(&self) -> Vec<SkillPlan> {
        plans::plans_for(self.id)
    }

    /// Reads the open plan (or builds the training queue) and simulates it.
    fn load(&self) {
        let base = self.base();
        let mut st = self.st.borrow_mut();
        if let Some(pid) = st.open.clone() {
            match plans::get(self.id, &pid) {
                Some(plan) => {
                    st.planner = Planner::with_items(&base, &sim::items_of(&plan.queue));
                    st.name = plan.name;
                    return;
                }
                None => st.open = None,
            }
        }
        st.planner = sim::training_queue(&base, &self.plans());
        st.name = "Training Queue".into();
    }

    /// Opens a plan (None = the training queue).
    pub fn open(self: &Rc<Self>, plan: Option<String>) {
        {
            let mut st = self.st.borrow_mut();
            st.open = plan.clone();
            st.selection.clear();
        }
        self.ctx.remember(&format!("plans.{}.open", self.id), plan.unwrap_or_default());
        self.load();
        self.right_scroll.vadjustment().set_value(0.0);
        self.render();
    }

    /// Debounced reload after the character or the plans changed elsewhere.
    fn schedule_refresh(self: &Rc<Self>) {
        if self.refresh_pending.replace(true) {
            return;
        }
        let p = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            p.refresh_pending.set(false);
            if p.right.root().is_none() {
                return;
            }
            accelerator::refresh(p.core(), p.id);
            let before = {
                let st = p.st.borrow();
                (st.planner.entries(), st.open.clone())
            };
            p.load();
            let st = p.st.borrow();
            if (st.planner.entries(), st.open.clone()) != before {
                drop(st);
                p.st.borrow_mut().selection.clear();
            } else {
                drop(st);
            }
            p.render();
        });
    }

    /// Changes the open plan, saves it and redraws. Does nothing on the training queue.
    pub fn edit(self: &Rc<Self>, f: impl FnOnce(&mut Planner)) {
        {
            let mut st = self.st.borrow_mut();
            if st.open.is_none() {
                return;
            }
            f(&mut st.planner);
            st.selection.clear();
        }
        self.save();
        self.render();
    }

    /// Saves the open plan's items (and name).
    pub fn save(&self) {
        let st = self.st.borrow();
        let Some(pid) = st.open.clone() else { return };
        let Some(mut plan) = plans::get(self.id, &pid) else { return };
        plan.queue = st.planner.entries();
        plan.name = st.name.clone();
        drop(st);
        self.own_saves.set(self.own_saves.get() + 1);
        plans::save_plan(self.core(), self.id, plan);
    }

    /// Creates a plan holding `planner`'s items and opens it.
    pub fn create_plan(self: &Rc<Self>, name: &str, items: &[Item]) -> String {
        let pid = plans::create(self.core(), self.id, name);
        self.own_saves.set(self.own_saves.get() + 1);
        if let Some(mut plan) = plans::get(self.id, &pid) {
            plan.queue = items.iter().map(Item::to_entry).collect();
            self.own_saves.set(self.own_saves.get() + 1);
            plans::save_plan(self.core(), self.id, plan);
        }
        self.open(Some(pid.clone()));
        pid
    }

    pub fn render(self: &Rc<Self>) {
        self.render_list();
        self.render_right();
    }

    /// The plan list on the left.
    pub fn render_list(self: &Rc<Self>) {
        self.filling.set(true);
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let all = self.plans();
        let base = self.base();
        self.merge.set_visible(all.len() > 1);
        let open = self.st.borrow().open.clone();

        let queue_time = sim::training_queue(&base, &all).time;
        let on = all.iter().filter(|p| p.enabled).count();
        let meta = format!(
            "{on} of {} plan{} on{}",
            all.len(),
            if all.len() == 1 { "" } else { "s" },
            if all.is_empty() { String::new() } else { format!(" · {}", dur(queue_time)) }
        );
        let qrow = w::list_row(Some(&w::icon("playlist_play").upcast()), "Training Queue", Some(&meta), None);
        let row = gtk::ListBoxRow::new();
        row.set_widget_name("__queue__");
        row.set_child(Some(&qrow));
        self.list.append(&row);
        if open.is_none() {
            self.list.select_row(Some(&row));
        }

        if all.is_empty() {
            let e = w::text("No plans yet. Use New, or import a ship fitting or your EVE skill queue.", &["body-medium", "dim"]);
            e.set_margin_start(16);
            e.set_margin_end(16);
            e.set_margin_top(8);
            e.set_margin_bottom(8);
            let r = gtk::ListBoxRow::new();
            r.set_activatable(false);
            r.set_selectable(false);
            r.set_child(Some(&e));
            self.list.append(&r);
        }

        let n = all.len();
        let ids: Vec<String> = all.iter().map(|p| p.id.clone()).collect();
        for (i, plan) in all.iter().enumerate() {
            let (count, time) = sim::plan_summary(&base, &plan.queue);
            let row = w::hbox(12);
            row.add_css_class("list-item");
            row.set_margin_end(0);
            let prio = gtk::Label::new(Some(&(i + 1).to_string()));
            prio.add_css_class("plan-priority");
            prio.set_valign(gtk::Align::Center);
            row.append(&prio);
            let texts = w::vbox(2);
            texts.set_hexpand(true);
            texts.set_valign(gtk::Align::Center);
            let name = w::line(&plan.name, &["headline", "plan-name"]);
            name.set_tooltip_text(Some(&plan.name));
            texts.append(&name);
            let meta = if count > 0 { format!("{count} skill{} · {}", plural(count), dur(time)) } else { "No skills".into() };
            texts.append(&w::line(&meta, &["supporting"]));
            row.append(&texts);

            let up = w::icon_button("keyboard_arrow_up", "Higher priority");
            up.set_sensitive(i > 0);
            let down = w::icon_button("keyboard_arrow_down", "Lower priority");
            down.set_sensitive(i + 1 < n);
            for (b, dir) in [(&up, -1i32), (&down, 1i32)] {
                let pp = self.clone();
                let ids = ids.clone();
                b.connect_clicked(move |_| {
                    let mut ids = ids.clone();
                    let to = i as i32 + dir;
                    if to < 0 || to as usize >= ids.len() {
                        return;
                    }
                    let it = ids.remove(i);
                    ids.insert(to as usize, it);
                    plans::set_order(pp.core(), pp.id, &ids);
                });
            }
            row.append(&up);
            row.append(&down);
            let sw = gtk::Switch::new();
            sw.set_active(plan.enabled);
            sw.set_valign(gtk::Align::Center);
            sw.set_tooltip_text(Some(if plan.enabled { "In the training queue" } else { "Not in the training queue" }));
            {
                let pp = self.clone();
                let pid = plan.id.clone();
                sw.connect_active_notify(move |s| {
                    if !pp.filling.get() {
                        plans::set_enabled(pp.core(), pp.id, &pid, s.is_active());
                    }
                });
            }
            row.append(&sw);

            let lrow = gtk::ListBoxRow::new();
            lrow.set_widget_name(&plan.id);
            if !plan.enabled {
                lrow.add_css_class("off");
            }
            lrow.set_child(Some(&row));
            self.list.append(&lrow);
            if open.as_deref() == Some(plan.id.as_str()) {
                self.list.select_row(Some(&lrow));
            }
        }
        self.catalogue_hint.set_text(if open.is_some() { "Click a skill to add it to the plan." } else { "Open a plan to add skills." });
        self.filling.set(false);
    }

    /// The toolbar, panels and editor on the right. The fit planner and profiler panels are kept while open, so
    /// their inputs survive.
    pub fn render_right(self: &Rc<Self>) {
        clear(&self.toolbar_slot);
        self.toolbar_slot.append(&self.toolbar());
        if self.st.borrow().fit_open {
            if self.fit_slot.first_child().is_none() {
                self.fit_slot.append(&self.fit_card());
            }
        } else {
            clear(&self.fit_slot);
        }
        if self.st.borrow().profiler.is_some() {
            let existing = self.profiler_panel.borrow().clone();
            match existing {
                Some(refresh) => refresh(),
                None => {
                    let (panel, refresh) = profiler::panel(self);
                    self.profiler_slot.append(&panel);
                    *self.profiler_panel.borrow_mut() = Some(refresh);
                }
            }
        } else {
            clear(&self.profiler_slot);
            *self.profiler_panel.borrow_mut() = None;
        }
        self.render_editor();
    }

    /// Only the plan editor (or training queue) card, keeping the scroll position.
    pub fn render_editor(self: &Rc<Self>) {
        let adj = self.right_scroll.vadjustment();
        let pos = adj.value();
        clear(&self.editor_slot);
        self.editor_slot.append(&editor::card(self));
        if pos > 0.0 {
            let adj = adj.clone();
            glib::idle_add_local_once(move || adj.set_value(pos.min((adj.upper() - adj.page_size()).max(0.0))));
        }
    }

    fn fit_card(self: &Rc<Self>) -> gtk::Widget {
        let panel = self
            .fit_panel
            .borrow_mut()
            .get_or_insert_with(|| {
                let pp = self.clone();
                let add: AddToPlan = Rc::new(move |skills| pp.add_from_fit(skills));
                crate::pages::fit_planner::panel(&self.ctx, self.id, add)
            })
            .clone();
        if let Some(parent) = panel.parent() {
            if let Some(b) = parent.downcast_ref::<gtk::Box>() {
                b.remove(&panel);
            }
        }
        let close = w::icon_button("close", "Close the fit planner");
        {
            let pp = self.clone();
            close.connect_clicked(move |_| {
                pp.st.borrow_mut().fit_open = false;
                pp.render_right();
            });
        }
        let sub = if self.st.borrow().open.is_some() { "Skills you add go into the open plan" } else { "Skills you add go into a new plan" };
        w::card_with("Fit planner", Some(sub), &[close.upcast()], &panel).upcast()
    }

    /// The fit planner's skills: into the open plan, or a new one when the training queue is showing.
    fn add_from_fit(self: &Rc<Self>, skills: Vec<(i64, u8)>) {
        if skills.is_empty() {
            return;
        }
        if self.st.borrow().open.is_none() {
            let mut p = Planner::new(&self.base());
            p.add_note("Fit", "Added from the fit planner");
            for (id, lvl) in &skills {
                p.plan_skill(*id, *lvl, None);
            }
            if p.skill_count() == 0 {
                self.ctx.toast("This character already has every skill for it.");
                return;
            }
            self.create_plan("Fit skills", &p.queue);
            self.ctx.toast(&format!("Added {} skill levels to a new plan", p.skill_count()));
            return;
        }
        let before = self.st.borrow().planner.skill_count();
        self.edit(|p| {
            for (id, lvl) in &skills {
                p.plan_skill(*id, *lvl, None);
            }
        });
        let added = self.st.borrow().planner.skill_count() - before;
        self.ctx.toast(&if added == 0 { "The plan already has every skill for it.".to_string() } else { format!("Added {added} skill level{} to the plan", plural(added)) });
    }

    fn toolbar(self: &Rc<Self>) -> gtk::Widget {
        let bar = w::hbox(6);
        bar.add_css_class("plans-toolbar");
        let st = self.st.borrow();
        let plan_open = st.open.is_some();

        let new = w::button_icon("add", "New", Btn::Filled);
        {
            let pp = self.clone();
            new.connect_clicked(move |_| {
                let pp = pp.clone();
                dialogs::name(&pp.ctx.clone(), "New plan", "", "Create", move |name| {
                    pp.create_plan(&name, &[]);
                });
            });
        }
        bar.append(&new);
        let rename = w::button_icon("edit", "Rename", Btn::Text);
        rename.set_sensitive(plan_open);
        {
            let pp = self.clone();
            rename.connect_clicked(move |_| {
                let pp = pp.clone();
                let cur = pp.st.borrow().name.clone();
                dialogs::name(&pp.ctx.clone(), "Rename plan", &cur, "Rename", move |name| {
                    pp.st.borrow_mut().name = name;
                    pp.save();
                    pp.render();
                });
            });
        }
        bar.append(&rename);
        let dup = w::button_icon("content_copy", "Duplicate", Btn::Text);
        dup.set_sensitive(plan_open);
        {
            let pp = self.clone();
            dup.connect_clicked(move |_| {
                let (name, items) = {
                    let st = pp.st.borrow();
                    (format!("Copy of {}", st.name), st.planner.queue.clone())
                };
                pp.create_plan(&name, &items);
            });
        }
        bar.append(&dup);
        let del = w::button_icon("delete", "Delete", Btn::Danger);
        del.set_sensitive(plan_open);
        {
            let pp = self.clone();
            del.connect_clicked(move |_| {
                let (pid, name, n) = {
                    let st = pp.st.borrow();
                    (st.open.clone(), st.name.clone(), st.planner.skill_count())
                };
                let Some(pid) = pid else { return };
                let body = format!(
                    "Delete the plan \"{name}\"?\n\n{}This can't be undone.",
                    if n > 0 { format!("Its {n} skill level{} will be removed. ", plural(n)) } else { String::new() }
                );
                let p2 = pp.clone();
                w::confirm(&pp.ctx, "Delete plan", &body, "Delete", true, move || {
                    // the plan may have been switched while the question was open
                    if p2.st.borrow().open.as_deref() != Some(pid.as_str()) {
                        return;
                    }
                    p2.own_saves.set(p2.own_saves.get() + 1);
                    plans::delete(p2.core(), p2.id, &pid);
                    p2.open(None);
                });
            });
        }
        bar.append(&del);
        bar.append(&separator());
        bar.append(&io::import_button(self));
        bar.append(&io::export_button(self));
        bar.append(&separator());

        let prof = gtk::ToggleButton::new();
        let row = w::hbox(8);
        row.append(&w::icon_sized("psychology", 18));
        row.append(&gtk::Label::new(Some("Implants & Accelerators")));
        prof.set_child(Some(&row));
        prof.add_css_class("outlined");
        prof.set_active(st.profiler.is_some());
        {
            let pp = self.clone();
            prof.connect_toggled(move |b| {
                let open = b.is_active();
                let state = if open { Some(profiler::ProfilerState::new(&pp)) } else { None };
                pp.st.borrow_mut().profiler = state;
                // after the toggle: the toolbar is rebuilt
                let pp = pp.clone();
                glib::idle_add_local_once(move || pp.render_right());
            });
        }
        bar.append(&prof);
        let fit = gtk::ToggleButton::new();
        let row = w::hbox(8);
        row.append(&w::icon_sized("rocket_launch", 18));
        row.append(&gtk::Label::new(Some("Fit planner")));
        fit.set_child(Some(&row));
        fit.add_css_class("outlined");
        fit.set_active(st.fit_open);
        {
            let pp = self.clone();
            fit.connect_toggled(move |b| {
                pp.st.borrow_mut().fit_open = b.is_active();
                let pp = pp.clone();
                glib::idle_add_local_once(move || pp.render_right());
            });
        }
        bar.append(&fit);
        drop(st);

        if let Some(t) = self.accelerator_toggle() {
            let spacer = w::hbox(0);
            spacer.set_hexpand(true);
            bar.append(&spacer);
            bar.append(&t);
        }
        let sw = gtk::ScrolledWindow::new();
        sw.set_vscrollbar_policy(gtk::PolicyType::Never);
        sw.set_hscrollbar_policy(gtk::PolicyType::Automatic);
        sw.set_propagate_natural_height(true);
        sw.set_child(Some(&bar));
        sw.upcast()
    }

    /// The active accelerator's switch, when there is one plans could count.
    fn accelerator_toggle(self: &Rc<Self>) -> Option<gtk::Widget> {
        let c = self.character()?;
        let window = accelerator::plan_window(&c, chrono::Utc::now())?;
        let on = !accelerator::ignored_in_plans(&c);
        let until = short_date(window.end);
        let b = gtk::Button::new();
        b.add_css_class("accel-toggle");
        if on {
            b.add_css_class("on");
        }
        let row = w::hbox(6);
        row.append(&w::icon_sized("bolt", 18));
        row.append(&gtk::Label::new(Some(&format!("+{} accelerator", window.bonus))));
        row.append(&w::icon_sized(if on { "toggle_on" } else { "toggle_off" }, 20));
        b.set_child(Some(&row));
        b.set_tooltip_text(Some(&if on {
            format!("Plan times include the active +{} cerebral accelerator until {until}, when EVE's skill queue stops counting it. Click to plan without it.", window.bonus)
        } else {
            format!("Plan times leave out the active +{} cerebral accelerator. Click to include it until {until}, as EVE's skill queue does.", window.bonus)
        }));
        let pp = self.clone();
        b.connect_clicked(move |_| {
            pp.core().update_character(pp.id, |c| {
                let v = !accelerator::ignored_in_plans(c);
                c.set_local(accelerator::IGNORE_KEY, &v);
            });
        });
        Some(b.upcast())
    }

    /// Says on the plan whether its times include the active accelerator.
    pub fn accelerator_note(&self) -> Option<String> {
        let c = self.character()?;
        let window = accelerator::plan_window(&c, chrono::Utc::now())?;
        Some(if accelerator::ignored_in_plans(&c) {
            format!("Without the +{} accelerator", window.bonus)
        } else {
            format!("With +{} accelerator until {}", window.bonus, short_date(window.end))
        })
    }

    pub fn set_columns(self: &Rc<Self>, c: Columns) {
        self.st.borrow_mut().columns = c;
        self.core().set_setting(COLUMNS_SETTING, &c);
        self.render_right();
    }

    /// Adds skill levels from an import to the open plan (or a new plan named `name` on the training queue),
    /// headed by a note saying where they came from, then offers to optimise.
    pub fn import_skills(self: &Rc<Self>, name: &str, source: &str, skills: &[(i64, u8)]) {
        if skills.is_empty() {
            self.ctx.toast("No skills found to import.");
            return;
        }
        if self.st.borrow().open.is_none() {
            let mut p = Planner::new(&self.base());
            p.add_note(name, &format!("Imported from {source}"));
            for (id, lvl) in skills {
                p.plan_skill(*id, *lvl, None);
            }
            self.create_plan(name, &p.queue);
        } else {
            self.edit(|p| {
                p.add_note(name, &format!("Imported from {source}"));
                for (id, lvl) in skills {
                    p.plan_skill(*id, *lvl, None);
                }
            });
        }
        dialogs::prompt_optimise(self, true);
    }
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn separator() -> gtk::Widget {
    let s = gtk::Box::new(gtk::Orientation::Vertical, 0);
    s.add_css_class("plans-toolbar-sep");
    s.upcast()
}

/// A plan time, e.g. "12d 4h 3m".
fn dur(ms: f64) -> String {
    fmt::duration(chrono::Duration::milliseconds(ms.round() as i64), 3)
}

/// Times in comparison columns: rounded to the minute (seconds only under a minute), so they fit.
fn dur_compare(ms: f64) -> String {
    let ms = if ms >= 60_000.0 { (ms / 60_000.0).round() * 60_000.0 } else { ms };
    dur(ms)
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

/// "Nov 21, 19:42" local time
fn short_date(t: chrono::DateTime<chrono::Utc>) -> String {
    t.with_timezone(&chrono::Local).format("%b %-d, %H:%M").to_string()
}
