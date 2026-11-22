//! Ship Browser: every ship in EVE by class, filtered by race and tech level, and for a chosen pilot whether they
//! can fly it, how long until they can, the skills that takes, and a button to add them to a skill plan.

use crate::ctx::{Ctx, Page};
use crate::pages::skill_browser::shared::{self, Catalogue, Row};
use crate::widgets::{self as w, Status};
use gtk::prelude::*;
use podstack_core::features::fitting::{can_fly, ship_training, skill_name, total_time, PlannedLevel};
use podstack_core::features::plans;
use podstack_core::sde::{ShipInfo, SHIPS};
use podstack_core::{config, fmt, training::roman, Character, Event};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const META_ORDER: [&str; 5] = ["Tech I", "Faction", "Storyline", "Tech II", "Tech III"];
const SELECTED_KEY: &str = "ship_browser.selected";
const SEARCH_KEY: &str = "ship_browser.search";
const RACE_KEY: &str = "ship_browser.race";
const META_KEY: &str = "ship_browser.meta";
const FLYABLE_KEY: &str = "ship_browser.flyable";

/// Levels to train per ship type id.
type ShipTraining = Rc<HashMap<i64, Vec<PlannedLevel>>>;

struct State {
    ctx: Ctx,
    pilot: Cell<Option<i64>>,
    selected: Cell<Option<i64>>,
    search: RefCell<String>,
    race: Cell<Option<i64>>,
    meta: RefCell<Option<String>>,
    flyable_only: Cell<bool>,
    /// Classes opened by hand (no search) and closed by hand (while searching, matches start open).
    open: RefCell<HashSet<String>>,
    closed: RefCell<HashSet<String>>,
    /// What the pilot has to train per ship, worked out once per pilot (and again when their skills change).
    training: RefCell<Option<(i64, ShipTraining)>>,
    catalogue: Catalogue,
    detail: gtk::Box,
    flyable_chip: RefCell<Option<gtk::ToggleButton>>,
}

impl State {
    fn character(&self) -> Option<Arc<Character>> {
        self.pilot.get().and_then(|id| self.ctx.core().character(id))
    }

    fn training(&self) -> ShipTraining {
        let Some(c) = self.character() else { return Rc::default() };
        if let Some((id, t)) = &*self.training.borrow() {
            if *id == c.id {
                return t.clone();
            }
        }
        let t: ShipTraining = Rc::new(SHIPS.by_id.values().map(|s| (s.type_id, ship_training(&c, s))).collect());
        *self.training.borrow_mut() = Some((c.id, t.clone()));
        t
    }

    fn searching(&self) -> bool {
        !self.search.borrow().trim().is_empty()
    }

    fn is_open(&self, group: &str) -> bool {
        if self.searching() {
            !self.closed.borrow().contains(group)
        } else {
            self.open.borrow().contains(group)
        }
    }

    fn toggle(self: &Rc<Self>, group: &str) {
        let set = if self.searching() { &self.closed } else { &self.open };
        {
            let mut set = set.borrow_mut();
            if !set.remove(group) {
                set.insert(group.to_string());
            }
        }
        render_list(self);
    }

    fn select(self: &Rc<Self>, id: i64) {
        if id < 0 || self.selected.get() == Some(id) {
            return;
        }
        self.selected.set(Some(id));
        self.ctx.remember(SELECTED_KEY, id.to_string());
        render_detail(self);
        if let Some(s) = self.detail.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>() {
            s.vadjustment().set_value(0.0);
        }
    }
}

fn race_name(id: Option<i64>) -> String {
    id.and_then(|r| SHIPS.races.get(&r).cloned()).unwrap_or_else(|| "Other".into())
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(shared::CSS);

    let state: Rc<State> = Rc::new_cyclic(|weak: &std::rc::Weak<State>| {
        let (wg, wi) = (weak.clone(), weak.clone());
        State {
            ctx: ctx.clone(),
            pilot: Cell::new(shared::selected_pilot(ctx)),
            selected: Cell::new(ctx.recall(SELECTED_KEY).and_then(|s| s.parse().ok())),
            search: RefCell::new(ctx.recall(SEARCH_KEY).unwrap_or_default()),
            race: Cell::new(ctx.recall(RACE_KEY).and_then(|s| s.parse().ok())),
            meta: RefCell::new(ctx.recall(META_KEY).filter(|m| !m.is_empty())),
            flyable_only: Cell::new(ctx.recall(FLYABLE_KEY).as_deref() == Some("1")),
            open: RefCell::default(),
            closed: RefCell::default(),
            training: RefCell::default(),
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
            flyable_chip: RefCell::default(),
        }
    });

    let pane = w::vbox(12);
    pane.add_css_class("browser-pane");
    let s = state.clone();
    pane.append(&shared::pilot_picker(ctx, move |id| {
        s.pilot.set(id);
        if let Some(chip) = &*s.flyable_chip.borrow() {
            chip.set_sensitive(id.is_some());
        }
        render_list(&s);
        render_detail(&s);
    }));

    let search = w::search_entry("Search ships or classes");
    search.set_text(&state.search.borrow());
    search.set_hexpand(false);
    let s = state.clone();
    search.connect_search_changed(move |e| {
        *s.search.borrow_mut() = e.text().to_string();
        s.closed.borrow_mut().clear();
        s.ctx.remember(SEARCH_KEY, e.text().to_string());
        render_list(&s);
    });
    pane.append(&search);
    pane.append(&filters(&state));
    pane.append(&state.catalogue.scroller);

    state.detail.set_margin_bottom(8);
    let body = shared::two_pane(ctx, &pane, &state.detail);
    render_list(&state);
    render_detail(&state);

    let s = state.clone();
    ctx.on_event(&body, move |e| match e {
        Event::CharacterUpdated(id) if Some(*id) == s.pilot.get() => {
            s.training.borrow_mut().take();
            render_list(&s);
            render_detail(&s);
        }
        Event::Doc(d) if d == plans::DOC => render_detail(&s),
        _ => {}
    });

    Page::new("Ship Browser", body).no_scroll().static_page()
}

fn filters(state: &Rc<State>) -> gtk::Widget {
    let b = w::vbox(8);
    b.add_css_class("browser-filters");
    let row = w::hbox(8);
    row.set_homogeneous(true);

    let mut races: Vec<(i64, String)> = SHIPS.races.iter().map(|(k, v)| (*k, v.clone())).collect();
    races.sort_by(|a, b| a.1.cmp(&b.1));
    let mut race_labels = vec!["All races".to_string()];
    race_labels.extend(races.iter().map(|r| r.1.clone()));
    let refs: Vec<&str> = race_labels.iter().map(String::as_str).collect();
    let selected = state.race.get().and_then(|r| races.iter().position(|x| x.0 == r)).map(|i| i as u32 + 1).unwrap_or(0);
    let race = w::dropdown(&refs, selected);
    let s = state.clone();
    race.connect_selected_notify(move |d| {
        let r = (d.selected() as usize).checked_sub(1).and_then(|i| races.get(i)).map(|r| r.0);
        s.race.set(r);
        s.ctx.remember(RACE_KEY, r.map(|r| r.to_string()).unwrap_or_default());
        render_list(&s);
    });
    row.append(&race);

    let mut meta_labels = vec!["All tech levels"];
    meta_labels.extend(META_ORDER);
    let selected = state.meta.borrow().as_deref().and_then(|m| META_ORDER.iter().position(|x| *x == m)).map(|i| i as u32 + 1).unwrap_or(0);
    let meta = w::dropdown(&meta_labels, selected);
    let s = state.clone();
    meta.connect_selected_notify(move |d| {
        let m = (d.selected() as usize).checked_sub(1).map(|i| META_ORDER[i].to_string());
        s.ctx.remember(META_KEY, m.clone().unwrap_or_default());
        *s.meta.borrow_mut() = m;
        render_list(&s);
    });
    row.append(&meta);
    b.append(&row);

    let chip = w::chip("Can fly", state.flyable_only.get());
    chip.set_halign(gtk::Align::Start);
    chip.set_tooltip_text(Some("Only ships the pilot can fly now"));
    chip.set_sensitive(state.pilot.get().is_some());
    let s = state.clone();
    chip.connect_toggled(move |c| {
        s.flyable_only.set(c.is_active());
        s.ctx.remember(FLYABLE_KEY, if c.is_active() { "1" } else { "0" });
        render_list(&s);
    });
    b.append(&chip);
    *state.flyable_chip.borrow_mut() = Some(chip);
    b.upcast()
}

fn render_list(state: &Rc<State>) {
    let c = state.character();
    let training = state.training();
    let query = state.search.borrow().trim().to_lowercase();
    let meta = state.meta.borrow().clone();
    let ships = SHIPS.by_id.values().filter(|s| {
        state.race.get().is_none_or(|r| s.race_id == Some(r))
            && meta.as_deref().is_none_or(|m| s.meta == m)
            && (!state.flyable_only.get() || c.as_deref().is_none_or(|c| can_fly(c, s)))
            && (query.is_empty() || s.name.to_lowercase().contains(&query) || s.group.to_lowercase().contains(&query))
    });
    let mut groups: BTreeMap<&str, Vec<&ShipInfo>> = BTreeMap::new();
    for s in ships {
        groups.entry(s.group.as_str()).or_default().push(s);
    }

    let mut rows = Vec::new();
    for (group, mut list) in groups {
        list.sort_by(|a, b| a.name.cmp(&b.name));
        let count = match &c {
            Some(c) => format!("{}/{}", list.iter().filter(|s| can_fly(c, s)).count(), list.len()),
            None => list.len().to_string(),
        };
        let open = state.is_open(group);
        rows.push(Row::Group { key: group.to_string(), title: group.to_string(), count, open });
        if !open {
            continue;
        }
        for s in list {
            let trailing: Option<gtk::Widget> = c.as_deref().map(|c| {
                if can_fly(c, s) {
                    let i = w::icon_sized("check_circle", 20);
                    i.add_css_class("success-text");
                    i.set_tooltip_text(Some("Can fly"));
                    i.upcast()
                } else {
                    let t = training.get(&s.type_id).map(|l| fmt::duration(total_time(l), 2)).unwrap_or_default();
                    w::line(&t, &["body-small", "dim", "numeric"]).upcast()
                }
            });
            let icon = w::eve_image(&config::type_icon(s.type_id, 32), 28);
            rows.push(Row::Item { id: s.type_id, content: shared::item_content(Some(icon.upcast()), &s.name, trailing) });
        }
    }
    if rows.is_empty() {
        let l = w::text("No ships match.", &["body-medium", "dim"]);
        l.set_margin_start(16);
        l.set_margin_top(8);
        rows.push(Row::Item { id: -1, content: l.upcast() });
    }
    state.catalogue.set_rows(rows, state.selected.get(), false);
}

fn render_detail(state: &Rc<State>) {
    let d = &state.detail;
    shared::clear(d);
    let Some(ship) = state.selected.get().and_then(|id| SHIPS.by_id.get(&id)) else {
        d.append(&w::empty_state(
            "rocket_launch",
            "No ship selected",
            "Pick a ship to see the skills it needs, whether the pilot can fly it, and how long until they can.",
        ));
        return;
    };
    let c = state.character();
    let training = state.training();
    let needed = training.get(&ship.type_id).cloned().unwrap_or_default();
    d.append(&hero(ship, c.as_deref(), &needed));
    d.append(&skills_card(ship, c.as_deref()));
    if let Some(c) = &c {
        if !can_fly(c, ship) && !needed.is_empty() {
            d.append(&to_train_card(state, ship, c, &needed));
        }
    }
    if !ship.skins.is_empty() {
        let chips: Vec<gtk::Widget> = ship.skins.iter().map(|s| shared::info_chip(&s.name).upcast()).collect();
        let body = shared::chip_row(chips);
        body.set_margin_top(12);
        let exp = gtk::Expander::new(None);
        exp.set_label_widget(Some(&w::text(&format!("{} SKIN{}", ship.skins.len(), if ship.skins.len() == 1 { "" } else { "s" }), &["title-medium"])));
        exp.set_child(Some(&body));
        d.append(&w::card(&exp));
    }
}

fn hero(ship: &ShipInfo, c: Option<&Character>, needed: &[PlannedLevel]) -> gtk::Widget {
    let hero = w::hbox(24);
    hero.add_css_class("browser-hero");
    let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
    frame.add_css_class("browser-render");
    frame.set_overflow(gtk::Overflow::Hidden);
    frame.set_valign(gtk::Align::Center);
    frame.append(&w::eve_image(&config::type_render(ship.type_id, 256), 160));
    hero.append(&frame);

    let texts = w::vbox(8);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    let name = w::text(&ship.name, &["headline-medium"]);
    name.set_selectable(true);
    texts.append(&name);
    texts.append(&w::text(&ship.group, &["title-medium", "dim"]));
    let mut chips: Vec<gtk::Widget> = vec![shared::info_chip(&race_name(ship.race_id)).upcast()];
    if !ship.meta.is_empty() {
        chips.push(shared::info_chip(&ship.meta).upcast());
    }
    if let Some(c) = c {
        if can_fly(c, ship) {
            chips.push(w::status(&format!("{} can fly it", c.name()), Status::Good).upcast());
        } else if !needed.is_empty() {
            chips.push(w::status(&format!("{} to fly", fmt::duration(total_time(needed), 3)), Status::Warn).upcast());
        } else {
            chips.push(w::status("Can't fly yet", Status::Warn).upcast());
        }
    }
    texts.append(&shared::pill_row(chips));
    hero.append(&texts);
    hero.upcast()
}

fn skills_card(ship: &ShipInfo, c: Option<&Character>) -> gtk::Widget {
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("m3-list");
    if ship.skills.is_empty() {
        return w::card_with("Required skills", None, &[], &w::text("None: any pilot can fly it.", &["body-medium", "dim"])).upcast();
    }
    for r in &ship.skills {
        let has = c.map(|c| c.trained_skill_level(r.id));
        let (icon, class) = match has {
            Some(h) if h >= r.level => ("check_circle", "success-text"),
            Some(_) => ("cancel", "warning-text"),
            None => ("school", "dim"),
        };
        let i = w::icon(icon);
        i.add_css_class(class);
        let trailing: Option<gtk::Widget> = has.map(|h| {
            let l = w::line(
                &if h > 0 { format!("Has {}", roman(h)) } else { "Not trained".into() },
                &["body-medium", if h >= r.level { "success-text" } else { "warning-text" }],
            );
            l.upcast()
        });
        let row = w::list_row(Some(i.upcast_ref()), &skill_name(r.id), Some(&format!("Needs level {}", roman(r.level))), trailing.as_ref());
        list.append(&row);
    }
    let card = w::card_with("Required skills", Some("Their prerequisites come with them"), &[], &list);
    card.upcast()
}

fn to_train_card(state: &Rc<State>, ship: &ShipInfo, c: &Character, needed: &[PlannedLevel]) -> gtk::Widget {
    let body = w::vbox(8);
    body.append(&shared::levels_list(needed));
    let name = format!("Fly {}", ship.name);
    body.append(&shared::add_to_plan_row(&state.ctx, c.id, &name, Some(name.clone()), needed));
    let subtitle = format!(
        "{} level{} \u{00b7} {} for {}",
        needed.len(),
        if needed.len() == 1 { "" } else { "s" },
        fmt::duration(total_time(needed), 3),
        c.name()
    );
    w::card_with("To train", Some(&subtitle), &[], &body).upcast()
}
