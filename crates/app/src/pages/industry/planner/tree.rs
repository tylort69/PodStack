//! The plan shown job by job: totals with the job cost broken into its parts, the build tree from each target down
//! to the raw materials, and every single job with its runs, time, materials and installation cost.

use crate::pages::industry::common::{self, Prices};
use crate::pages::industry::{icon_url, type_cell};
use crate::widgets::{self as w, Table};
use gtk::prelude::*;
use podstack_core::blueprints::{Activity, INDUSTRY};
use podstack_core::features::manufacturing::{JobResult, Plan, Project};
use podstack_core::fmt;
use std::collections::HashMap;
use std::rc::Rc;

pub(super) const CSS: &str = "
.ind-tree-children { margin-left: 22px; border-left: 1px solid @outline_variant; padding-left: 8px; }
.ind-tree-leaf { margin-left: 22px; }
.ind-tree-row { padding: 2px 4px; }
";

/// Materials cost and the job costs broken into what the game charges for.
pub(super) fn totals_card(project: &Project, plan: &Plan, prices: &Prices, slots: (usize, usize, usize)) -> gtk::Box {
    let hub = &prices.hub_name;
    let (materials, unpriced) = plan.purchase_cost(&prices.buy_costs());
    let t = plan.totals();
    let total = materials + t.job_costs();
    let to_buy = plan.purchases.iter().filter(|p| p.to_buy > 0).count();
    let mut rows: Vec<(&str, String)> = vec![
        (
            "Raw materials",
            format!(
                "{} · {to_buy} item{} at {hub}'s lowest sell{}",
                fmt::isk(materials),
                if to_buy == 1 { "" } else { "s" },
                if unpriced.is_empty() { String::new() } else { format!(" ({} without a price left out)", unpriced.len()) }
            ),
        ),
        ("Estimated item value", format!("{} over all jobs (CCP's adjusted prices × the blueprints' base quantities)", fmt::isk(t.eiv))),
        ("System cost index", format!("{} (EIV × the system's index × the structure's cost bonus)", fmt::isk(t.gross))),
        ("Facility tax", fmt::isk(t.facility_tax)),
        ("SCC surcharge", fmt::isk(t.scc)),
    ];
    if t.invention > 0.0 {
        rows.push(("Invention jobs", format!("{} for the expected attempts", fmt::isk(t.invention))));
    }
    rows.push(("Installation costs", fmt::isk(t.job_costs())));
    rows.push(("Total", fmt::isk(total)));
    if let [(_, q)] = project.targets[..] {
        if q > 1 {
            rows.push(("Per unit", fmt::isk(total / q as f64)));
        }
    }
    rows.push((
        "Manufacturing",
        format!("{} job{} · {} of job time", t.manufacturing_jobs, if t.manufacturing_jobs == 1 { "" } else { "s" }, common::duration(t.manufacturing_time)),
    ));
    rows.push(("Reactions", format!("{} job{} · {} of job time", t.reaction_jobs, if t.reaction_jobs == 1 { "" } else { "s" }, common::duration(t.reaction_time))));
    if t.invention_time > 0.0 {
        rows.push(("Invention", format!("{} of job time", common::duration(t.invention_time))));
    }
    rows.push(("One job at a time", common::duration(plan.serial_time())));
    rows.push((
        "Start to finish",
        format!(
            "About {} on {} manufacturing, {} reaction and {} science slots, each job starting once its parts are done",
            common::duration(plan.estimated_time(slots)),
            slots.0,
            slots.2,
            slots.1
        ),
    ));
    w::card_with(
        "Totals",
        Some("Job cost = EIV × (system cost index × structure cost bonus + facility tax + SCC surcharge), per job."),
        &[],
        &w::key_values(&rows),
    )
}

/// What the tree needs to draw a node; shared by every node so expanding one doesn't copy the plan.
struct TreeData {
    plan: Plan,
    prices: HashMap<i64, f64>,
}

/// Each target with what it's made of, level by level down to what's bought. A part several items use has one
/// set of jobs; under each parent it shows the units that parent takes.
pub(super) fn tree_card(project: &Project, plan: &Plan, prices: &Prices) -> gtk::Box {
    crate::theme::add_css(CSS);
    let data = Rc::new(TreeData { plan: plan.clone(), prices: prices.buy_costs() });
    let col = w::vbox(2);
    for (t, q) in &project.targets {
        col.append(&node(&data, *t, *q, true));
    }
    w::card_with(
        "Build tree",
        Some("Every target down to the raw materials. Expand a part to see what it's made of; the units are what its parent uses."),
        &[],
        &col,
    )
}

fn node(data: &Rc<TreeData>, type_id: i64, units: i64, open: bool) -> gtk::Widget {
    let name = INDUSTRY.type_name(type_id);
    let Some(step) = data.plan.step(type_id) else {
        // bought
        let price = data.prices.get(&type_id).copied();
        let sub = match price {
            Some(p) => format!("Bought · {} ISK each", fmt::decimal(p, 2)),
            None => "Bought · no price".to_string(),
        };
        let row = row(type_id, &name, &sub, units, price.map(|p| p * units as f64));
        row.add_css_class("ind-tree-leaf");
        return row.upcast();
    };
    let activity = step.activity.map(|a| a.label()).unwrap_or("");
    let me_te = if step.activity == Some(Activity::Reaction) { String::new() } else { format!(" · ME {} TE {}", step.me, step.te) };
    let longest = step.job_times.iter().copied().fold(0.0, f64::max);
    let share = if step.needed > units && units > 0 { format!(" (of {} made)", fmt::int(step.produced)) } else { String::new() };
    let sub = format!(
        "{activity} · {} run{} in {} job{}{me_te} · {} per job{share}",
        fmt::int(step.runs),
        if step.runs == 1 { "" } else { "s" },
        step.jobs.len(),
        if step.jobs.len() == 1 { "" } else { "s" },
        common::duration(longest),
    );
    let head = row(type_id, &name, &sub, units, Some(step.job_cost));
    head.set_tooltip_text(Some("Right: the installation cost of all of its jobs"));
    let ex = gtk::Expander::new(None);
    ex.set_label_widget(Some(&head));
    // Children are made on the first expand: a fully drawn titan tree would be thousands of rows.
    let filled = std::cell::Cell::new(false);
    let fill = {
        let data = data.clone();
        move |ex: &gtk::Expander| {
            if filled.replace(true) {
                return;
            }
            let Some(step) = data.plan.step(type_id) else { return };
            let kids = w::vbox(2);
            kids.add_css_class("ind-tree-children");
            let mut mats = step.materials.clone();
            // parts that are built first, then what's bought, each by name
            mats.sort_by_key(|(m, _)| (data.plan.step(*m).is_none(), INDUSTRY.type_name(*m)));
            for (m, q) in mats {
                kids.append(&node(&data, m, q, false));
            }
            ex.set_child(Some(&kids));
        }
    };
    if open {
        fill(&ex);
        ex.set_expanded(true);
    }
    ex.connect_expanded_notify(move |ex| {
        if ex.is_expanded() {
            fill(ex);
        }
    });
    ex.upcast()
}

fn row(type_id: i64, name: &str, sub: &str, units: i64, isk: Option<f64>) -> gtk::Box {
    let b = w::hbox(12);
    b.add_css_class("ind-tree-row");
    let cell = type_cell(&icon_url(type_id, name, false), name, Some(sub), None);
    cell.set_hexpand(true);
    b.append(&cell);
    let n = w::line(&format!("× {}", fmt::int(units)), &["body-medium"]);
    n.set_xalign(1.0);
    n.set_width_chars(14);
    b.append(&n);
    let c = w::line(&isk.map(|v| format!("{} ISK", fmt::isk_short(v))).unwrap_or_else(|| "—".into()), &["body-medium", "dim"]);
    c.set_xalign(1.0);
    c.set_width_chars(12);
    b.append(&c);
    b
}

#[derive(Clone)]
struct JobRow {
    product: i64,
    activity: Option<Activity>,
    depth: usize,
    /// 1-based, of `count`.
    number: usize,
    count: usize,
    facility: String,
    job: JobResult,
}

/// Every job to install, deepest first, with what each costs to install and what it takes.
pub(super) fn jobs_card(plan: &Plan) -> gtk::Box {
    let rows: Vec<JobRow> = plan
        .steps
        .iter()
        .flat_map(|s| {
            s.details.iter().enumerate().map(move |(i, j)| JobRow {
                product: s.product,
                activity: s.activity,
                depth: s.depth,
                number: i + 1,
                count: s.details.len(),
                facility: s.facility.clone(),
                job: j.clone(),
            })
        })
        .collect();
    let n = rows.len();
    let table = Table::new(rows)
        .widget_sorted(
            "Job",
            |r| {
                let name = INDUSTRY.type_name(r.product);
                let of = if r.count > 1 { format!(" · job {} of {}", r.number, r.count) } else { String::new() };
                let sub = format!("{} · level {}{of}", r.activity.map(|a| a.label()).unwrap_or(""), r.depth);
                type_cell(&icon_url(r.product, &name, false), &name, Some(&sub), None)
            },
            |a, b| b.depth.cmp(&a.depth).then_with(|| INDUSTRY.type_name(a.product).cmp(&INDUSTRY.type_name(b.product))).then(a.number.cmp(&b.number)),
        )
        .expand()
        .width(240)
        .num("Runs", |r| fmt::int(r.job.runs), |r| r.job.runs as f64)
        .width(60)
        .num("Makes", |r| fmt::int(r.job.produced), |r| r.job.produced as f64)
        .width(80)
        .num("Time", |r| common::duration(r.job.time), |r| r.job.time)
        .width(90)
        .widget("Materials", |r| {
            let list: Vec<String> = r.job.materials.iter().map(|(t, q)| format!("{} × {}", fmt::int(*q), INDUSTRY.type_name(*t))).collect();
            let l = w::line(&list.join(", "), &["body-small"]);
            l.set_tooltip_text(Some(&list.join("\n")));
            l.upcast()
        })
        .width(260)
        .num("EIV", |r| fmt::isk_short(r.job.cost.eiv), |r| r.job.cost.eiv)
        .width(80)
        .num("Index", |r| format!("{:.2}%", r.job.cost.cost_index * 100.0), |r| r.job.cost.cost_index)
        .width(70)
        .num("Index cost", |r| fmt::isk_short(r.job.cost.gross), |r| r.job.cost.gross)
        .width(90)
        .num("Tax", |r| fmt::isk_short(r.job.cost.facility_tax), |r| r.job.cost.facility_tax)
        .width(80)
        .num("SCC", |r| fmt::isk_short(r.job.cost.scc), |r| r.job.cost.scc)
        .width(80)
        .num("Install cost", |r| fmt::isk_short(r.job.cost.total), |r| r.job.cost.total)
        .width(100)
        .text("Facility", |r| r.facility.clone())
        .width(160)
        .max_height(560)
        .empty_text("No jobs.")
        .build();
    w::card_with(
        "Every job",
        Some(&format!(
            "{n} job{} to install, deepest first. Materials are rounded up per job, the way the game does it; hover a job's materials for the full list.",
            if n == 1 { "" } else { "s" }
        )),
        &[],
        &table,
    )
}
