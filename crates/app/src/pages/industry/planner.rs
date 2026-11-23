//! Industry → Planner: saved projects that build anything (one Venture, a pasted list, a fit, a titan) down to raw
//! materials. A project holds its targets and choices; the plan (every reaction, component, invention and purchase)
//! is worked out on the GTK thread whenever either changes, then priced at the trade hub off-thread.
//!
//! - `targets`: the list of things to build, with fit and list import;
//! - `results`: tiles, the steps, invention, shopping and blueprint tables;
//! - `tree`: totals with the job cost parts, the build tree and every single job.

mod results;
mod targets;
mod tree;

use super::common::{self, Context, Prices};
use super::{scope_failed, tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use gtk::glib;
use gtk::prelude::*;
use podstack_core::blueprints::{Activity, INDUSTRY};
use podstack_core::domains::industry::BLUEPRINTS_SCOPE;
use podstack_core::features::manufacturing::{self as mf, PlanChoices, PlanInput, Project};
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

const PROJECT_KEY: &str = "industry.planner.project";
const NOTE_KEY: &str = "industry.planner.note";

const CSS: &str = "
.ind-plan-paste { font-family: monospace; padding: 12px; border-radius: 12px; background-color: @surface_container_highest; }
.ind-plan-paste text { background-color: transparent; }
.ind-plan-target { padding: 4px 8px; border-radius: 12px; }
.ind-plan-target:hover { background-color: @surface_container_high; }
.ind-plan-choices .list-item { padding-top: 4px; padding-bottom: 4px; }
";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(common::CSS);
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Planner));
    let cx = common::context(ctx);
    for b in blueprint_banners(&cx) {
        body.append(&b);
    }
    let root = w::vbox(16);
    body.append(&root);
    let projects = cx.doc.projects.clone();
    let current = ctx.recall(PROJECT_KEY).filter(|id| projects.iter().any(|p| &p.id == id)).or_else(|| projects.first().map(|p| p.id.clone()));
    let state = Rc::new(State {
        ctx: ctx.clone(),
        cx,
        projects: RefCell::new(projects),
        current: RefCell::new(current),
        root: root.downgrade(),
        results: RefCell::default(),
        adder: RefCell::default(),
        prices: RefCell::default(),
        priced: RefCell::default(),
        loaded: Cell::new(false),
        generation: Cell::new(0),
        edits: Cell::new(0),
    });
    // Things another tab sent here ("Plan this" in the calculator or the blueprints tab).
    let additions = common::take_planner_additions(ctx);
    if !additions.is_empty() {
        state.add_targets(&additions);
    }
    state.render_all();
    Page::new("Industry", body).static_page()
}

/// Notices about the characters' blueprints, which decide what counts as owned (and at which ME/TE).
fn blueprint_banners(cx: &Context) -> Vec<gtk::Box> {
    let mut out = Vec::new();
    let no_scope = cx.characters.iter().filter(|c| scope_failed(c, "blueprints")).count();
    let not_loaded = cx.characters.iter().filter(|c| c.blueprints.is_none() && !scope_failed(c, "blueprints")).count();
    if no_scope > 0 {
        out.push(w::banner(
            "lock",
            &format!(
                "{no_scope} character(s) need to log in again (\"Add character\") to grant {BLUEPRINTS_SCOPE}: until then their blueprints don't count as owned."
            ),
            w::BannerKind::Notice,
        ));
    }
    if not_loaded > 0 {
        out.push(w::banner(
            "hourglass_empty",
            &format!("{not_loaded} character(s) haven't loaded their blueprints yet: plans treat those blueprints as missing for now."),
            w::BannerKind::Info,
        ));
    }
    out
}

/// The planner's state while the tab is open. Structural changes (another project, a target added or removed)
/// rebuild everything under the tab bar; edits (quantities, choices, build/buy) only redo the results.
pub(super) struct State {
    ctx: Ctx,
    cx: Context,
    projects: RefCell<Vec<Project>>,
    current: RefCell<Option<String>>,
    // Weak: the widgets' handlers hold the state, so strong references back would keep a closed page alive.
    root: glib::WeakRef<gtk::Box>,
    results: RefCell<glib::WeakRef<gtk::Box>>,
    /// The item search and quantity, kept across redraws (see `targets::adder`).
    adder: RefCell<Option<gtk::Box>>,
    /// Prices loaded so far, and the types they cover (so an edit only fetches what's new).
    prices: RefCell<Prices>,
    priced: RefCell<HashSet<i64>>,
    loaded: Cell<bool>,
    /// Bumped by every recompute, so a slow price load doesn't overwrite newer results.
    generation: Cell<u64>,
    /// Bumped by every edit, for debouncing.
    edits: Cell<u64>,
}

impl State {
    fn project(&self) -> Option<Project> {
        let id = self.current.borrow().clone()?;
        self.projects.borrow().iter().find(|p| p.id == id).cloned()
    }

    fn choices(&self) -> PlanChoices {
        self.project().map(|p| p.options).unwrap_or_default()
    }

    fn select(&self, id: Option<String>) {
        self.ctx.remember(PROJECT_KEY, id.clone().unwrap_or_default());
        *self.current.borrow_mut() = id;
    }

    /// Saves the projects into the industry document, re-read first so facilities or rules another tab changed
    /// aren't overwritten.
    fn save(&self) {
        let core = self.ctx.core();
        let mut doc = mf::load(core);
        doc.projects = self.projects.borrow().clone();
        mf::save(core, &doc);
    }

    /// Changes the open project, starting a "New project" if there is none. Returns whether one was started.
    fn with_project(&self, f: impl FnOnce(&mut Project)) -> bool {
        let created = self.project().is_none();
        if created {
            self.create("New project".into(), Vec::new(), PlanChoices::default());
        }
        let id = self.current.borrow().clone();
        if let Some(p) = self.projects.borrow_mut().iter_mut().find(|p| Some(&p.id) == id.as_ref()) {
            f(p);
        }
        self.save();
        created
    }

    fn create(&self, name: String, targets: Vec<(i64, i64)>, options: PlanChoices) {
        let id = format!("p{}", chrono::Utc::now().timestamp_millis());
        // two projects made within the same millisecond (a quick duplicate) still get their own ids
        let id = if self.projects.borrow().iter().any(|p| p.id == id) { format!("{id}-{}", self.projects.borrow().len()) } else { id };
        self.projects.borrow_mut().push(Project { id: id.clone(), name, targets, options });
        self.select(Some(id));
        self.save();
    }

    /// Adds targets, summing quantities of ones already there.
    fn add_targets(&self, items: &[(i64, i64)]) {
        if items.is_empty() {
            return;
        }
        self.with_project(|p| {
            for (t, q) in items {
                match p.targets.iter_mut().find(|x| x.0 == *t) {
                    Some(x) => x.1 += q,
                    None => p.targets.push((*t, *q)),
                }
            }
        });
    }

    /// Shows the items an import left out because nothing builds them (none: hides the note).
    fn set_note(&self, mut skipped: Vec<String>) {
        skipped.sort();
        skipped.dedup();
        let note = if skipped.is_empty() {
            String::new()
        } else {
            format!("Not manufacturable, left out (officer, deadspace, story or unknown items): {}.", skipped.join(", "))
        };
        self.ctx.remember(NOTE_KEY, note);
    }

    /// Whether the plan builds `type_id` when nothing overrides it.
    fn default_build(choices: &PlanChoices, type_id: i64) -> bool {
        match INDUSTRY.maker(type_id) {
            Some((_, Activity::Reaction)) => choices.build_reactions,
            Some(_) => choices.build_components,
            None => false,
        }
    }

    /// Builds (true) or buys `type_id`, as an override only where that differs from the switches.
    fn set_build(self: &Rc<Self>, type_id: i64, build: bool) {
        let created = self.with_project(|p| {
            if Self::default_build(&p.options, type_id) == build {
                p.options.overrides.remove(&type_id);
            } else {
                p.options.overrides.insert(type_id, build);
            }
        });
        self.after_edit(created, false);
    }

    /// After an edit: a new project needs the whole tab redrawn (the project bar shows it), else the results.
    fn after_edit(self: &Rc<Self>, created: bool, debounce: bool) {
        let me = self.clone();
        if created {
            glib::idle_add_local_once(move || me.render_all());
        } else if debounce {
            let n = self.edits.get() + 1;
            self.edits.set(n);
            glib::timeout_add_local_once(std::time::Duration::from_millis(350), move || {
                if me.edits.get() == n {
                    me.recompute();
                }
            });
        } else {
            // once the signal that got here has returned: the results hold the widget that sent it
            glib::idle_add_local_once(move || me.recompute());
        }
    }

    /// Redraws everything below the tab bar.
    fn render_all(self: &Rc<Self>) {
        let Some(root) = self.root.upgrade() else { return };
        while let Some(c) = root.first_child() {
            root.remove(&c);
        }
        root.append(&self.project_bar());
        root.append(&targets::card(self));
        if self.project().is_some_and(|p| !p.targets.is_empty()) {
            root.append(&self.choices_card());
        }
        let results = w::vbox(16);
        root.append(&results);
        *self.results.borrow_mut() = results.downgrade();
        self.recompute();
    }

    fn project_bar(self: &Rc<Self>) -> gtk::Box {
        let bar = w::hbox(8);
        let projects = self.projects.borrow().clone();
        let current = self.current.borrow().clone();
        if projects.is_empty() {
            let l = w::text("No projects yet. Add something to build below and a project starts.", &["body-medium", "dim"]);
            l.set_hexpand(true);
            bar.append(&l);
        } else {
            bar.append(&w::line("Project", &["label-large", "dim"]));
            let names: Vec<&str> = projects.iter().map(|p| p.name.as_str()).collect();
            let selected = projects.iter().position(|p| Some(&p.id) == current.as_ref()).unwrap_or(0);
            let dd = w::dropdown(&names, selected as u32);
            dd.set_hexpand(true);
            dd.set_tooltip_text(Some("Saved projects"));
            let me = self.clone();
            dd.connect_selected_notify(move |dd| {
                let id = me.projects.borrow().get(dd.selected() as usize).map(|p| p.id.clone());
                if id != *me.current.borrow() {
                    me.select(id);
                    me.ctx.remember(NOTE_KEY, "");
                    let me = me.clone();
                    glib::idle_add_local_once(move || me.render_all());
                }
            });
            bar.append(&dd);
        }
        let new = w::button_icon("add", "New", w::Btn::Tonal);
        new.set_tooltip_text(Some("Start an empty project"));
        {
            let me = self.clone();
            new.connect_clicked(move |_| {
                let n = me.projects.borrow().len() + 1;
                me.create(format!("Project {n}"), Vec::new(), PlanChoices::default());
                me.ctx.remember(NOTE_KEY, "");
                me.render_all();
            });
        }
        bar.append(&new);
        if let Some(p) = self.project() {
            let rename = w::icon_button("edit", "Rename");
            {
                let me = self.clone();
                let name = p.name.clone();
                rename.connect_clicked(move |_| me.rename_dialog(&name));
            }
            let duplicate = w::icon_button("content_copy", "Duplicate");
            {
                let (me, p) = (self.clone(), p.clone());
                duplicate.connect_clicked(move |_| {
                    me.create(format!("{} (copy)", p.name), p.targets.clone(), p.options.clone());
                    me.render_all();
                });
            }
            let delete = w::icon_button("delete", "Delete");
            {
                let me = self.clone();
                let (id, name) = (p.id.clone(), p.name.clone());
                delete.connect_clicked(move |_| {
                    let me = me.clone();
                    let id = id.clone();
                    w::confirm(&me.ctx.clone(), "Delete project?", &format!("\"{name}\" and its targets and choices will be deleted."), "Delete", true, move || {
                        me.projects.borrow_mut().retain(|p| p.id != id);
                        let next = me.projects.borrow().first().map(|p| p.id.clone());
                        me.select(next);
                        me.save();
                        me.render_all();
                    });
                });
            }
            bar.append(&rename);
            bar.append(&duplicate);
            bar.append(&delete);
        }
        bar
    }

    fn rename_dialog(self: &Rc<Self>, name: &str) {
        let (win, content, actions) = w::dialog(&self.ctx, "Rename project", 400);
        let entry = gtk::Entry::new();
        entry.set_text(name);
        entry.set_activates_default(false);
        content.append(&entry);
        let cancel = w::button("Cancel", w::Btn::Text);
        let save = w::button("Rename", w::Btn::Filled);
        {
            let win = win.clone();
            cancel.connect_clicked(move |_| win.close());
        }
        let apply: Rc<dyn Fn()> = {
            let (me, win, entry) = (self.clone(), win.clone(), entry.clone());
            Rc::new(move || {
                let name = entry.text().trim().to_string();
                if name.is_empty() {
                    return;
                }
                me.with_project(|p| p.name = name);
                win.close();
                me.render_all();
            })
        };
        {
            let apply = apply.clone();
            save.connect_clicked(move |_| apply());
        }
        entry.connect_activate(move |_| apply());
        actions.append(&cancel);
        actions.append(&save);
        win.present();
        entry.grab_focus();
    }

    fn choices_card(self: &Rc<Self>) -> gtk::Box {
        let choices = self.choices();
        let col = w::vbox(8);
        col.add_css_class("ind-plan-choices");
        type Set = fn(&mut PlanChoices, bool);
        let switches: [(&str, &str, bool, Set); 4] = [
            ("Build components", "Make the parts (components, T1 hulls for T2) instead of buying them", choices.build_components, |c, v| c.build_components = v),
            ("Run reactions", "Make reaction products from moon materials (needs a refinery outside high-sec)", choices.build_reactions, |c, v| c.build_reactions = v),
            ("Invent T2 blueprints", "Invent copies (with datacores) when nobody owns the original", choices.invent, |c, v| c.invent = v),
            ("Use stock from assets", "Take what the characters already have off the shopping list", choices.use_stock, |c, v| c.use_stock = v),
        ];
        let mut rows: Vec<gtk::Widget> = Vec::new();
        for (title, sub, active, set) in switches {
            let me = self.clone();
            rows.push(
                w::switch_row(title, Some(sub), active, move |v| {
                    let created = me.with_project(|p| set(&mut p.options, v));
                    me.after_edit(created, false);
                })
                .upcast(),
            );
        }
        col.append(&w::flow(rows, 2));

        let fields = gtk::FlowBox::new();
        fields.set_selection_mode(gtk::SelectionMode::None);
        fields.set_column_spacing(16);
        fields.set_row_spacing(8);
        fields.set_max_children_per_line(4);
        // decryptor
        let mut names = vec!["No decryptor".to_string()];
        names.extend(INDUSTRY.decryptors.iter().map(|d| {
            let runs = if d.runs != 0 { format!(" · {:+} runs", d.runs) } else { String::new() };
            format!("{} · ×{:.1} chance · ME {:+} TE {:+}{runs}", d.name.trim_end_matches(" Decryptor"), d.probability, d.me, d.te)
        }));
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        let selected = choices.decryptor.and_then(|id| INDUSTRY.decryptors.iter().position(|d| d.id == id)).map(|i| i + 1).unwrap_or(0);
        let dd = w::dropdown(&refs, selected as u32);
        dd.set_tooltip_text(Some("The decryptor every invention uses"));
        {
            let me = self.clone();
            dd.connect_selected_notify(move |dd| {
                let i = dd.selected() as usize;
                let id = i.checked_sub(1).and_then(|i| INDUSTRY.decryptors.get(i)).map(|d| d.id);
                let created = me.with_project(|p| p.options.decryptor = id);
                me.after_edit(created, false);
            });
        }
        fields.insert(&labelled("Invention decryptor", &dd), -1);
        let days = gtk::SpinButton::with_range(0.5, 365.0, 0.5);
        days.set_digits(1);
        days.set_value(choices.max_job_days);
        days.set_tooltip_text(Some("Longer batches are split into several jobs. Originals have no run limit, but a job can't take more than 30 days. Every reaction or part of one type is pooled into as few jobs as this allows."));
        {
            let me = self.clone();
            days.connect_value_changed(move |s| {
                let v = s.value();
                let created = me.with_project(|p| p.options.max_job_days = v);
                me.after_edit(created, true);
            });
        }
        fields.insert(&labelled("Longest job (days)", &days), -1);
        if self.cx.characters.is_empty() {
            let count = gtk::SpinButton::with_range(1.0, 100.0, 1.0);
            count.set_value(choices.builder_count.max(1) as f64);
            count.set_tooltip_text(Some("How many characters (every skill at V) run jobs side by side, for the build time"));
            let me = self.clone();
            count.connect_value_changed(move |s| {
                let v = s.value() as i64;
                let created = me.with_project(|p| p.options.builder_count = v);
                me.after_edit(created, true);
            });
            fields.insert(&labelled("Characters building", &count), -1);
        }
        let buys = choices.overrides.values().filter(|b| !**b).count();
        if !choices.build_components || !choices.build_reactions || buys > 0 {
            let all = w::button_icon("account_tree", "Build everything", w::Btn::Tonal);
            all.set_tooltip_text(Some("Build every part that has a blueprint or formula, reactions included, down to raw materials"));
            all.set_valign(gtk::Align::End);
            let me = self.clone();
            all.connect_clicked(move |_| {
                me.with_project(|p| {
                    p.options.build_components = true;
                    p.options.build_reactions = true;
                    p.options.overrides.retain(|_, build| *build);
                });
                let me = me.clone();
                glib::idle_add_local_once(move || me.render_all());
            });
            fields.insert(&all, -1);
        }
        if !choices.overrides.is_empty() {
            let n = choices.overrides.len();
            let reset = w::button_icon("restart_alt", &format!("Reset {n} build/buy choice{}", if n == 1 { "" } else { "s" }), w::Btn::Text);
            reset.set_tooltip_text(Some("Forget the items switched between build and buy in the tables below"));
            reset.set_valign(gtk::Align::End);
            let me = self.clone();
            reset.connect_clicked(move |_| {
                me.with_project(|p| p.options.overrides.clear());
                let me = me.clone();
                glib::idle_add_local_once(move || me.render_all());
            });
            fields.insert(&reset, -1);
        }
        col.append(&fields);
        if !self.cx.characters.is_empty() {
            col.append(&self.builders_row(&choices));
        }

        // where jobs run, from the Facilities tab
        let doc = &self.cx.doc;
        let places = [Activity::Manufacturing, Activity::Reaction, Activity::Invention]
            .iter()
            .map(|a| format!("{}: {}", a.label(), doc.facility_for(*a).label()))
            .collect::<Vec<_>>()
            .join(" · ");
        let foot = w::hbox(8);
        let l = w::text(&format!("{places}. Job times use {}.", self.cx.levels_label), &["body-small", "dim"]);
        l.set_hexpand(true);
        foot.append(&l);
        let fac = w::button_icon("factory", "Facilities", w::Btn::Text);
        fac.set_tooltip_text(Some("Choose the stations and structures jobs run in"));
        {
            let ctx = self.ctx.clone();
            fac.connect_clicked(move |_| super::open(&ctx, Tab::Facilities));
        }
        foot.append(&fac);
        col.append(&foot);
        w::card_with("Choices", None, &[], &col)
    }

    /// Which characters run the jobs: their slots decide how long the build takes from start to finish.
    fn builders_row(self: &Rc<Self>, choices: &PlanChoices) -> gtk::Box {
        let row = w::hbox(8);
        row.append(&w::line("Building", &["label-medium", "dim"]));
        let flow = gtk::FlowBox::new();
        flow.set_selection_mode(gtk::SelectionMode::None);
        flow.set_hexpand(true);
        flow.set_max_children_per_line(8);
        for c in &self.cx.characters {
            let check = gtk::CheckButton::with_label(&c.name());
            check.set_active(choices.builders.is_empty() || choices.builders.contains(&c.id));
            check.set_tooltip_text(Some("Count this character's job slots in the build time"));
            let (me, id) = (self.clone(), c.id);
            let all: Vec<i64> = self.cx.characters.iter().map(|c| c.id).collect();
            check.connect_toggled(move |b| {
                let on = b.is_active();
                let all = all.clone();
                let created = me.with_project(|p| {
                    let mut list = if p.options.builders.is_empty() { all.clone() } else { p.options.builders.clone() };
                    list.retain(|x| *x != id);
                    if on {
                        list.push(id);
                    }
                    // everyone (or no one, which counts the same) is stored as the empty list
                    if list.is_empty() || all.iter().all(|a| list.contains(a)) {
                        list.clear();
                    }
                    p.options.builders = list;
                });
                me.after_edit(created, false);
            });
            flow.insert(&check, -1);
        }
        row.append(&flow);
        row
    }

    /// Works out the plan with the prices loaded so far.
    fn plan(&self, project: &Project) -> mf::Plan {
        let prices = self.prices.borrow();
        let input = PlanInput {
            choices: &project.options,
            doc: &self.cx.doc,
            levels: &self.cx.levels,
            owned: &self.cx.owned,
            stock: &self.cx.stock,
            adjusted: &prices.adjusted,
            indices: prices.indices.as_deref(),
            rules: &self.cx.rules,
        };
        mf::plan(&project.targets, &input)
    }

    /// Works out the plan, shows it, and loads the prices it still lacks (then shows it again with them).
    fn recompute(self: &Rc<Self>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let Some(results) = self.results.borrow().upgrade() else { return };
        let Some(project) = self.project().filter(|p| !p.targets.is_empty()) else {
            set_child(&results, &w::empty_state("precision_manufacturing", "Nothing to build yet", "Search for an item above, import a fit or paste a list. The planner works out every job, invention and purchase down to raw materials."));
            return;
        };
        let plan = self.plan(&project);
        let types = results::price_types(&plan, &project, &self.cx);
        let missing: Vec<i64> = types.iter().filter(|t| !self.priced.borrow().contains(t)).copied().collect();
        if self.loaded.get() {
            set_child(&results, &results::view(self, &project, &plan, !missing.is_empty()));
        } else {
            let spinner = w::vbox(12);
            spinner.set_margin_top(24);
            let sp = gtk::Spinner::new();
            sp.start();
            sp.set_size_request(32, 32);
            spinner.append(&sp);
            spinner.append(&w::line(&format!("Pricing {} items…", types.len()), &["dim"]));
            set_child(&results, &spinner);
        }
        if missing.is_empty() && self.loaded.get() {
            return;
        }
        // The first load asks for everything: it also fetches the adjusted prices and cost indices.
        let ask = if self.loaded.get() { missing } else { types };
        self.load_prices(ask, false, generation);
    }

    fn load_prices(self: &Rc<Self>, types: Vec<i64>, force: bool, generation: u64) {
        let me = self.clone();
        common::load_prices(&self.ctx, types, force, move |p| {
            {
                let mut prices = me.prices.borrow_mut();
                me.priced.borrow_mut().extend(p.hub.keys().copied());
                prices.hub.extend(p.hub);
                if !p.adjusted.is_empty() {
                    prices.adjusted = p.adjusted;
                }
                if p.indices.is_some() {
                    prices.indices = p.indices;
                }
                prices.hub_name = p.hub_name;
            }
            me.loaded.set(true);
            if me.generation.get() == generation {
                // the job costs change with the adjusted prices and indices: plan again
                let Some(project) = me.project() else { return };
                let plan = me.plan(&project);
                let Some(results) = me.results.borrow().upgrade() else { return };
                set_child(&results, &results::view(&me, &project, &plan, false));
            }
        });
    }

    /// Loads every price of the current plan again, bypassing the caches.
    fn refresh_prices(self: &Rc<Self>) {
        let Some(project) = self.project() else { return };
        let plan = self.plan(&project);
        let types = results::price_types(&plan, &project, &self.cx);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.priced.borrow_mut().clear();
        self.load_prices(types, true, generation);
    }
}

fn labelled(label: &str, child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let b = w::vbox(4);
    b.append(&w::line(label, &["label-medium", "dim"]));
    b.append(child);
    b
}

/// Replaces a container's children.
fn set_child(container: &gtk::Box, child: &impl IsA<gtk::Widget>) {
    while let Some(c) = container.first_child() {
        container.remove(&c);
    }
    container.append(child);
}
