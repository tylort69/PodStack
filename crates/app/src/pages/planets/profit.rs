//! Planetary Industry → Profit: what to produce. Every P1-P4 product's earnings for one factory running an hour at
//! hub prices, after customs taxes on importing its inputs and exporting it, with inputs bought from sell or buy
//! orders and the output sold to either; and, per character, the products their current planet kinds can make
//! without importing anything. Activating a product opens it in the Chains calculator.

use super::chains::open_in_calculator;
use super::common::{self, commodity_cell, Prices};
use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::percent_text;
use crate::pages::market_orders::character_cell;
use crate::widgets::{self as w, Table};
use gtk::prelude::*;
use podstack_core::features::planetary::{self as pl, FactoryHour};
use podstack_core::fmt;
use podstack_core::pi::{Tier, PI};
use std::cell::Cell;
use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

const TIER_KEY: &str = "planets.profit.tier";
const INPUTS_KEY: &str = "planets.profit.inputs";
const OUTPUT_KEY: &str = "planets.profit.output";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(common::CSS);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Profit));
    let (sw, holder) = w::async_holder();
    sw.set_vexpand(false);
    body.append(&sw);
    load(ctx, &holder, false);
    Page::new("Planetary Industry", body).static_page()
}

fn load(ctx: &Ctx, holder: &gtk::Box, force: bool) {
    clear(holder);
    holder.append(&w::text("Loading hub prices for every commodity…", &["body-medium", "dim"]));
    let (ctx2, holder) = (ctx.clone(), holder.clone());
    common::load_prices(ctx, Vec::new(), force, move |p| fill(&ctx2, &holder, Rc::new(p)));
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// One product's factory hour at the chosen prices.
#[derive(Clone)]
struct Row {
    type_id: i64,
    tier: Tier,
    /// What one unit of output is valued at.
    price: Option<f64>,
    f: FactoryHour,
}

impl Row {
    /// Output and every input have a price (the numbers mean something).
    fn priced(&self) -> bool {
        self.price.is_some() && self.f.unpriced == 0
    }
    fn profit(&self) -> Option<f64> {
        self.priced().then_some(self.f.profit)
    }
    fn margin(&self) -> Option<f64> {
        self.profit().filter(|_| self.f.revenue > 0.0).map(|p| p / self.f.revenue)
    }
}

/// Which prices to use: inputs bought at sell orders (else buy orders), output sold to sell orders (else buy).
#[derive(Clone, Copy)]
struct Basis {
    inputs_at_sell: bool,
    output_at_sell: bool,
}

fn rows(prices: &Prices, basis: Basis, rate: f64) -> Vec<Row> {
    let (sell, buy) = (prices.sell(), prices.buy());
    let out: &HashMap<i64, f64> = if basis.output_at_sell { &sell } else { &buy };
    let inp: &HashMap<i64, f64> = if basis.inputs_at_sell { &sell } else { &buy };
    PI.commodities
        .values()
        .filter(|c| c.tier >= 1)
        .filter_map(|c| Some(Row { type_id: c.id, tier: c.tier, price: out.get(&c.id).copied(), f: pl::factory_hour(c.id, out, inp, rate)? }))
        .collect()
}

fn money_cell(v: Option<f64>) -> gtk::Widget {
    let l = w::line(&v.map(fmt::isk_short).unwrap_or_else(|| "—".into()), &["numeric"]);
    l.set_xalign(1.0);
    if let Some(v) = v {
        l.add_css_class(if v >= 0.0 { "success-text" } else { "error-text" });
    }
    l.upcast()
}

fn by_profit(a: &Row, b: &Row) -> std::cmp::Ordering {
    a.profit().unwrap_or(f64::MIN).total_cmp(&b.profit().unwrap_or(f64::MIN))
}

fn open(ctx: &Ctx, product: i64) {
    // once the activation handler has returned: opening rebuilds the page holding the table
    let ctx = ctx.clone();
    gtk::glib::idle_add_local_once(move || open_in_calculator(&ctx, product));
}

/// The full table of products.
fn products_table(ctx: &Ctx, rows: Vec<Row>) -> gtk::Widget {
    let ctx = ctx.clone();
    Table::new(rows)
        .widget_sorted("Product", |r: &Row| commodity_cell(r.type_id), |a, b| PI.name(a.type_id).cmp(&PI.name(b.type_id)))
        .expand()
        .num("Tier", |r| format!("P{}", r.tier), |r| r.tier as f64)
        .num("Output/h", |r| fmt::decimal(r.f.output_per_hour, 0), |r| r.f.output_per_hour)
        .num("Price", |r| r.price.map(fmt::isk_short).unwrap_or_else(|| "no orders".into()), |r| r.price.unwrap_or(-1.0))
        .num("Revenue/h", |r| fmt::isk_short(r.f.revenue), |r| r.f.revenue)
        .num(
            "Inputs/h",
            |r| if r.f.unpriced > 0 { format!("{} ({} unpriced)", fmt::isk_short(r.f.input_cost), r.f.unpriced) } else { fmt::isk_short(r.f.input_cost) },
            |r| r.f.input_cost,
        )
        .num("Customs/h", |r| fmt::isk_short(r.f.taxes), |r| r.f.taxes)
        .widget_sorted("Profit/h", |r| money_cell(r.profit()), by_profit)
        .num("Margin", |r| r.margin().map(|m| format!("{:.1}%", m * 100.0)).unwrap_or_else(|| "—".into()), |r| r.margin().unwrap_or(f64::MIN))
        .sort_by(7, true)
        .max_height(560)
        .on_activate(move |r| open(&ctx, r.type_id))
        .build()
}

fn fill(ctx: &Ctx, holder: &gtk::Box, prices: Rc<Prices>) {
    clear(holder);
    let content = w::vbox(16);
    let tier: Rc<Cell<Option<Tier>>> = Rc::new(Cell::new(ctx.recall(TIER_KEY).and_then(|s| s.parse().ok()).filter(|t| (1..=4).contains(t))));
    let basis = Rc::new(Cell::new(Basis {
        inputs_at_sell: ctx.recall(INPUTS_KEY).as_deref() != Some("buy"),
        output_at_sell: ctx.recall(OUTPUT_KEY).as_deref() != Some("buy"),
    }));
    let render: Rc<dyn Fn()> = {
        let (ctx, content, prices, tier, basis) = (ctx.clone(), content.clone(), prices.clone(), tier.clone(), basis.clone());
        Rc::new(move || render(&ctx, &content, &prices, tier.get(), basis.get()))
    };

    // controls
    let controls = gtk::FlowBox::new();
    controls.set_selection_mode(gtk::SelectionMode::None);
    controls.set_column_spacing(16);
    controls.set_row_spacing(12);
    controls.set_max_children_per_line(4);
    let labelled = |label: &str, child: &gtk::Widget| {
        let b = w::vbox(4);
        b.append(&w::line(label, &["label-medium", "dim"]));
        b.append(child);
        b
    };
    let inputs = {
        let (basis, render, ctx) = (basis.clone(), render.clone(), ctx.clone());
        w::segmented(&["Sell orders", "Buy orders"], (!basis.get().inputs_at_sell) as usize, move |i| {
            basis.set(Basis { inputs_at_sell: i == 0, ..basis.get() });
            ctx.remember(INPUTS_KEY, if i == 0 { "sell" } else { "buy" });
            render();
        })
    };
    controls.insert(&labelled("Inputs bought at", inputs.upcast_ref()), -1);
    let output = {
        let (basis, render, ctx) = (basis.clone(), render.clone(), ctx.clone());
        w::segmented(&["Sell orders", "Instant sell"], (!basis.get().output_at_sell) as usize, move |i| {
            basis.set(Basis { output_at_sell: i == 0, ..basis.get() });
            ctx.remember(OUTPUT_KEY, if i == 0 { "sell" } else { "buy" });
            render();
        })
    };
    controls.insert(&labelled("Output sold to", output.upcast_ref()), -1);
    let chips = w::hbox(8);
    let mut first: Option<gtk::ToggleButton> = None;
    for (i, label) in ["All", "P1", "P2", "P3", "P4"].iter().enumerate() {
        let t: Option<Tier> = if i == 0 { None } else { Some(i as Tier) };
        let chip = w::chip(label, t == tier.get());
        match &first {
            Some(f) => chip.set_group(Some(f)),
            None => first = Some(chip.clone()),
        }
        let (tier, render, ctx) = (tier.clone(), render.clone(), ctx.clone());
        chip.connect_toggled(move |c| {
            if c.is_active() {
                tier.set(t);
                ctx.remember(TIER_KEY, t.map(|t| t.to_string()).unwrap_or_default());
                render();
            }
        });
        chips.append(&chip);
    }
    chips.set_valign(gtk::Align::End);
    controls.insert(&labelled("Tier", chips.upcast_ref()), -1);
    let refresh = w::icon_button("refresh", "Reload prices");
    refresh.set_valign(gtk::Align::End);
    refresh.set_halign(gtk::Align::Start);
    {
        let (ctx, holder) = (ctx.clone(), holder.clone());
        refresh.connect_clicked(move |_| {
            let (ctx, holder) = (ctx.clone(), holder.clone());
            gtk::glib::idle_add_local_once(move || load(&ctx, &holder, true));
        });
    }
    controls.insert(&refresh, -1);
    holder.append(&controls);
    holder.append(&content);
    render();
}

fn render(ctx: &Ctx, content: &gtk::Box, prices: &Prices, tier: Option<Tier>, basis: Basis) {
    clear(content);
    let s = common::settings();
    let all = rows(prices, basis, s.customs_tax);
    let shown: Vec<Row> = all.iter().filter(|r| tier.is_none_or(|t| r.tier == t)).cloned().collect();

    // tiles
    let best = shown.iter().filter(|r| r.priced()).max_by(|a, b| by_profit(a, b));
    let profitable = shown.iter().filter(|r| r.profit().is_some_and(|p| p > 0.0)).count();
    let priced = shown.iter().filter(|r| r.priced()).count();
    let best_tile = match best {
        Some(b) => w::stat_tile("Best ISK/hr per factory", &fmt::isk_short(b.f.profit), Some(&format!("{} (P{})", PI.name(b.type_id), b.tier))),
        None => w::stat_tile("Best ISK/hr per factory", "—", Some("No prices")),
    };
    let unpriced = shown.len() - priced;
    let profitable_sub = if unpriced > 0 { format!("of {priced} with prices ({unpriced} without)") } else { format!("of {priced}") };
    content.append(&w::tile_row(vec![
        best_tile,
        w::stat_tile("Profitable products", &profitable.to_string(), Some(&profitable_sub)),
        w::stat_tile("Customs tax", &format!("{}%", percent_text(s.customs_tax)), Some("Change it with the gear button by the tabs")),
    ]));

    let note = w::text(
        &format!(
            "One factory running for an hour at {} prices: buying its inputs, importing them and exporting the output through customs. Double-click a product to open it in the chain calculator.",
            prices.hub_name
        ),
        &["body-medium", "dim"],
    );
    content.append(&note);
    content.append(&products_table(ctx, shown));

    // what each character's planets can make on their own
    content.append(&w::line("From your planets", &["title-large"]));
    let intro = w::text(
        "Products each character's current planet kinds have every raw resource for, so their colonies could make them without importing anything. Ranked by profit per factory hour.",
        &["body-medium", "dim"],
    );
    content.append(&intro);
    let by_id: HashMap<i64, &Row> = all.iter().map(|r| (r.type_id, r)).collect();
    let mut any = false;
    for c in ctx.core().characters() {
        let Some(planets) = c.planets.as_ref() else { continue };
        let kinds: BTreeSet<String> = planets.colonies.iter().map(|col| col.planet_type.to_lowercase()).filter(|k| PI.kind(k).is_some()).collect();
        if kinds.is_empty() {
            continue;
        }
        any = true;
        let keys: Vec<&str> = kinds.iter().map(|k| k.as_str()).collect();
        let mut local: Vec<Row> = pl::makeable_on(&keys).into_iter().filter_map(|t| by_id.get(&t).map(|r| (*r).clone())).collect();
        local.sort_by(|a, b| by_profit(b, a));
        let labels: Vec<String> = keys.iter().map(|k| PI.kind(k).map(|x| x.label.to_string()).unwrap_or_else(|| k.to_string())).collect();
        let sub = format!("{} · {} products", labels.join(", "), local.len());
        let head = w::hbox(12);
        let who = character_cell(c.id, &c.name());
        who.set_hexpand(true);
        head.append(&who);
        head.append(&w::line(&sub, &["body-medium", "dim"]));
        let ctx2 = ctx.clone();
        let table = Table::new(local)
            .widget_sorted("Product", |r: &Row| commodity_cell(r.type_id), |a, b| PI.name(a.type_id).cmp(&PI.name(b.type_id)))
            .expand()
            .num("Tier", |r| format!("P{}", r.tier), |r| r.tier as f64)
            .num("Output/h", |r| fmt::decimal(r.f.output_per_hour, 0), |r| r.f.output_per_hour)
            .num("Revenue/h", |r| fmt::isk_short(r.f.revenue), |r| r.f.revenue)
            .num("Customs/h", |r| fmt::isk_short(r.f.taxes), |r| r.f.taxes)
            .widget_sorted("Profit/h", |r| money_cell(r.profit()), by_profit)
            .sort_by(5, true)
            .max_height(320)
            .on_activate(move |r| open(&ctx2, r.type_id))
            .empty_text("Nothing these planet kinds make alone.")
            .build();
        let card = w::vbox(12);
        card.append(&head);
        card.append(&table);
        content.append(&w::card(&card));
    }
    if !any {
        content.append(&w::empty_state(
            "public",
            "No colonies yet",
            "Once your characters' colonies are loaded, this shows what their planets can make without imports. It needs the esi-planets.manage_planets.v1 scope.",
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use podstack_core::market::BestPrices;

    #[test]
    fn rows_from_prices() {
        let by_name = |n: &str| PI.commodities.values().find(|c| c.name == n).unwrap().id;
        let (water, coolant, electrolytes) = (by_name("Water"), by_name("Coolant"), by_name("Electrolytes"));
        let mut hub = HashMap::new();
        hub.insert(coolant, BestPrices { sell: Some(10_000.0), buy: Some(8_000.0) });
        hub.insert(water, BestPrices { sell: Some(500.0), buy: Some(400.0) });
        hub.insert(electrolytes, BestPrices { sell: Some(500.0), buy: Some(400.0) });
        let prices = Prices { hub, hub_name: "Jita".into() };
        let rs = rows(&prices, Basis { inputs_at_sell: true, output_at_sell: true }, 0.0);
        let c = rs.iter().find(|r| r.type_id == coolant).unwrap();
        assert!(c.priced());
        assert!((c.profit().unwrap() - 10_000.0).abs() < 1e-6);
        assert!((c.margin().unwrap() - 0.2).abs() < 1e-9);
        // instant sell and buy-order inputs
        let rs = rows(&prices, Basis { inputs_at_sell: false, output_at_sell: false }, 0.0);
        let c = rs.iter().find(|r| r.type_id == coolant).unwrap();
        assert!((c.profit().unwrap() - (40_000.0 - 32_000.0)).abs() < 1e-6);
        // water's own input has no price: not counted as profitable
        assert!(rs.iter().find(|r| r.type_id == water).unwrap().profit().is_none());
        assert_eq!(rs.len(), PI.commodities.values().filter(|c| c.tier >= 1).count());
    }
}
