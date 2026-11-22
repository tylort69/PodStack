//! Pieces the Industry tabs share: the planning context (facilities, rules, skills, owned blueprints, stock),
//! loading prices and cost indices off the GTK thread, an item picker, and opening an item in another tab.

#![allow(dead_code)] // shared by the Industry tabs, which use different parts

use super::Tab;
use crate::ctx::Ctx;
use crate::rt;
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::blueprints::{Activity, INDUSTRY};
use podstack_core::features::manufacturing::{self as mf, IndustryDoc, Levels, Rules};
use podstack_core::market::{BestPrices, CostIndices};
use podstack_core::sde::TRADE_HUBS;
use podstack_core::Character;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Everything stored or known locally that a calculation needs.
pub struct Context {
    pub doc: IndustryDoc,
    pub rules: Rules,
    pub characters: Vec<Arc<Character>>,
    /// The skills calculations use: the chosen character's, else the best of all characters, else all V.
    pub levels: Levels,
    pub levels_label: String,
    pub owned: HashMap<i64, (i64, i64)>,
    pub stock: HashMap<i64, i64>,
}

pub fn context(ctx: &Ctx) -> Context {
    let core = ctx.core();
    let doc = mf::load(core);
    let characters = core.characters();
    // `skills_of` 0 means every skill at V
    let chosen = doc.skills_of.and_then(|id| characters.iter().find(|c| c.id == id));
    let (levels, levels_label) = match chosen {
        _ if doc.skills_of == Some(0) => (mf::all_five(), "all skills at V".to_string()),
        Some(c) => (mf::levels_of(c), format!("{}'s skills", c.name())),
        None if characters.iter().any(|c| c.skills.is_some()) => (mf::best_levels(&characters), "best skills of all characters".to_string()),
        None => (mf::all_five(), "all skills at V".to_string()),
    };
    Context {
        rules: mf::rules(core),
        owned: mf::owned_originals(&characters),
        stock: mf::stock(&characters),
        doc,
        characters,
        levels,
        levels_label,
    }
}

/// The hub whose prices value things: (name, station, system, region).
pub fn price_hub(doc: &IndustryDoc) -> (&'static str, i64, i64, i64) {
    doc.price_hub.as_deref().and_then(|n| TRADE_HUBS.iter().find(|h| h.0 == n)).copied().unwrap_or(TRADE_HUBS[0])
}

/// Live market data for a calculation.
#[derive(Clone, Default)]
pub struct Prices {
    /// CCP's adjusted prices (for job costs).
    pub adjusted: HashMap<i64, f64>,
    pub indices: Option<Arc<CostIndices>>,
    /// Best buy and sell at the price hub's station.
    pub hub: HashMap<i64, BestPrices>,
    pub hub_name: String,
}

impl Prices {
    /// What buying a unit costs: the lowest sell order at the hub.
    pub fn buy_cost(&self, type_id: i64) -> Option<f64> {
        self.hub.get(&type_id).and_then(|p| p.sell)
    }
    /// The best price to sell into: the highest buy order at the hub.
    pub fn instant_sell(&self, type_id: i64) -> Option<f64> {
        self.hub.get(&type_id).and_then(|p| p.buy)
    }
    /// Units of `type_id` → a buy cost map for `Plan::purchase_cost`.
    pub fn buy_costs(&self) -> HashMap<i64, f64> {
        self.hub.iter().filter_map(|(t, p)| p.sell.map(|s| (*t, s))).collect()
    }
}

/// Loads adjusted prices, cost indices and hub prices for `types` off the GTK thread.
pub fn load_prices(ctx: &Ctx, types: Vec<i64>, force: bool, done: impl FnOnce(Prices) + 'static) {
    let core = ctx.core().clone();
    let (hub_name, station, _, region) = price_hub(&mf::load(&core));
    rt::run(
        async move {
            let adjusted = core.market.adjusted_prices(&core.esi).await;
            let indices = core.market.cost_indices(&core.esi, force).await.ok();
            let mut types = types;
            types.sort();
            types.dedup();
            let hub = core.market.best_prices(&core.esi, region, Some(station), &types, force).await;
            Prices { adjusted, indices, hub, hub_name: hub_name.to_string() }
        },
        done,
    )
}

/// A search field for anything that can be built: suggestions drop down as you type; `on_pick` gets the product.
pub fn product_picker(placeholder: &str, on_pick: impl Fn(i64) + 'static) -> gtk::Box {
    let b = w::vbox(0);
    let entry = w::search_entry(placeholder);
    entry.set_hexpand(true);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("ind-suggestions");
    let pop = gtk::Popover::new();
    pop.set_autohide(true);
    pop.set_has_arrow(false);
    pop.set_position(gtk::PositionType::Bottom);
    pop.set_child(Some(&list));
    pop.set_parent(&entry);
    // GTK popovers keep the keyboard focus: give it back to the entry so typing continues.
    pop.set_can_focus(false);
    let hits: Rc<RefCell<Vec<i64>>> = Rc::default();
    let on_pick = Rc::new(on_pick);
    {
        let (list, pop, hits) = (list.clone(), pop.clone(), hits.clone());
        entry.connect_search_changed(move |e| {
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            let found = INDUSTRY.search_products(&e.text(), 10);
            *hits.borrow_mut() = found.iter().map(|h| h.0).collect();
            for (product, _, activity) in &found {
                let row = w::hbox(8);
                row.set_margin_start(8);
                row.set_margin_end(8);
                row.set_margin_top(4);
                row.set_margin_bottom(4);
                let img = w::eve_image(&format!("https://images.evetech.net/types/{product}/icon?size=32"), 24);
                row.append(&img);
                let name = gtk::Label::new(Some(&INDUSTRY.type_name(*product)));
                name.set_xalign(0.0);
                name.set_hexpand(true);
                row.append(&name);
                if *activity == Activity::Reaction {
                    let tag = gtk::Label::new(Some("Reaction"));
                    tag.add_css_class("dim");
                    row.append(&tag);
                }
                list.append(&row);
            }
            if found.is_empty() {
                pop.popdown();
            } else {
                pop.popup();
            }
        });
    }
    {
        let (hits, pop, entry2, on_pick) = (hits.clone(), pop.clone(), entry.clone(), on_pick.clone());
        list.connect_row_activated(move |_, row| {
            if let Some(t) = hits.borrow().get(row.index() as usize) {
                pop.popdown();
                entry2.set_text("");
                on_pick(*t);
            }
        });
    }
    {
        let (hits, pop, on_pick) = (hits.clone(), pop.clone(), on_pick.clone());
        entry.connect_activate(move |e| {
            if let Some(t) = hits.borrow().first().copied() {
                pop.popdown();
                e.set_text("");
                on_pick(t);
            }
        });
    }
    b.append(&entry);
    b
}

const CALC_ITEM_KEY: &str = "industry.calculator.item";
const PLANNER_ADD_KEY: &str = "industry.planner.add";

/// Opens `product` in the calculator tab.
pub fn open_in_calculator(ctx: &Ctx, product: i64) {
    ctx.remember(CALC_ITEM_KEY, product.to_string());
    super::open(ctx, Tab::Calculator);
}

/// The product the calculator shows (set by `open_in_calculator`).
pub fn calculator_item(ctx: &Ctx) -> Option<i64> {
    ctx.recall(CALC_ITEM_KEY).and_then(|s| s.parse().ok())
}

/// Opens the planner with `targets` ((type, units)) to add to the open project.
pub fn open_in_planner(ctx: &Ctx, targets: &[(i64, i64)]) {
    let text = targets.iter().map(|(t, q)| format!("{t}:{q}")).collect::<Vec<_>>().join(",");
    ctx.remember(PLANNER_ADD_KEY, text);
    super::open(ctx, Tab::Planner);
}

/// Targets waiting to be added to the planner (taken once).
pub fn take_planner_additions(ctx: &Ctx) -> Vec<(i64, i64)> {
    let text = ctx.recall(PLANNER_ADD_KEY).unwrap_or_default();
    if text.is_empty() {
        return Vec::new();
    }
    ctx.remember(PLANNER_ADD_KEY, "");
    text.split(',')
        .filter_map(|p| {
            let (t, q) = p.split_once(':')?;
            Some((t.parse().ok()?, q.parse().ok()?))
        })
        .collect()
}

/// Seconds as "3d 4h", "2h 05m" or "45s".
pub fn duration(secs: f64) -> String {
    podstack_core::fmt::duration(chrono::Duration::seconds(secs.round() as i64), 2)
}

pub const CSS: &str = "
.ind-suggestions { min-width: 360px; }
.stat-tile .stat-value.success-text { color: @success; }
.stat-tile .stat-value.error-text { color: @error; }
";
