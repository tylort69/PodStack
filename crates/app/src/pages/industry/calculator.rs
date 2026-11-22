//! Industry → Calculator: one item worked out. Pick anything buildable, set runs, ME/TE and the facility, and see
//! the materials at hub prices, the job cost with the system's live cost index, build time with your skills, and
//! profit against the hub's sell and buy orders after market fees. T2 items add invention: chance with your skills,
//! cost per invented run, and every decryptor compared. Materials that can be built themselves open in the
//! calculator in turn, and the whole thing goes to the planner in one click.

use super::common::{self, Context, Prices, CSS};
use super::form::{fields, labelled, number_entry, percent_text, system_name};
use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::type_cell;
use crate::widgets::{self as w, Btn, Table};
use gtk::prelude::*;
use podstack_core::blueprints::{Activity, Blueprint, INDUSTRY};
use podstack_core::features::manufacturing::{self as mf, Facility, Invention, InventionEnv, JobInput, JobResult};
use podstack_core::features::trade::Fees;
use podstack_core::fmt;
use podstack_core::sde::MARKET;
use std::cell::RefCell;
use std::rc::Rc;

const RUNS_KEY: &str = "industry.calculator.runs";
const ME_KEY: &str = "industry.calculator.me";
const TE_KEY: &str = "industry.calculator.te";
const FACILITY_KEY: &str = "industry.calculator.facility";
const DECRYPTOR_KEY: &str = "industry.calculator.decryptor";

/// Broker Relations and Accounting, for market fees on the product.
const BROKER_RELATIONS: i64 = 3446;
const ACCOUNTING: i64 = 16622;

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Calculator));
    {
        let ctx = ctx.clone();
        body.append(&common::product_picker("Search anything you can build or react…", move |t| {
            for k in [ME_KEY, TE_KEY, DECRYPTOR_KEY] {
                ctx.remember(k, "");
            }
            common::open_in_calculator(&ctx, t);
        }));
    }
    // The item: chosen here or sent from another tab; `PODSTACK_CALC_ITEM` for UI testing.
    let item = common::calculator_item(ctx).or_else(|| std::env::var("PODSTACK_CALC_ITEM").ok().and_then(|v| v.parse().ok()));
    match item.and_then(|t| INDUSTRY.maker(t).map(|(bp, a)| (t, bp.clone(), a))) {
        Some((product, bp, activity)) => body.append(&item_view(ctx, product, bp, activity)),
        None => body.append(&w::empty_state(
            "calculate",
            "What do you want to build?",
            "Search for an item above (a Venture, a module, a T2 ship, a reaction, a capital component, a titan) to see what it takes, what it costs where you build, and what it's worth.",
        )),
    }
    Page::new("Industry", body).static_page()
}

/// The inputs a calculation is made with.
#[derive(Clone)]
struct Inputs {
    runs: i64,
    me: i64,
    te: i64,
    facility: Facility,
    decryptor: Option<i64>,
}

fn item_view(ctx: &Ctx, product: i64, bp: Blueprint, activity: Activity) -> gtk::Box {
    let v = w::vbox(16);
    let cx = Rc::new(common::context(ctx));
    let reaction = activity == Activity::Reaction;
    let sources = INDUSTRY.invention_sources(bp.id).into_iter().cloned().collect::<Vec<_>>();
    let owned = cx.owned.get(&bp.id).copied();
    let invented = owned.is_none() && !sources.is_empty();

    // header
    let header = w::hbox(16);
    header.append(&w::eve_image(&format!("https://images.evetech.net/types/{product}/icon?size=64"), 56));
    let titles = w::vbox(2);
    titles.append(&w::line(&INDUSTRY.type_name(product), &["title-large"]));
    let made_with = match (reaction, owned, invented) {
        (true, _, _) => format!("Reaction · {}", INDUSTRY.type_name(bp.id)),
        (_, Some((me, te)), _) => format!("{} · you own an original (ME {me}, TE {te})", INDUSTRY.type_name(bp.id)),
        (_, None, true) => format!("{} · invented from {}", INDUSTRY.type_name(bp.id), sources.iter().map(|s| INDUSTRY.type_name(s.id)).collect::<Vec<_>>().join(", ")),
        _ => format!("{} · not owned by your characters", INDUSTRY.type_name(bp.id)),
    };
    titles.append(&w::line(&made_with, &["body-medium", "dim"]));
    let per_run = bp.activity(activity).and_then(|a| a.products.first()).map(|p| p.quantity).unwrap_or(1);
    if per_run > 1 {
        titles.append(&w::line(&format!("{per_run} units per run"), &["body-small", "dim"]));
    }
    titles.set_hexpand(true);
    header.append(&titles);
    let to_planner = w::button_icon("playlist_add", "Add to planner", Btn::Tonal);
    header.append(&to_planner);
    v.append(&header);

    // inputs
    let default_me_te = match (owned, invented) {
        (Some(o), _) => o,
        (None, true) => (cx.rules.invented_me, cx.rules.invented_te),
        (None, false) => (cx.rules.assumed_me, cx.rules.assumed_te),
    };
    let recall_num = |k: &str| ctx.recall(k).and_then(|s| s.parse::<i64>().ok());
    let runs = number_entry(&recall_num(RUNS_KEY).unwrap_or(1).to_string(), 8);
    let me = number_entry(&recall_num(ME_KEY).unwrap_or(default_me_te.0).to_string(), 4);
    let te = number_entry(&recall_num(TE_KEY).unwrap_or(default_me_te.1).to_string(), 4);
    let facilities: Vec<Facility> = std::iter::once(cx.doc.facility_for(activity))
        .chain(cx.doc.facilities.iter().filter(|f| f.supports(&INDUSTRY, activity)).cloned())
        .fold(Vec::new(), |mut acc, f| {
            if !acc.iter().any(|x: &Facility| x.id == f.id) {
                acc.push(f);
            }
            acc
        });
    let fac_labels: Vec<String> = facilities.iter().map(|f| f.label()).collect();
    let fac_labels: Vec<&str> = fac_labels.iter().map(|s| s.as_str()).collect();
    let fac_now = ctx.recall(FACILITY_KEY).and_then(|id| facilities.iter().position(|f| f.id == id)).unwrap_or(0) as u32;
    let facility = w::dropdown(&fac_labels, fac_now);
    let decryptors: Vec<Option<i64>> = std::iter::once(None).chain(INDUSTRY.decryptors.iter().map(|d| Some(d.id))).collect();
    let dec_labels: Vec<String> = decryptors.iter().map(|d| d.and_then(|d| INDUSTRY.decryptor(d)).map(|d| d.name.clone()).unwrap_or_else(|| "No decryptor".into())).collect();
    let dec_labels: Vec<&str> = dec_labels.iter().map(|s| s.as_str()).collect();
    let dec_now = ctx.recall(DECRYPTOR_KEY).and_then(|s| s.parse::<i64>().ok()).and_then(|d| decryptors.iter().position(|x| *x == Some(d))).unwrap_or(0) as u32;
    let decryptor = w::dropdown(&dec_labels, dec_now);

    let mut field_list: Vec<gtk::Widget> = vec![labelled("Runs", &runs).upcast()];
    if !reaction {
        field_list.push(labelled("ME", &me).upcast());
        field_list.push(labelled("TE", &te).upcast());
    }
    field_list.push(labelled("Facility", &facility).upcast());
    if invented {
        field_list.push(labelled("Decryptor", &decryptor).upcast());
    }
    v.append(&fields(field_list));
    let skills_note = w::line(&format!("Times and invention use {}. Change it, the price hub and facilities in the Facilities tab.", cx.levels_label), &["body-small", "dim"]);
    skills_note.set_xalign(0.0);
    skills_note.set_wrap(true);
    v.append(&skills_note);

    let (holder_sw, holder) = w::async_holder();
    v.append(&holder_sw);
    holder.append(&w::line("Loading prices and cost indices…", &["dim"]));

    let prices: Rc<RefCell<Option<Prices>>> = Rc::default();
    let read_inputs = {
        let (runs, me, te, facility, decryptor) = (runs.clone(), me.clone(), te.clone(), facility.clone(), decryptor.clone());
        let (facilities, decryptors) = (facilities.clone(), decryptors.clone());
        move || Inputs {
            runs: runs.text().trim().parse::<i64>().unwrap_or(1).clamp(1, 1_000_000),
            me: me.text().trim().parse::<i64>().unwrap_or(0).clamp(0, 10),
            te: te.text().trim().parse::<i64>().unwrap_or(0).clamp(0, 20),
            facility: facilities.get(facility.selected() as usize).cloned().unwrap_or_else(|| Facility::npc(30000142)),
            decryptor: decryptors.get(decryptor.selected() as usize).copied().flatten(),
        }
    };
    let read_inputs = Rc::new(read_inputs);
    let render: Rc<dyn Fn()> = {
        let (ctx, holder, prices, cx, bp, sources, read_inputs) = (ctx.clone(), holder.clone(), prices.clone(), cx.clone(), bp.clone(), sources.clone(), read_inputs.clone());
        Rc::new(move || {
            let Some(p) = prices.borrow().clone() else { return };
            let inputs = read_inputs();
            ctx.remember(RUNS_KEY, inputs.runs.to_string());
            if !reaction {
                ctx.remember(ME_KEY, inputs.me.to_string());
                ctx.remember(TE_KEY, inputs.te.to_string());
            }
            ctx.remember(FACILITY_KEY, inputs.facility.id.clone());
            ctx.remember(DECRYPTOR_KEY, inputs.decryptor.map(|d| d.to_string()).unwrap_or_default());
            set_children(&holder, results(&ctx, &cx, &p, &bp, product, invented.then(|| sources.first()).flatten(), &inputs));
        })
    };
    for e in [&runs, &me, &te] {
        let render = render.clone();
        e.connect_changed(move |_| render());
    }
    for d in [&facility, &decryptor] {
        let render = render.clone();
        d.connect_selected_notify(move |_| render());
    }
    {
        let (ctx, read_inputs) = (ctx.clone(), read_inputs.clone());
        to_planner.connect_clicked(move |_| {
            let i = read_inputs();
            common::open_in_planner(&ctx, &[(product, i.runs * per_run)]);
        });
    }

    // Every type the page may price: materials, the product, and invention inputs with every decryptor.
    let mut types: Vec<i64> = bp.activity(activity).map(|a| a.materials.iter().map(|m| m.0).collect()).unwrap_or_default();
    types.push(product);
    for s in &sources {
        if let Some(inv) = s.activity(Activity::Invention) {
            types.extend(inv.materials.iter().map(|m| m.0));
        }
    }
    types.extend(INDUSTRY.decryptors.iter().map(|d| d.id));
    {
        let (prices, render) = (prices.clone(), render.clone());
        common::load_prices(ctx, types, false, move |p| {
            *prices.borrow_mut() = Some(p);
            render();
        });
    }
    v
}

fn set_children(holder: &gtk::Box, children: Vec<gtk::Widget>) {
    while let Some(c) = holder.first_child() {
        holder.remove(&c);
    }
    for c in children {
        holder.append(&c);
    }
}

fn job(cx: &Context, p: &Prices, bp: &Blueprint, inputs: &Inputs, me: i64, te: i64) -> Option<JobResult> {
    mf::manufacture(&JobInput {
        blueprint: bp,
        runs: inputs.runs,
        me,
        te,
        facility: &inputs.facility,
        levels: &cx.levels,
        adjusted: &p.adjusted,
        indices: p.indices.as_deref(),
        rules: &cx.rules,
    })
}

/// Cost of a list of (type, units) at hub sell prices, and the types without a price.
fn cost_of(p: &Prices, items: &[(i64, i64)]) -> (f64, usize) {
    let mut total = 0.0;
    let mut missing = 0;
    for (t, q) in items {
        match p.buy_cost(*t) {
            Some(c) => total += c * *q as f64,
            None => missing += 1,
        }
    }
    (total, missing)
}

/// Invention per invented run: (invention cost per run, the invention).
fn invention_per_run(cx: &Context, p: &Prices, source: &Blueprint, bp: &Blueprint, decryptor: Option<i64>) -> Option<(f64, Invention)> {
    let inv_fac = cx.doc.facility_for(Activity::Invention);
    let env = InventionEnv { facility: &inv_fac, levels: &cx.levels, adjusted: &p.adjusted, indices: p.indices.as_deref(), rules: &cx.rules };
    let inv = mf::invention(source, bp, decryptor, &env)?;
    if inv.chance <= 0.0 {
        return None;
    }
    let (materials, _) = cost_of(p, &inv.materials);
    let per_attempt = materials + inv.cost.total;
    Some((per_attempt / inv.chance / inv.runs as f64, inv))
}

fn results(ctx: &Ctx, cx: &Context, p: &Prices, bp: &Blueprint, product: i64, source: Option<&Blueprint>, inputs: &Inputs) -> Vec<gtk::Widget> {
    let mut out: Vec<gtk::Widget> = Vec::new();
    // With invention, the decryptor sets ME/TE of the invented copy.
    let invention = source.and_then(|s| invention_per_run(cx, p, s, bp, inputs.decryptor));
    let (me, te) = match &invention {
        Some((_, inv)) if inputs.decryptor.is_some() => (inv.me, inv.te),
        _ => (inputs.me, inputs.te),
    };
    let Some(j) = job(cx, p, bp, inputs, me, te) else {
        out.push(w::empty_state("error", "Can't work this out", "The blueprint has no manufacturing or reaction activity.").upcast());
        return out;
    };
    let (materials, unpriced) = cost_of(p, &j.materials);
    let invention_cost = invention.as_ref().map(|(per_run, _)| per_run * inputs.runs as f64).unwrap_or(0.0);
    let total = materials + j.cost.total + invention_cost;
    let per_unit = total / j.produced.max(1) as f64;
    let fees = Fees::from_skills(
        *cx.levels.get(&BROKER_RELATIONS).unwrap_or(&0) as u8,
        *cx.levels.get(&ACCOUNTING).unwrap_or(&0) as u8,
    );
    let sell = p.buy_cost(product);
    let buy = p.instant_sell(product);
    let sell_net = sell.map(|s| s * (1.0 - fees.broker - fees.tax));
    let buy_net = buy.map(|b| b * (1.0 - fees.tax));

    if unpriced > 0 {
        out.push(w::banner("info", &format!("{unpriced} material(s) have no sell orders at {}: the cost leaves them out.", p.hub_name), w::BannerKind::Notice).upcast());
    }
    if p.indices.is_none() {
        out.push(w::banner("cloud_off", "Couldn't load cost indices from ESI: job costs leave the system index out.", w::BannerKind::Notice).upcast());
    }

    let profit_tile = |label: &str, net: Option<f64>| -> gtk::Box {
        match net {
            Some(n) => {
                let profit = n - per_unit;
                let margin = if n > 0.0 { profit / n * 100.0 } else { 0.0 };
                let t = w::stat_tile(label, &fmt::isk_short(profit * j.produced as f64), Some(&format!("{} per unit · {}% margin", fmt::isk_short(profit), fmt::decimal(margin, 1))));
                // stat_tile's second child is its value label
                if let Some(value) = t.first_child().and_then(|c| c.next_sibling()) {
                    value.add_css_class(if profit >= 0.0 { "success-text" } else { "error-text" });
                }
                t
            }
            None => w::stat_tile(label, "—", Some(&format!("No orders at {}", p.hub_name))),
        }
    };
    out.push(
        w::tile_row(vec![
            w::stat_tile("Total cost", &fmt::isk_short(total), Some(&format!("{} per unit · {} units", fmt::isk_short(per_unit), fmt::int(j.produced)))),
            profit_tile("Profit selling to sell orders", sell_net),
            profit_tile("Profit selling to buy orders", buy_net),
            w::stat_tile("Build time", &common::duration(j.time), Some(&format!("{} per run", common::duration(j.time / inputs.runs.max(1) as f64)))),
            w::stat_tile("Job cost", &fmt::isk_short(j.cost.total), Some(&format!("{}% index in {}", percent_text(j.cost.cost_index), system_name(inputs.facility.system_id)))),
        ])
        .upcast(),
    );
    let fee_note = w::line(
        &format!(
            "Prices at {}: materials at the lowest sell order; profit after {}% broker fee and {}% sales tax (sell orders) or the tax alone (buy orders).",
            p.hub_name,
            percent_text(fees.broker),
            percent_text(fees.tax)
        ),
        &["body-small", "dim"],
    );
    fee_note.set_wrap(true);
    fee_note.set_xalign(0.0);
    out.push(fee_note.upcast());

    // cost breakdown
    let mut rows = vec![
        ("Materials", fmt::isk(materials)),
        ("Estimated item value", fmt::isk(j.cost.eiv)),
        ("System cost index", format!("{}% ({})", percent_text(j.cost.cost_index), system_name(inputs.facility.system_id))),
        ("Index cost after bonuses", fmt::isk(j.cost.gross)),
        ("Facility tax", fmt::isk(j.cost.facility_tax)),
        ("SCC surcharge", fmt::isk(j.cost.scc)),
        ("Job cost", fmt::isk(j.cost.total)),
    ];
    if invention.is_some() {
        rows.push(("Invention", fmt::isk(invention_cost)));
    }
    rows.push(("Total", fmt::isk(total)));
    rows.push(("Material bonus", format!("{}% of base", fmt::decimal(j.material_multiplier * 100.0, 2))));
    rows.push(("Time bonus", format!("{}% of base", fmt::decimal(j.time_multiplier * 100.0, 2))));
    out.push(w::card_with("Cost breakdown", None, &[], &w::key_values(&rows)).upcast());

    // materials
    #[derive(Clone)]
    struct Row {
        type_id: i64,
        name: String,
        base: i64,
        units: i64,
        price: Option<f64>,
        volume: f64,
        buildable: bool,
    }
    let base: std::collections::HashMap<i64, i64> = bp.output().map(|(_, a, _)| a.materials.iter().copied().collect()).unwrap_or_default();
    let rows: Vec<Row> = j
        .materials
        .iter()
        .map(|(t, q)| Row {
            type_id: *t,
            name: INDUSTRY.type_name(*t),
            base: base.get(t).copied().unwrap_or(0),
            units: *q,
            price: p.buy_cost(*t),
            volume: MARKET.get(*t).map(|m| m.volume).unwrap_or(0.0) * *q as f64,
            buildable: INDUSTRY.maker(*t).is_some(),
        })
        .collect();
    let copy_list = mf::multibuy(&j.materials.iter().map(|(t, q)| mf::Purchase { type_id: *t, needed: *q, from_stock: 0, to_buy: *q }).collect::<Vec<_>>());
    let copy = w::button_icon("content_copy", "Copy multibuy", Btn::Text);
    {
        let ctx = ctx.clone();
        copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &copy_list));
    }
    let table = {
        let ctx = ctx.clone();
        Table::new(rows)
            .widget_sorted("Material", |r| type_cell(r.type_id, &r.name), |a, b| a.name.cmp(&b.name))
            .expand()
            .width(240)
            .num("Per run", |r| fmt::int(r.base), |r| r.base as f64)
            .num("Needed", |r| fmt::int(r.units), |r| r.units as f64)
            .num("Price", |r| r.price.map(fmt::isk_short).unwrap_or_else(|| "—".into()), |r| r.price.unwrap_or(0.0))
            .num("Total", |r| r.price.map(|p| fmt::isk_short(p * r.units as f64)).unwrap_or_else(|| "—".into()), |r| r.price.unwrap_or(0.0) * r.units as f64)
            .num("Volume", |r| format!("{} m³", fmt::decimal(r.volume, 1)), |r| r.volume)
            .text("", |r| if r.buildable { "Can be built ›".into() } else { String::new() })
            .sort_by(4, true)
            .natural_height()
            .on_activate(move |r| {
                if r.buildable {
                    common::open_in_calculator(&ctx, r.type_id);
                }
            })
            .build()
    };
    let mats = w::vbox(8);
    mats.append(&table);
    let hint = w::line("Activate a material that can be built to work it out too.", &["body-small", "dim"]);
    hint.set_xalign(0.0);
    mats.append(&hint);
    out.push(w::card_with("Materials", Some(&format!("ME {me} · {} runs", fmt::int(inputs.runs))), &[copy.upcast()], &mats).upcast());

    if let (Some(src), Some((per_run, inv))) = (source, &invention) {
        out.push(invention_card(cx, p, bp, inputs, (src, *per_run, inv), sell_net));
    }
    out
}

/// `invented`: the blueprint invented from, the invention cost per run and the invention with the chosen decryptor.
fn invention_card(cx: &Context, p: &Prices, bp: &Blueprint, inputs: &Inputs, invented: (&Blueprint, f64, &Invention), sell_net: Option<f64>) -> gtk::Widget {
    let (source, per_run, inv) = invented;
    let b = w::vbox(12);
    let rows = vec![
        ("Invented from", INDUSTRY.type_name(source.id)),
        ("Chance", format!("{}%", fmt::decimal(inv.chance * 100.0, 1))),
        ("Runs per copy", fmt::int(inv.runs)),
        ("Copy ME / TE", format!("{} / {}", inv.me, inv.te)),
        ("Attempt time", common::duration(inv.time)),
        ("Job cost per attempt", fmt::isk(inv.cost.total)),
        ("Invention cost per run", fmt::isk(per_run)),
        ("Datacores per attempt", inv.materials.iter().map(|(t, q)| format!("{q} × {}", INDUSTRY.type_name(*t))).collect::<Vec<_>>().join(", ")),
    ];
    b.append(&w::key_values(&rows));

    // Every decryptor side by side: what it does to the cost and profit of one unit.
    #[derive(Clone)]
    struct Row {
        name: String,
        chance: f64,
        runs: i64,
        me: i64,
        te: i64,
        per_unit: f64,
        profit: Option<f64>,
    }
    let options: Vec<Option<i64>> = std::iter::once(None).chain(INDUSTRY.decryptors.iter().map(|d| Some(d.id))).collect();
    let mut rows: Vec<Row> = Vec::new();
    for d in options {
        let Some((inv_per_run, inv)) = invention_per_run(cx, p, source, bp, d) else { continue };
        let Some(j) = job(cx, p, bp, inputs, inv.me, inv.te) else { continue };
        let (materials, _) = cost_of(p, &j.materials);
        let units = j.produced.max(1) as f64;
        let per_unit = (materials + j.cost.total + inv_per_run * inputs.runs as f64) / units;
        rows.push(Row {
            name: d.and_then(|d| INDUSTRY.decryptor(d)).map(|d| d.name.replace(" Decryptor", "")).unwrap_or_else(|| "None".into()),
            chance: inv.chance,
            runs: inv.runs,
            me: inv.me,
            te: inv.te,
            per_unit,
            profit: sell_net.map(|s| s - per_unit),
        });
    }
    let best = rows.iter().map(|r| r.per_unit).fold(f64::INFINITY, f64::min);
    let table = Table::new(rows)
        .text("Decryptor", move |r| if (r.per_unit - best).abs() < 1e-6 { format!("{} ★", r.name) } else { r.name.clone() })
        .expand()
        .width(200)
        .num("Chance", |r| format!("{}%", fmt::decimal(r.chance * 100.0, 1)), |r| r.chance)
        .num("Runs", |r| fmt::int(r.runs), |r| r.runs as f64)
        .num("ME", |r| r.me.to_string(), |r| r.me as f64)
        .num("TE", |r| r.te.to_string(), |r| r.te as f64)
        .num("Cost per unit", |r| fmt::isk_short(r.per_unit), |r| r.per_unit)
        .num("Profit per unit", |r| r.profit.map(fmt::isk_short).unwrap_or_else(|| "—".into()), |r| r.profit.unwrap_or(0.0))
        .sort_by(5, false)
        .natural_height()
        .build();
    b.append(&w::line("Decryptors compared (★ cheapest per unit, invention included)", &["title-small"]));
    b.append(&table);
    w::card_with("Invention", Some(&format!("Using {}", cx.levels_label)), &[], &b).upcast()
}
