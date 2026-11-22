//! Industry → Systems: every system's live industry cost indices from ESI, to find a cheap place to build. Filter
//! by name, region and security, sort by any activity, and turn a system into a facility in one click.

use super::common::CSS;
use super::form::security_label;
use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::rt;
use crate::widgets::{self as w, Table};
use gtk::prelude::*;
use podstack_core::blueprints::Activity;
use podstack_core::features::manufacturing as mf;
use podstack_core::market::CostIndices;
use podstack_core::sde::{SecurityClass, MAP};
use podstack_core::fmt;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

const SEARCH_KEY: &str = "industry.systems.search";
const SEC_KEY: &str = "industry.systems.security";
const REGION_KEY: &str = "industry.systems.region";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&tab_bar(ctx, Tab::Systems));

    let search = w::search_entry("System name");
    search.set_text(&ctx.recall(SEARCH_KEY).unwrap_or_default());
    search.set_hexpand(true);
    let sec_labels = ["All security", "High sec", "Low sec", "Null sec"];
    let sec = w::dropdown(&sec_labels, ctx.recall(SEC_KEY).and_then(|s| s.parse().ok()).unwrap_or(0));
    let regions: Vec<(i64, String)> = std::iter::once((0, "All regions".to_string())).chain(MAP.regions.iter().cloned()).collect();
    let region_labels: Vec<&str> = regions.iter().map(|r| r.1.as_str()).collect();
    let region_now = ctx.recall(REGION_KEY).and_then(|s| s.parse::<i64>().ok()).and_then(|id| regions.iter().position(|r| r.0 == id)).unwrap_or(0) as u32;
    let region = w::dropdown(&region_labels, region_now);
    let bar = w::hbox(12);
    bar.append(&search);
    bar.append(&sec);
    bar.append(&region);
    body.append(&bar);

    let tiles = w::vbox(0);
    body.append(&tiles);
    let (holder_sw, holder) = w::async_holder();
    body.append(&holder_sw);
    holder.append(&w::line("Loading cost indices…", &["dim"]));

    let data: Rc<RefCell<Option<Arc<CostIndices>>>> = Rc::default();
    let render: Rc<dyn Fn()> = {
        let (ctx, data, holder, tiles, search, sec, region, regions) = (ctx.clone(), data.clone(), holder.clone(), tiles.clone(), search.clone(), sec.clone(), region.clone(), regions.clone());
        Rc::new(move || {
            let Some(all) = data.borrow().clone() else { return };
            let q = search.text().trim().to_lowercase();
            let sec_filter = match sec.selected() {
                1 => Some(SecurityClass::High),
                2 => Some(SecurityClass::Low),
                3 => Some(SecurityClass::Null),
                _ => None,
            };
            let region_filter = regions.get(region.selected() as usize).map(|r| r.0).filter(|r| *r != 0);
            ctx.remember(SEARCH_KEY, q.clone());
            ctx.remember(SEC_KEY, sec.selected().to_string());
            ctx.remember(REGION_KEY, region_filter.unwrap_or(0).to_string());
            let rows: Vec<Row> = all
                .keys()
                .filter_map(|id| MAP.system(*id))
                .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q))
                .filter(|s| sec_filter.is_none_or(|c| s.security_class() == c))
                .filter(|s| region_filter.is_none_or(|r| s.region == r))
                .map(|s| Row {
                    system: s.id,
                    name: s.name.clone(),
                    region: MAP.region_name(s.region).unwrap_or("").to_string(),
                    security: s.security,
                    index: Activity::ALL.map(|a| mf::cost_index(Some(&all), s.id, a)),
                })
                .collect();
            fill_tiles(&tiles, &rows);
            while let Some(c) = holder.first_child() {
                holder.remove(&c);
            }
            holder.append(&table(&ctx, rows));
        })
    };
    {
        let render = render.clone();
        search.connect_search_changed(move |_| render());
    }
    for d in [&sec, &region] {
        let render = render.clone();
        d.connect_selected_notify(move |_| render());
    }
    let core = ctx.core().clone();
    let holder2 = holder.clone();
    rt::run(async move { core.market.cost_indices(&core.esi, false).await }, move |r| match r {
        Ok(all) => {
            *data.borrow_mut() = Some(all);
            render();
        }
        Err(err) => {
            while let Some(c) = holder2.first_child() {
                holder2.remove(&c);
            }
            holder2.append(&w::empty_state("cloud_off", "Couldn't load cost indices", &err.to_string()));
        }
    });
    Page::new("Industry", body).static_page().no_scroll()
}

#[derive(Clone)]
struct Row {
    system: i64,
    name: String,
    region: String,
    security: f64,
    /// In `Activity::ALL` order.
    index: [f64; 6],
}

fn pct(v: f64) -> String {
    format!("{}%", fmt::decimal(v * 100.0, 2))
}

fn fill_tiles(tiles: &gtk::Box, rows: &[Row]) {
    while let Some(c) = tiles.first_child() {
        tiles.remove(&c);
    }
    let mut manuf: Vec<&Row> = rows.iter().filter(|r| r.index[0] > 0.0).collect();
    manuf.sort_by(|a, b| a.index[0].total_cmp(&b.index[0]));
    let cheapest = |i: usize| -> (String, String) {
        rows.iter()
            .filter(|r| r.index[i] > 0.0)
            .min_by(|a, b| a.index[i].total_cmp(&b.index[i]))
            .map(|r| (pct(r.index[i]), r.name.clone()))
            .unwrap_or_else(|| ("—".into(), "No data".into()))
    };
    let median = manuf.get(manuf.len() / 2).map(|r| pct(r.index[0])).unwrap_or_else(|| "—".into());
    let (m, ms) = cheapest(0);
    let (r, rs) = cheapest(1);
    let (i, is) = cheapest(2);
    tiles.append(&w::tile_row(vec![
        w::stat_tile("Cheapest manufacturing", &m, Some(&ms)),
        w::stat_tile("Cheapest reactions", &r, Some(&rs)),
        w::stat_tile("Cheapest invention", &i, Some(&is)),
        w::stat_tile("Median manufacturing", &median, Some(&format!("{} systems shown", fmt::int(rows.len() as i64)))),
    ]));
}

fn table(ctx: &Ctx, rows: Vec<Row>) -> gtk::Widget {
    let ctx = ctx.clone();
    let mut t = Table::new(rows)
        .text("System", |r| r.name.clone())
        .expand()
        .width(150)
        .widget_sorted("Sec", |r| security_label(r.system).upcast(), |a, b| a.security.total_cmp(&b.security))
        .width(60)
        .text("Region", |r| r.region.clone())
        .width(150);
    for (i, a) in Activity::ALL.iter().enumerate() {
        t = t.num(a.label(), move |r| pct(r.index[i]), move |r| r.index[i]).width(110);
    }
    t.sort_by(3, false)
        .empty_text("No systems match")
        .on_activate(move |r| super::facilities::new_facility_in(&ctx, r.system))
        .build()
}
