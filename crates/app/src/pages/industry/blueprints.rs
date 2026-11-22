//! Industry → Blueprints, in two views:
//!
//! - My blueprints: every character's originals, copies and reaction formulas in one table (identical copies in one
//!   place merged into a row), filtered by character, kind, product category, location and research;
//! - All blueprints: every blueprint and formula in the game, with what it makes, how it's invented and who owns it.
//!
//! Activating a row opens its product in the calculator; the row button adds it to the planner.

use super::common::{duration, open_in_calculator, open_in_planner};
use super::{keep_focus, scope_failed, tab_bar, type_cell, Tab};
use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Status, Table};
use gtk::prelude::*;
use podstack_core::blueprints::{Activity, IndustryData, INDUSTRY};
use podstack_core::features::industry::{self as ind, BlueprintKind, BlueprintStack, OwnedSummary, ProductCategory, TechLevel};
use podstack_core::{config, fmt, Character};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const VIEW_KEY: &str = "industry.blueprints.view";
const MINE_SEARCH_KEY: &str = "industry.blueprints.search";
const MINE_FILTERS_KEY: &str = "industry.blueprints.filters";
const ALL_SEARCH_KEY: &str = "industry.blueprints.all.search";
const ALL_FILTERS_KEY: &str = "industry.blueprints.all.filters";

const CSS: &str = "
.bp-filters { margin: 0; }
.bp-filters > flowboxchild { padding: 0; }
";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&tab_bar(ctx, Tab::Blueprints));
    // PODSTACK_BLUEPRINTS_VIEW=all opens the game's blueprints (for UI testing)
    let all = ctx.recall(VIEW_KEY).or_else(|| std::env::var("PODSTACK_BLUEPRINTS_VIEW").ok()).as_deref() == Some("all");
    {
        let ctx = ctx.clone();
        body.append(&w::segmented(&["My blueprints", "All blueprints"], all as usize, move |i| {
            ctx.remember(VIEW_KEY, if i == 1 { "all" } else { "mine" });
            // once the toggle handler has returned: the rebuild replaces the button being clicked
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        }));
    }
    let chars = ctx.core().characters();
    if all {
        body.append(&all_view(ctx, &chars));
    } else {
        body.append(&mine_view(ctx, &chars));
    }
    Page::new("Industry", body).no_scroll()
}

// ---------- shared pieces ----------

/// Edits a view's filters, saves them and refills its table.
type Change<F> = Rc<dyn Fn(&dyn Fn(&mut F))>;

fn bp_icon(type_id: i64, copy: bool) -> String {
    format!("{}/types/{type_id}/{}?size=32", config::IMAGE_SERVER, if copy { "bpc" } else { "bp" })
}

/// What a blueprint makes: (product, units per run).
fn output_of(data: &IndustryData, blueprint: i64) -> Option<(i64, i64)> {
    data.blueprint(blueprint).and_then(|b| b.output()).map(|(_, _, p)| (p.type_id, p.quantity))
}

/// A dropdown whose button shortens long entries (station names), so filters fit a narrow window; the list shows
/// them in full and can be searched by typing.
fn filter_dropdown(items: &[String], selected: usize, tooltip: &str) -> gtk::DropDown {
    let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    let dd = w::dropdown(&refs, selected.min(items.len().saturating_sub(1)) as u32);
    dd.set_tooltip_text(Some(tooltip));
    dd.set_enable_search(items.len() > 8);
    dd.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
    dd.set_expression(Some(gtk::PropertyExpression::new(gtk::StringObject::static_type(), None::<&gtk::Expression>, "string")));
    let short = gtk::SignalListItemFactory::new();
    short.connect_setup(|_, item| {
        let l = gtk::Label::new(None);
        l.set_xalign(0.0);
        l.set_ellipsize(pango::EllipsizeMode::End);
        l.set_max_width_chars(22);
        item.downcast_ref::<gtk::ListItem>().unwrap().set_child(Some(&l));
    });
    short.connect_bind(bind_string);
    let full = gtk::SignalListItemFactory::new();
    full.connect_setup(|_, item| {
        let l = gtk::Label::new(None);
        l.set_xalign(0.0);
        item.downcast_ref::<gtk::ListItem>().unwrap().set_child(Some(&l));
    });
    full.connect_bind(bind_string);
    dd.set_factory(Some(&short));
    dd.set_list_factory(Some(&full));
    dd
}

fn bind_string(_: &gtk::SignalListItemFactory, item: &gtk::glib::Object) {
    let item = item.downcast_ref::<gtk::ListItem>().unwrap();
    let text = item.item().and_downcast::<gtk::StringObject>().map(|s| s.string().to_string()).unwrap_or_default();
    if let Some(l) = item.child().and_downcast::<gtk::Label>() {
        l.set_text(&text);
    }
}

/// A row of filter controls that wraps on narrow windows.
fn filter_row() -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.add_css_class("bp-filters");
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_homogeneous(false);
    f.set_column_spacing(8);
    f.set_row_spacing(8);
    f.set_max_children_per_line(12);
    f.set_valign(gtk::Align::Start);
    // only as wide as its filters, so a line with room to spare doesn't stretch them
    f.set_halign(gtk::Align::Start);
    f
}

/// The search field and a "12 of 340" count to its right.
fn search_bar(ctx: &Ctx, placeholder: &str, key: &str) -> (gtk::Box, gtk::SearchEntry, gtk::Label) {
    let bar = w::hbox(12);
    let entry = w::search_entry(placeholder);
    entry.set_text(&ctx.recall(key).unwrap_or_default());
    keep_focus(ctx, &entry, key);
    bar.append(&entry);
    let count = w::line("", &["body-small", "dim", "numeric"]);
    count.set_valign(gtk::Align::Center);
    bar.append(&count);
    (bar, entry, count)
}

fn shown_count(label: &gtk::Label, shown: usize, total: usize) {
    label.set_text(&if shown == total { format!("{} shown", fmt::int(total as i64)) } else { format!("{} of {}", fmt::int(shown as i64), fmt::int(total as i64)) });
}

fn planner_button(ctx: &Ctx, target: Option<(i64, i64)>) -> gtk::Widget {
    let Some(target) = target else { return w::hbox(0).upcast() };
    let b = w::icon_button("playlist_add", "Add to planner");
    b.set_valign(gtk::Align::Center);
    let ctx = ctx.clone();
    b.connect_clicked(move |_| open_in_planner(&ctx, &[target]));
    b.upcast()
}

fn me_te_label(value: i64, max: i64) -> gtk::Widget {
    let classes: &[&str] = if value >= max { &["numeric", "success-text"] } else { &["numeric"] };
    let l = w::line(&value.to_string(), classes);
    l.set_xalign(1.0);
    l.upcast()
}

fn open_product(ctx: &Ctx, product: Option<i64>) {
    match product {
        Some(p) => open_in_calculator(ctx, p),
        None => ctx.toast("PodStack doesn't know what this blueprint makes"),
    }
}

// ---------- my blueprints ----------

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct MineFilters {
    /// 0: every character.
    character: i64,
    /// A `ProductCategory` label; empty: all.
    category: String,
    /// 0: everywhere.
    location: i64,
    /// Kind chips; none on shows every kind.
    originals: bool,
    copies: bool,
    formulas: bool,
    unresearched: bool,
}

struct MineRow {
    stack: BlueprintStack,
    character: String,
    /// (product, units per run)
    output: Option<(i64, i64)>,
    product_name: String,
    category: ProductCategory,
    /// Lowercased blueprint, product, character and location names.
    haystack: String,
    sort_name: String,
    sort_product: String,
    sort_character: String,
    sort_location: String,
}

fn mine_view(ctx: &Ctx, chars: &[Arc<Character>]) -> gtk::Box {
    let view = w::vbox(16);
    view.set_vexpand(true);
    if chars.is_empty() {
        view.append(&w::empty_state("architecture", "No characters", "Add a character to see its blueprints here."));
        return view;
    }
    let no_scope = chars.iter().filter(|c| scope_failed(c, "blueprints")).count();
    let loading = chars.iter().filter(|c| c.blueprints.is_none() && !scope_failed(c, "blueprints")).count();
    if no_scope > 0 {
        view.append(&w::banner(
            "lock",
            &format!("{no_scope} character(s) need to log in again (\"Add character\") to grant Read Blueprints and show their blueprints."),
            w::BannerKind::Notice,
        ));
    }
    if loading > 0 {
        view.append(&w::banner("hourglass_empty", &format!("{loading} character(s) haven't loaded blueprints yet."), w::BannerKind::Info));
    }

    let data = &*INDUSTRY;
    let names: HashMap<i64, String> = chars.iter().map(|c| (c.id, c.name())).collect();
    let rows: Vec<Rc<MineRow>> = ind::blueprint_stacks(data, chars)
        .into_iter()
        .map(|stack| {
            let output = output_of(data, stack.type_id);
            let product_name = output.map(|(p, _)| data.type_name(p)).unwrap_or_default();
            let character = names.get(&stack.character_id).cloned().unwrap_or_default();
            let location = stack.location_name();
            let haystack = format!("{}\n{}\n{}\n{}", stack.name, product_name, character, location).to_lowercase();
            Rc::new(MineRow {
                category: ind::product_category(data, stack.type_id),
                sort_name: stack.name.to_lowercase(),
                sort_location: location.to_lowercase(),
                sort_product: product_name.to_lowercase(),
                sort_character: character.to_lowercase(),
                stack,
                character,
                output,
                product_name,
                haystack,
            })
        })
        .collect();
    if rows.is_empty() {
        if loading == 0 && no_scope < chars.len() {
            view.append(&w::empty_state("architecture", "No blueprints", "Blueprints your characters own, in any hangar or container, show up here."));
        }
        return view;
    }

    // headline counts, over every character
    let count = |k: BlueprintKind| rows.iter().filter(|r| r.stack.kind == k).map(|r| r.stack.quantity).sum::<i64>();
    let originals = count(BlueprintKind::Original);
    let researched: i64 =
        rows.iter().filter(|r| r.stack.kind == BlueprintKind::Original && ind::fully_researched(r.stack.me, r.stack.te)).map(|r| r.stack.quantity).sum();
    let products: HashSet<i64> = rows.iter().filter_map(|r| r.output.map(|o| o.0)).collect();
    view.append(&w::tile_row(vec![
        w::stat_tile("Originals", &fmt::int(originals), None),
        w::stat_tile("Copies", &fmt::int(count(BlueprintKind::Copy)), None),
        w::stat_tile("Formulas", &fmt::int(count(BlueprintKind::Formula)), None),
        w::stat_tile("Products", &fmt::int(products.len() as i64), Some("Different things they make")),
        w::stat_tile(
            "Fully researched",
            &fmt::int(researched),
            Some(&if originals > 0 { format!("{:.0}% of originals at ME 10 / TE 20", researched as f64 * 100.0 / originals as f64) } else { "No originals".into() }),
        ),
    ]));

    // filter choices from what's owned
    let mut by_char: Vec<(i64, String)> = rows.iter().map(|r| (r.stack.character_id, r.character.clone())).collect::<HashMap<_, _>>().into_iter().collect();
    by_char.sort_by_key(|c| c.1.to_lowercase());
    let present: HashSet<ProductCategory> = rows.iter().map(|r| r.category).collect();
    let categories: Vec<ProductCategory> = ProductCategory::ALL.into_iter().filter(|c| present.contains(c)).collect();
    let mut locations: Vec<(i64, String)> =
        rows.iter().map(|r| (r.stack.location_id, r.stack.location_name())).collect::<BTreeMap<_, _>>().into_iter().collect();
    locations.sort_by_key(|l| l.1.to_lowercase());

    let mut f: MineFilters = ctx.recall(MINE_FILTERS_KEY).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    // a remembered choice that no longer exists (character removed, blueprints moved) falls back to "all"
    if !by_char.iter().any(|c| c.0 == f.character) {
        f.character = 0;
    }
    if !categories.iter().any(|c| c.label() == f.category) {
        f.category.clear();
    }
    if !locations.iter().any(|l| l.0 == f.location) {
        f.location = 0;
    }
    let state = Rc::new(RefCell::new((f, ctx.recall(MINE_SEARCH_KEY).unwrap_or_default())));
    let (bar, entry, count_label) = search_bar(ctx, "Search blueprints, products, characters or locations", MINE_SEARCH_KEY);
    view.append(&bar);

    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let rows = Rc::new(rows);
    let refill: Rc<dyn Fn()> = {
        let (ctx, holder, rows, state) = (ctx.clone(), holder.clone(), rows.clone(), state.clone());
        Rc::new(move || {
            let (f, query) = state.borrow().clone();
            let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
            let any_kind = !(f.originals || f.copies || f.formulas);
            let list: Vec<Rc<MineRow>> = rows
                .iter()
                .filter(|r| {
                    let s = &r.stack;
                    (f.character == 0 || s.character_id == f.character)
                        && (f.location == 0 || s.location_id == f.location)
                        && (f.category.is_empty() || r.category.label() == f.category)
                        && (any_kind
                            || match s.kind {
                                BlueprintKind::Original => f.originals,
                                BlueprintKind::Copy => f.copies,
                                BlueprintKind::Formula => f.formulas,
                            })
                        && (!f.unresearched || (s.kind == BlueprintKind::Original && !ind::fully_researched(s.me, s.te)))
                        && words.iter().all(|w| r.haystack.contains(w.as_str()))
                })
                .cloned()
                .collect();
            shown_count(&count_label, list.len(), rows.len());
            while let Some(c) = holder.first_child() {
                holder.remove(&c);
            }
            holder.append(&mine_table(&ctx, list));
        })
    };
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        entry.connect_search_changed(move |e| {
            ctx.remember(MINE_SEARCH_KEY, e.text().as_str());
            state.borrow_mut().1 = e.text().to_string();
            refill();
        });
    }

    // Saves the filters, then refills the table.
    let change: Change<MineFilters> = {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        Rc::new(move |edit: &dyn Fn(&mut MineFilters)| {
            {
                let mut s = state.borrow_mut();
                edit(&mut s.0);
                if let Ok(json) = serde_json::to_string(&s.0) {
                    ctx.remember(MINE_FILTERS_KEY, json);
                }
            }
            refill();
        })
    };
    let f = state.borrow().0.clone();
    let filters = filter_row();
    if by_char.len() > 1 {
        let mut items = vec!["All characters".to_string()];
        items.extend(by_char.iter().map(|c| c.1.clone()));
        let dd = filter_dropdown(&items, by_char.iter().position(|c| c.0 == f.character).map_or(0, |i| i + 1), "Character");
        let (change, ids) = (change.clone(), by_char.iter().map(|c| c.0).collect::<Vec<_>>());
        dd.connect_selected_notify(move |d| {
            let id = (d.selected() as usize).checked_sub(1).and_then(|i| ids.get(i).copied()).unwrap_or(0);
            change(&|f| f.character = id);
        });
        filters.append(&dd);
    }
    {
        let mut items = vec!["All categories".to_string()];
        items.extend(categories.iter().map(|c| c.label().to_string()));
        let dd = filter_dropdown(&items, categories.iter().position(|c| c.label() == f.category).map_or(0, |i| i + 1), "What the blueprints make");
        let change = change.clone();
        dd.connect_selected_notify(move |d| {
            let label = (d.selected() as usize).checked_sub(1).and_then(|i| categories.get(i)).map(|c| c.label().to_string()).unwrap_or_default();
            change(&|f| f.category = label.clone());
        });
        filters.append(&dd);
    }
    if locations.len() > 1 {
        let mut items = vec!["All locations".to_string()];
        items.extend(locations.iter().map(|l| l.1.clone()));
        let dd = filter_dropdown(&items, locations.iter().position(|l| l.0 == f.location).map_or(0, |i| i + 1), "Location");
        let (change, ids) = (change.clone(), locations.iter().map(|l| l.0).collect::<Vec<_>>());
        dd.connect_selected_notify(move |d| {
            let id = (d.selected() as usize).checked_sub(1).and_then(|i| ids.get(i).copied()).unwrap_or(0);
            change(&|f| f.location = id);
        });
        filters.append(&dd);
    }
    let chip = |label: &str, on: bool, tooltip: &str, set: fn(&mut MineFilters, bool)| {
        let c = w::chip(label, on);
        c.set_valign(gtk::Align::Center);
        c.set_tooltip_text(Some(tooltip));
        let change = change.clone();
        c.connect_toggled(move |c| {
            let active = c.is_active();
            change(&|f| set(f, active));
        });
        c
    };
    filters.append(&chip("Originals", f.originals, "Show originals", |f, v| f.originals = v));
    filters.append(&chip("Copies", f.copies, "Show copies", |f, v| f.copies = v));
    filters.append(&chip("Formulas", f.formulas, "Show reaction formulas", |f, v| f.formulas = v));
    filters.append(&chip("Not fully researched", f.unresearched, "Originals below ME 10 / TE 20", |f, v| f.unresearched = v));
    view.append(&filters);
    view.append(&holder);
    refill();
    view
}

fn mine_table(ctx: &Ctx, rows: Vec<Rc<MineRow>>) -> gtk::Widget {
    let ctx_activate = ctx.clone();
    let ctx_plan = ctx.clone();
    Table::new(rows)
        .widget_sorted(
            "Blueprint",
            |r| type_cell(&bp_icon(r.stack.type_id, r.stack.kind == BlueprintKind::Copy), &r.stack.name, None, None),
            |a, b| a.sort_name.cmp(&b.sort_name),
        )
        .expand()
        .width(240)
        .widget_sorted(
            "Type",
            |r| {
                let tone = match r.stack.kind {
                    BlueprintKind::Original => Status::Info,
                    BlueprintKind::Copy => Status::Neutral,
                    BlueprintKind::Formula => Status::Good,
                };
                w::status(r.stack.kind.label(), tone).upcast()
            },
            |a, b| a.stack.kind.cmp(&b.stack.kind),
        )
        .width(96)
        .widget_sorted(
            "ME",
            |r| if r.stack.kind == BlueprintKind::Formula { dash() } else { me_te_label(r.stack.me, 10) },
            |a, b| a.stack.me.cmp(&b.stack.me),
        )
        .width(56)
        .widget_sorted(
            "TE",
            |r| if r.stack.kind == BlueprintKind::Formula { dash() } else { me_te_label(r.stack.te, 20) },
            |a, b| a.stack.te.cmp(&b.stack.te),
        )
        .width(56)
        .num("Runs", |r| if r.stack.runs < 0 { "∞".into() } else { fmt::int(r.stack.runs) }, |r| if r.stack.runs < 0 { f64::INFINITY } else { r.stack.runs as f64 })
        .width(72)
        .num("Qty", |r| fmt::int(r.stack.quantity), |r| r.stack.quantity as f64)
        .width(64)
        .widget_sorted(
            "Makes",
            |r| match r.output {
                Some((p, q)) => type_cell(&config::type_icon(p, 64), &r.product_name, (q > 1).then(|| format!("{} per run", fmt::int(q))).as_deref(), None),
                None => dash(),
            },
            |a, b| a.sort_product.cmp(&b.sort_product),
        )
        .width(220)
        .widget_sorted("Character", |r| super::char_cell(r.stack.character_id, &r.character), |a, b| a.sort_character.cmp(&b.sort_character))
        .width(170)
        .widget_sorted(
            "Location",
            |r| super::place_cell(r.stack.location_name(), r.stack.location.as_ref().and_then(super::place_sub)),
            |a, b| a.sort_location.cmp(&b.sort_location),
        )
        .width(220)
        .widget("", move |r| {
            // a copy's runs are what it can build; an original or formula starts at one run
            let runs = if r.stack.kind == BlueprintKind::Copy { r.stack.runs.max(1) } else { 1 };
            planner_button(&ctx_plan, r.output.map(|(p, q)| (p, q * runs)))
        })
        .width(56)
        .on_activate(move |r| open_product(&ctx_activate, r.output.map(|o| o.0)))
        .empty_text("No blueprints match the filters.")
        .build()
}

fn dash() -> gtk::Widget {
    let l = w::line("—", &["dim"]);
    l.set_xalign(1.0);
    l.upcast()
}

// ---------- all blueprints ----------

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct AllFilters {
    /// A `ProductCategory` label; empty: all.
    category: String,
    /// A `TechLevel` label; empty: all.
    tech: String,
    owned: bool,
}

struct GameRow {
    id: i64,
    name: String,
    product: i64,
    product_name: String,
    per_run: i64,
    activity: Activity,
    category: ProductCategory,
    tech: TechLevel,
    /// Blueprints it's invented from.
    sources: Vec<String>,
    max_runs: i64,
    /// Seconds per run, before skills and facilities.
    time: i64,
    materials: usize,
    owned: Option<OwnedSummary>,
    owners: String,
    haystack: String,
    sort_name: String,
    sort_product: String,
}

fn game_rows(data: &IndustryData, chars: &[Arc<Character>]) -> Vec<Rc<GameRow>> {
    let owned = ind::owned_summary(chars);
    let names: HashMap<i64, String> = chars.iter().map(|c| (c.id, c.name())).collect();
    let mut rows: Vec<Rc<GameRow>> = data
        .blueprints
        .values()
        // unpublished blueprints are test and removed items nobody can own
        .filter(|b| data.get_type(b.id).is_some_and(|t| t.published) || owned.contains_key(&b.id))
        .filter_map(|b| {
            let (activity, act, product) = b.output()?;
            let name = data.type_name(b.id);
            let product_name = data.type_name(product.type_id);
            let mut sources: Vec<String> = data.invention_sources(b.id).iter().map(|s| data.type_name(s.id)).collect();
            sources.sort();
            let owned = owned.get(&b.id).cloned();
            let owners = owned
                .as_ref()
                .map(|o| o.characters.iter().filter_map(|c| names.get(c).cloned()).collect::<Vec<_>>().join(", "))
                .unwrap_or_default();
            let haystack = format!("{name}\n{product_name}\n{owners}").to_lowercase();
            Some(Rc::new(GameRow {
                id: b.id,
                sort_name: name.to_lowercase(),
                sort_product: product_name.to_lowercase(),
                name,
                product: product.type_id,
                product_name,
                per_run: product.quantity,
                activity,
                category: ind::product_category(data, b.id),
                tech: ind::tech_level(product.type_id),
                sources,
                max_runs: b.max_runs,
                time: act.time,
                materials: act.materials.len(),
                owned,
                owners,
                haystack,
            }))
        })
        .collect();
    rows.sort_by(|a, b| a.sort_name.cmp(&b.sort_name));
    rows
}

fn all_view(ctx: &Ctx, chars: &[Arc<Character>]) -> gtk::Box {
    let view = w::vbox(16);
    view.set_vexpand(true);
    let rows = Rc::new(game_rows(&INDUSTRY, chars));

    let mut f: AllFilters = ctx.recall(ALL_FILTERS_KEY).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    if !ProductCategory::ALL.iter().any(|c| c.label() == f.category) {
        f.category.clear();
    }
    if !TechLevel::ALL.iter().any(|t| t.label() == f.tech) {
        f.tech.clear();
    }
    let state = Rc::new(RefCell::new((f.clone(), ctx.recall(ALL_SEARCH_KEY).unwrap_or_default())));
    let (bar, entry, count_label) = search_bar(ctx, "Search blueprints, products or owners", ALL_SEARCH_KEY);
    view.append(&bar);
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let refill: Rc<dyn Fn()> = {
        let (ctx, holder, rows, state) = (ctx.clone(), holder.clone(), rows.clone(), state.clone());
        Rc::new(move || {
            let (f, query) = state.borrow().clone();
            let words: Vec<String> = query.to_lowercase().split_whitespace().map(String::from).collect();
            let list: Vec<Rc<GameRow>> = rows
                .iter()
                .filter(|r| {
                    (f.category.is_empty() || r.category.label() == f.category)
                        && (f.tech.is_empty() || r.tech.label() == f.tech)
                        && (!f.owned || r.owned.is_some())
                        && words.iter().all(|w| r.haystack.contains(w.as_str()))
                })
                .cloned()
                .collect();
            shown_count(&count_label, list.len(), rows.len());
            while let Some(c) = holder.first_child() {
                holder.remove(&c);
            }
            holder.append(&game_table(&ctx, list));
        })
    };
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        entry.connect_search_changed(move |e| {
            ctx.remember(ALL_SEARCH_KEY, e.text().as_str());
            state.borrow_mut().1 = e.text().to_string();
            refill();
        });
    }
    let change: Change<AllFilters> = {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        Rc::new(move |edit: &dyn Fn(&mut AllFilters)| {
            {
                let mut s = state.borrow_mut();
                edit(&mut s.0);
                if let Ok(json) = serde_json::to_string(&s.0) {
                    ctx.remember(ALL_FILTERS_KEY, json);
                }
            }
            refill();
        })
    };

    let filters = filter_row();
    {
        let mut items = vec!["All categories".to_string()];
        items.extend(ProductCategory::ALL.iter().map(|c| c.label().to_string()));
        let dd = filter_dropdown(&items, ProductCategory::ALL.iter().position(|c| c.label() == f.category).map_or(0, |i| i + 1), "What the blueprints make");
        let change = change.clone();
        dd.connect_selected_notify(move |d| {
            let label = (d.selected() as usize).checked_sub(1).and_then(|i| ProductCategory::ALL.get(i)).map(|c| c.label().to_string()).unwrap_or_default();
            change(&|f| f.category = label.clone());
        });
        filters.append(&dd);
    }
    {
        let mut items = vec!["All tech levels".to_string()];
        items.extend(TechLevel::ALL.iter().map(|t| t.label().to_string()));
        let dd = filter_dropdown(&items, TechLevel::ALL.iter().position(|t| t.label() == f.tech).map_or(0, |i| i + 1), "Tech level of the product");
        let change = change.clone();
        dd.connect_selected_notify(move |d| {
            let label = (d.selected() as usize).checked_sub(1).and_then(|i| TechLevel::ALL.get(i)).map(|t| t.label().to_string()).unwrap_or_default();
            change(&|f| f.tech = label.clone());
        });
        filters.append(&dd);
    }
    {
        let c = w::chip("Owned", f.owned);
        c.set_valign(gtk::Align::Center);
        c.set_tooltip_text(Some("Only blueprints one of your characters owns"));
        let change = change.clone();
        c.connect_toggled(move |c| {
            let on = c.is_active();
            change(&|f| f.owned = on);
        });
        filters.append(&c);
    }
    view.append(&filters);
    view.append(&holder);
    refill();
    view
}

fn game_table(ctx: &Ctx, rows: Vec<Rc<GameRow>>) -> gtk::Widget {
    let ctx_activate = ctx.clone();
    let ctx_plan = ctx.clone();
    Table::new(rows)
        .widget_sorted(
            "Blueprint",
            |r| {
                let pill = (r.tech != TechLevel::Tech1).then(|| w::status(r.tech.short(), Status::Neutral));
                type_cell(&bp_icon(r.id, false), &r.name, None, pill)
            },
            |a, b| a.sort_name.cmp(&b.sort_name),
        )
        .expand()
        .width(260)
        .widget_sorted(
            "Makes",
            |r| type_cell(&config::type_icon(r.product, 64), &r.product_name, (r.per_run > 1).then(|| format!("{} per run", fmt::int(r.per_run))).as_deref(), None),
            |a, b| a.sort_product.cmp(&b.sort_product),
        )
        .width(220)
        .text("Activity", |r| r.activity.label().to_string())
        .width(120)
        .widget_sorted(
            "Invented from",
            |r| match r.sources.as_slice() {
                [] => w::line("—", &["dim"]).upcast(),
                [one] => w::line(one, &[]).upcast(),
                many => {
                    let l = w::line(&format!("{} ({})", many[0], many.len()), &[]);
                    l.set_tooltip_text(Some(&many.join("\n")));
                    l.upcast()
                }
            },
            |a, b| a.sources.first().cmp(&b.sources.first()),
        )
        .width(200)
        .num(
            "Max runs",
            |r| if r.activity == Activity::Reaction { "—".into() } else { fmt::int(r.max_runs) },
            |r| if r.activity == Activity::Reaction { -1.0 } else { r.max_runs as f64 },
        )
        .width(88)
        .num("Time", |r| duration(r.time as f64), |r| r.time as f64)
        .width(96)
        .num("Materials", |r| r.materials.to_string(), |r| r.materials as f64)
        .width(88)
        .widget_sorted(
            "Owned",
            |r| match &r.owned {
                None => w::line("—", &["dim"]).upcast(),
                Some(o) => {
                    let b = w::vbox(0);
                    b.set_valign(gtk::Align::Center);
                    b.append(&w::line(&r.owners, &[]));
                    let mut parts = vec![];
                    if o.originals > 0 {
                        parts.push(format!("{} BPO", fmt::int(o.originals)));
                    }
                    if o.copies > 0 {
                        parts.push(format!("{} BPC", fmt::int(o.copies)));
                    }
                    if let Some((me, te)) = o.best.filter(|_| r.activity != Activity::Reaction) {
                        parts.push(format!("best ME {me} / TE {te}"));
                    }
                    let classes: &[&str] = if o.best.is_some_and(|(me, te)| ind::fully_researched(me, te)) { &["body-small", "success-text"] } else { &["body-small", "dim"] };
                    b.append(&w::line(&parts.join(" · "), classes));
                    b.set_tooltip_text(Some(&r.owners));
                    b.upcast()
                }
            },
            |a, b| a.owned.as_ref().map(|o| o.best).cmp(&b.owned.as_ref().map(|o| o.best)),
        )
        .width(200)
        .widget("", move |r| planner_button(&ctx_plan, Some((r.product, r.per_run))))
        .width(56)
        .on_activate(move |r| open_product(&ctx_activate, Some(r.product)))
        .empty_text("No blueprints match the filters.")
        .build()
}
