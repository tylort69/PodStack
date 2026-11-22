//! A worked-out plan: headline tiles, then the jobs to run, the inventions, the shopping list and the blueprints
//! nobody owns yet. Rows that can be built or bought carry a switch that overrides the project's choices.

use super::State;
use crate::pages::industry::common::{self, Context, Prices};
use crate::pages::industry::{icon_url, type_cell};
use crate::widgets::{self as w, Status, Table};
use gtk::prelude::*;
use podstack_core::blueprints::{Activity, INDUSTRY};
use podstack_core::features::manufacturing::{self as mf, InventionStep, Plan, Project, Purchase, Step};
use podstack_core::fmt;
use podstack_core::sde::MARKET;
use std::collections::{BTreeMap, HashSet};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BpStatus {
    Missing,
    Invented,
    Owned,
}

impl BpStatus {
    fn pill(self) -> gtk::Widget {
        match self {
            BpStatus::Owned => w::status("Owned", Status::Good),
            BpStatus::Invented => w::status("Invented", Status::Info),
            BpStatus::Missing => w::status("Missing", Status::Warn),
        }
        .upcast()
    }
}

fn invented(plan: &Plan) -> HashSet<i64> {
    plan.inventions.iter().map(|i| i.invention.blueprint).collect()
}

fn status(step: &Step, invented: &HashSet<i64>) -> BpStatus {
    if step.owned {
        BpStatus::Owned
    } else if invented.contains(&step.blueprint) {
        BpStatus::Invented
    } else {
        BpStatus::Missing
    }
}

/// A blueprint the plan needs that none of the characters has.
struct Needed {
    blueprint: i64,
    product: i64,
    why: String,
}

fn needed_blueprints(plan: &Plan, cx: &Context) -> Vec<Needed> {
    let inv = invented(plan);
    let mut out: Vec<Needed> = Vec::new();
    for s in &plan.steps {
        if status(s, &inv) != BpStatus::Missing {
            continue;
        }
        let why = if s.activity == Some(Activity::Reaction) {
            format!("Formula for {} runs", fmt::int(s.runs))
        } else if !INDUSTRY.invention_sources(s.blueprint).is_empty() {
            format!("Copies for {} runs, or turn on invention", fmt::int(s.runs))
        } else {
            format!("An original, or copies for {} runs", fmt::int(s.runs))
        };
        out.push(Needed { blueprint: s.blueprint, product: s.product, why });
    }
    // invention starts from copies of the T1 blueprint
    for i in &plan.inventions {
        let src = i.invention.source;
        if cx.owned.contains_key(&src) || out.iter().any(|n| n.blueprint == src) {
            continue;
        }
        let product = INDUSTRY.blueprint(src).and_then(|b| b.output()).map(|(_, _, p)| p.type_id).unwrap_or(src);
        out.push(Needed {
            blueprint: src,
            product,
            why: format!("Copies to invent {} ({} attempts)", INDUSTRY.type_name(i.invention.blueprint), fmt::int(i.attempts)),
        });
    }
    out
}

/// Every type the results show a price for: purchases, built items, targets and blueprints to buy.
pub(super) fn price_types(plan: &Plan, project: &Project, cx: &Context) -> Vec<i64> {
    let mut t: Vec<i64> = plan.purchases.iter().map(|p| p.type_id).collect();
    t.extend(plan.steps.iter().map(|s| s.product));
    t.extend(project.targets.iter().map(|x| x.0));
    t.extend(needed_blueprints(plan, cx).iter().map(|n| n.blueprint));
    t.sort();
    t.dedup();
    t
}

fn volume(type_id: i64) -> f64 {
    MARKET.get(type_id).map(|t| t.volume).unwrap_or(0.0)
}

/// "3 × 10 runs + 1 × 4 runs"
fn jobs_label(jobs: &[i64]) -> String {
    let mut groups: BTreeMap<i64, usize> = BTreeMap::new();
    for r in jobs {
        *groups.entry(*r).or_default() += 1;
    }
    groups.iter().rev().map(|(runs, n)| format!("{n} × {} run{}", fmt::int(*runs), if *runs == 1 { "" } else { "s" })).collect::<Vec<_>>().join(" + ")
}

fn stage(s: &Step) -> &'static str {
    match (s.depth, s.activity) {
        (0, _) => "Final product",
        (_, Some(Activity::Reaction)) => "Reaction",
        _ => "Component",
    }
}

fn price_text(p: Option<f64>) -> String {
    p.map(|p| fmt::decimal(p, 2)).unwrap_or_else(|| "—".into())
}

/// A switch: on builds the item, off buys it.
fn build_switch(state: &Rc<State>, type_id: i64, building: bool) -> gtk::Widget {
    let s = gtk::Switch::new();
    s.set_active(building);
    s.set_valign(gtk::Align::Center);
    s.set_halign(gtk::Align::Start);
    s.set_tooltip_text(Some(if building { "Built here: switch off to buy it instead" } else { "Bought: switch on to build it" }));
    let state = state.clone();
    s.connect_active_notify(move |s| state.set_build(type_id, s.is_active()));
    s.upcast()
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Machine slots of each kind across the characters: (manufacturing, science, reactions).
fn slots(cx: &Context) -> (usize, usize, usize) {
    let data = &*INDUSTRY;
    if cx.characters.is_empty() {
        let (m, s, r) = mf::slots(data, &cx.levels);
        return (m as usize, s as usize, r as usize);
    }
    cx.characters.iter().fold((0, 0, 0), |acc, c| {
        let (m, s, r) = mf::slots(data, &mf::levels_of(c));
        (acc.0 + m as usize, acc.1 + s as usize, acc.2 + r as usize)
    })
}

pub(super) fn view(state: &Rc<State>, project: &Project, plan: &Plan, loading: bool) -> gtk::Box {
    let v = w::vbox(16);
    let prices: Prices = state.prices.borrow().clone();
    let hub = common::price_hub(&state.cx.doc).0;
    let costs = prices.buy_costs();
    let (materials, unpriced) = plan.purchase_cost(&costs);
    let jobs = plan.job_costs();
    let total = materials + jobs;

    // ---- banners
    let failed: Vec<i64> = plan.purchases.iter().filter(|p| p.to_buy > 0 && !prices.hub.contains_key(&p.type_id)).map(|p| p.type_id).collect();
    if !loading && !failed.is_empty() {
        let b = w::banner("cloud_off", &format!("Couldn't load prices at {hub} for {} item(s): the total leaves them out.", failed.len()), w::BannerKind::Error);
        let retry = w::button("Retry", w::Btn::Text);
        let st = state.clone();
        retry.connect_clicked(move |_| st.refresh_prices());
        b.append(&retry);
        v.append(&b);
    }
    let no_orders = unpriced.len().saturating_sub(failed.len());
    if !loading && no_orders > 0 {
        v.append(&w::banner("money_off", &format!("{no_orders} item(s) have no sell orders at {hub}: the total leaves them out."), w::BannerKind::Notice));
    }
    if !loading && prices.adjusted.is_empty() {
        v.append(&w::banner("cloud_off", "Couldn't load CCP's adjusted prices: job costs show as 0 until they load.", w::BannerKind::Notice));
    } else if !loading && prices.indices.is_none() {
        v.append(&w::banner("cloud_off", "Couldn't load the systems' cost indices: job costs leave out the system index.", w::BannerKind::Notice));
    }
    if !plan.unbuildable.is_empty() {
        let names: Vec<String> = plan.unbuildable.iter().map(|t| INDUSTRY.type_name(*t)).collect();
        v.append(&w::banner("block", &format!("Nothing in the game builds {}: left out of the plan.", names.join(", ")), w::BannerKind::Notice));
    }

    // ---- tiles
    let single = (project.targets.len() == 1).then(|| project.targets[0]);
    let cost_sub = match single {
        Some((_, q)) if q > 0 => format!("{} ISK per unit", fmt::isk_short(total / q as f64)),
        _ => format!("Materials {} + jobs {}", fmt::isk_short(materials), fmt::isk_short(jobs)),
    };
    let cost_tile = w::stat_tile("Total cost", &format!("{} ISK", fmt::isk_short(total)), Some(&cost_sub));
    cost_tile.set_tooltip_text(Some(&format!("Materials at {hub}'s lowest sell: {}\nJob and invention costs: {}", fmt::isk(materials), fmt::isk(jobs))));
    let mut value = 0.0;
    let mut unvalued = 0;
    for (t, q) in &project.targets {
        match prices.buy_cost(*t) {
            Some(p) => value += p * *q as f64,
            None => unvalued += 1,
        }
    }
    let profit = value - total;
    let value_sub = if value > 0.0 {
        format!("Profit {} ISK · margin {:.1}%", fmt::isk_short(profit), profit / value * 100.0)
    } else {
        "No sell orders to compare with".to_string()
    };
    let value_tile = w::stat_tile("Market value", &format!("{} ISK", fmt::isk_short(value)), Some(&value_sub));
    value_tile.set_tooltip_text(Some(&format!(
        "The targets at {hub}'s lowest sell{}",
        if unvalued > 0 { format!(" ({unvalued} without sell orders left out)") } else { String::new() }
    )));
    if profit > 0.0 && value > 0.0 {
        value_tile.add_css_class("accent");
    }
    let reactions: usize = plan.steps.iter().filter(|s| s.activity == Some(Activity::Reaction)).map(|s| s.jobs.len()).sum();
    let attempts: i64 = plan.inventions.iter().map(|i| i.attempts).sum();
    let manufacturing = plan.job_count() - reactions;
    let jobs_tile = w::stat_tile(
        "Jobs",
        &fmt::int(plan.job_count() as i64 + attempts),
        Some(&format!("{manufacturing} manufacturing · {reactions} reaction · {attempts} invention")),
    );
    let haul: f64 = plan.purchases.iter().map(|p| p.to_buy as f64 * volume(p.type_id)).sum();
    let out: f64 = project.targets.iter().map(|(t, q)| *q as f64 * volume(*t)).sum();
    let volume_tile = w::stat_tile("Materials to haul", &format!("{} m³", fmt::decimal(haul, 0)), Some(&format!("Products {} m³", fmt::decimal(out, 0))));
    let slots = slots(&state.cx);
    let time_tile = w::stat_tile(
        "Build time",
        &common::duration(plan.estimated_time(slots)),
        Some(&format!("On {} manufacturing, {} reaction, {} science slots", slots.0, slots.2, slots.1)),
    );
    time_tile.set_tooltip_text(Some(&format!(
        "About how long, running every level after the one below it on {}. Job times use {}.",
        if state.cx.characters.is_empty() { "one character's slots".to_string() } else { format!("all {} characters' slots", state.cx.characters.len()) },
        state.cx.levels_label
    )));
    v.append(&w::tile_row(vec![cost_tile, value_tile, jobs_tile, volume_tile, time_tile]));

    let head = w::hbox(8);
    let info = if loading {
        format!("Loading prices at {hub}…")
    } else {
        format!("Prices: lowest sell orders at {hub}. Job times use {}.", state.cx.levels_label)
    };
    let l = w::text(&info, &["body-small", "dim"]);
    l.set_hexpand(true);
    head.append(&l);
    let refresh = w::button_icon("refresh", "Refresh prices", w::Btn::Text);
    {
        let st = state.clone();
        refresh.connect_clicked(move |_| st.refresh_prices());
    }
    head.append(&refresh);
    v.append(&head);

    v.append(&steps_card(state, project, plan));
    if !plan.inventions.is_empty() {
        v.append(&inventions_card(plan, &prices));
    }
    v.append(&shopping_card(state, plan, &prices));
    let needed = needed_blueprints(plan, &state.cx);
    if !needed.is_empty() {
        v.append(&blueprints_card(needed, &prices));
    }
    v
}

fn steps_card(state: &Rc<State>, project: &Project, plan: &Plan) -> gtk::Box {
    let inv = invented(plan);
    let targets: HashSet<i64> = project.targets.iter().map(|t| t.0).collect();
    let rows: Vec<(Step, BpStatus, bool)> = plan.steps.iter().map(|s| (s.clone(), status(s, &inv), targets.contains(&s.product))).collect();
    let n = rows.len();
    let st = state.clone();
    let ctx = state.ctx.clone();
    let table = Table::new(rows)
        .widget_sorted(
            "Item",
            |r| {
                let name = INDUSTRY.type_name(r.0.product);
                let sub = format!("{} · {}", stage(&r.0), r.0.activity.map(|a| a.label()).unwrap_or(""));
                type_cell(&icon_url(r.0.product, &name, false), &name, Some(&sub), None)
            },
            |a, b| b.0.depth.cmp(&a.0.depth).then_with(|| INDUSTRY.type_name(a.0.product).cmp(&INDUSTRY.type_name(b.0.product))),
        )
        .expand()
        .width(230)
        .num("Runs", |r| fmt::int(r.0.runs), |r| r.0.runs as f64)
        .width(70)
        .text("Jobs", |r| jobs_label(&r.0.jobs))
        .width(140)
        .text("ME / TE", |r| if r.0.activity == Some(Activity::Reaction) { "—".into() } else { format!("{} / {}", r.0.me, r.0.te) })
        .width(70)
        .num("Time per job", |r| common::duration(r.0.job_times.iter().copied().fold(0.0, f64::max)), |r| r.0.job_times.iter().copied().fold(0.0, f64::max))
        .width(100)
        .num("Job cost", |r| fmt::isk_short(r.0.job_cost), |r| r.0.job_cost)
        .width(90)
        .text("Facility", |r| r.0.facility.clone())
        .width(140)
        .widget_sorted("Blueprint", |r| r.1.pill(), |a, b| a.1.cmp(&b.1))
        .width(100)
        .widget("Build", move |r| if r.2 { w::line("Target", &["dim"]).upcast() } else { build_switch(&st, r.0.product, true) })
        .width(70)
        .on_activate(move |r| common::open_in_calculator(&ctx, r.0.product))
        .natural_height()
        .empty_text("Nothing to build.")
        .build();
    w::card_with(
        "Jobs",
        Some(&format!("{n} item{} to make, deepest first: reactions, then components, then the targets. Double-click one to open it in the calculator.", if n == 1 { "" } else { "s" })),
        &[],
        &table,
    )
}

fn inventions_card(plan: &Plan, prices: &Prices) -> gtk::Box {
    let hub = prices.hub.clone();
    let rows: Vec<InventionStep> = plan.inventions.clone();
    let material_cost = move |i: &InventionStep| -> f64 {
        i.invention.materials.iter().map(|(t, q)| hub.get(t).and_then(|p| p.sell).unwrap_or(0.0) * (*q * i.attempts) as f64).sum()
    };
    let mc = material_cost.clone();
    let table = Table::new(rows)
        .widget_sorted(
            "Blueprint",
            |i| {
                let name = INDUSTRY.type_name(i.invention.blueprint);
                let sub = format!("{} runs · ME {} · TE {} per copy", i.invention.runs, i.invention.me, i.invention.te);
                type_cell(&icon_url(i.invention.blueprint, &name, true), &name, Some(&sub), None)
            },
            |a, b| INDUSTRY.type_name(a.invention.blueprint).cmp(&INDUSTRY.type_name(b.invention.blueprint)),
        )
        .expand()
        .width(240)
        .text("From", |i| INDUSTRY.type_name(i.invention.source))
        .width(170)
        .num("Chance", |i| format!("{:.1}%", i.invention.chance * 100.0), |i| i.invention.chance)
        .width(70)
        .num("Copies", |i| fmt::int(i.copies), |i| i.copies as f64)
        .width(70)
        .num("Attempts", |i| fmt::int(i.attempts), |i| i.attempts as f64)
        .width(80)
        .text("Datacores per attempt", |i| {
            i.invention
                .materials
                .iter()
                .filter(|(t, _)| Some(*t) != i.invention.decryptor)
                .map(|(t, q)| format!("{q} × {}", INDUSTRY.type_name(*t).trim_start_matches("Datacore - ")))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .width(220)
        .text("Decryptor", |i| i.invention.decryptor.map(|d| INDUSTRY.type_name(d)).unwrap_or_else(|| "None".into()))
        .width(130)
        .num("Job cost", |i| fmt::isk_short(i.invention.cost.total * i.attempts as f64), |i| i.invention.cost.total * i.attempts as f64)
        .width(90)
        .num("Materials", move |i| fmt::isk_short(material_cost(i)), move |i| mc(i))
        .width(90)
        .natural_height()
        .build();
    w::card_with("Invention", Some("Expected attempts at the success chance; their datacores and decryptors are on the shopping list."), &[], &table)
}

fn shopping_card(state: &Rc<State>, plan: &Plan, prices: &Prices) -> gtk::Box {
    let rows: Vec<Purchase> = plan.purchases.clone();
    let unit = {
        let hub = prices.hub.clone();
        move |t: i64| hub.get(&t).and_then(|p| p.sell)
    };
    let csv = {
        let mut out = vec!["Item,Needed,From stock,To buy,Unit price,Total,Volume m3".to_string()];
        for p in &rows {
            let u = unit(p.type_id);
            out.push(format!(
                "{},{},{},{},{},{},{:.2}",
                csv_field(&INDUSTRY.type_name(p.type_id)),
                p.needed,
                p.from_stock,
                p.to_buy,
                u.map(|u| format!("{u:.2}")).unwrap_or_default(),
                u.map(|u| format!("{:.2}", u * p.to_buy as f64)).unwrap_or_default(),
                volume(p.type_id) * p.to_buy as f64
            ));
        }
        out.join("\n")
    };
    let multibuy = mf::multibuy(&rows);
    let copy_multibuy = w::button_icon("shopping_cart", "Copy multibuy", w::Btn::Tonal);
    copy_multibuy.set_tooltip_text(Some("For the game's multibuy window (Regional market → Multibuy → Import)"));
    copy_multibuy.set_sensitive(!multibuy.is_empty());
    {
        let ctx = state.ctx.clone();
        copy_multibuy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &multibuy));
    }
    let copy_csv = w::button_icon("table_view", "Copy as CSV", w::Btn::Text);
    {
        let ctx = state.ctx.clone();
        copy_csv.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &csv));
    }
    let to_buy = rows.iter().filter(|p| p.to_buy > 0).count();
    let st = state.clone();
    let (u1, u2, u3, u4) = (unit.clone(), unit.clone(), unit.clone(), unit);
    let table = Table::new(rows)
        .widget_sorted(
            "Item",
            |p| {
                let name = INDUSTRY.type_name(p.type_id);
                type_cell(&icon_url(p.type_id, &name, false), &name, None, None)
            },
            |a, b| INDUSTRY.type_name(a.type_id).cmp(&INDUSTRY.type_name(b.type_id)),
        )
        .expand()
        .width(240)
        .num("Needed", |p| fmt::int(p.needed), |p| p.needed as f64)
        .width(90)
        .num("From stock", |p| fmt::int(p.from_stock), |p| p.from_stock as f64)
        .width(90)
        .num("To buy", |p| fmt::int(p.to_buy), |p| p.to_buy as f64)
        .width(90)
        .num("Unit price", move |p| price_text(u1(p.type_id)), move |p| u2(p.type_id).unwrap_or(0.0))
        .width(100)
        .num("Total", move |p| u3(p.type_id).map(|u| fmt::isk_short(u * p.to_buy as f64)).unwrap_or_else(|| "—".into()), move |p| u4(p.type_id).unwrap_or(0.0) * p.to_buy as f64)
        .width(90)
        .num("m³", |p| fmt::decimal(volume(p.type_id) * p.to_buy as f64, 1), |p| volume(p.type_id) * p.to_buy as f64)
        .width(90)
        .widget("Build", move |p| if INDUSTRY.maker(p.type_id).is_some() { build_switch(&st, p.type_id, false) } else { w::line("", &[]).upcast() })
        .width(70)
        .sort_by(5, true)
        .natural_height()
        .empty_text("Nothing to buy.")
        .build();
    w::card_with(
        "Shopping list",
        Some(&format!("{to_buy} item{} to buy. Switch one on to build it instead.", if to_buy == 1 { "" } else { "s" })),
        &[copy_multibuy.upcast(), copy_csv.upcast()],
        &table,
    )
}

fn blueprints_card(needed: Vec<Needed>, prices: &Prices) -> gtk::Box {
    let hub = prices.hub.clone();
    let hub2 = hub.clone();
    let n = needed.len();
    let table = Table::new(needed)
        .widget_sorted(
            "Blueprint",
            |b| {
                let name = INDUSTRY.type_name(b.blueprint);
                type_cell(&icon_url(b.blueprint, &name, false), &name, None, None)
            },
            |a, b| INDUSTRY.type_name(a.blueprint).cmp(&INDUSTRY.type_name(b.blueprint)),
        )
        .expand()
        .width(260)
        .text("Makes", |b| INDUSTRY.type_name(b.product))
        .width(190)
        .text("Needed", |b| b.why.clone())
        .width(280)
        .num("Original at hub", move |b| price_text(hub.get(&b.blueprint).and_then(|p| p.sell)), move |b| hub2.get(&b.blueprint).and_then(|p| p.sell).unwrap_or(0.0))
        .width(120)
        .natural_height()
        .build();
    w::card_with(
        "Blueprints needed",
        Some(&format!(
            "{n} blueprint{} the plan uses that none of the characters owns. Until you have them, the plan assumes a researched original (or a typical invented copy).",
            if n == 1 { "" } else { "s" }
        )),
        &[],
        &table,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(jobs_label(&[10, 10, 10, 4]), "3 × 10 runs + 1 × 4 runs");
        assert_eq!(jobs_label(&[1]), "1 × 1 run");
        assert_eq!(csv_field("Plate, Armor"), "\"Plate, Armor\"");
        assert_eq!(csv_field("Tritanium"), "Tritanium");
    }
}
