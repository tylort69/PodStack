//! Character tab: clones. The active clone and every jump clone, grouped by the station or structure they're stored
//! in, with the implants' estimated value at Jita 4-4's lowest sell prices.

use super::summary::parts::{self, card, clear, collapsible, dim_text, fit, sec_label, type_cell};
use crate::ctx::{Ctx, Page};
use crate::rt;
use crate::widgets::{self as w, Btn, Status};
use gtk::prelude::*;
use podstack_core::domains::clones::{Implant, JumpClone};
use podstack_core::features::character_extras as extras;
use podstack_core::market::{JITA_44, THE_FORGE};
use podstack_core::{config, fmt, Character};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};

type Prices = HashMap<i64, Option<f64>>;

thread_local! {
    /// Jita prices seen this session, so a page rebuild shows them at once.
    static PRICES: RefCell<Prices> = RefCell::new(HashMap::new());
}

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let Some(c) = ctx.core().character(id) else { return Page::new("Clones", w::vbox(16)) };
    if c.clones.is_none() {
        return Page::new("Clones", parts::pending_page(&c, "clones", "esi-clones.read_clones.v1", "jump clones", "people_outline"));
    }
    let holder = w::vbox(16);
    let ids = implant_ids(&c);
    let known: Option<Prices> = PRICES.with(|p| {
        let p = p.borrow();
        ids.iter().all(|i| p.contains_key(i)).then(|| ids.iter().map(|i| (*i, p[i])).collect())
    });
    let have = known.is_some();
    fill(ctx, &holder, id, known, false);
    if !have {
        load_prices(ctx, &holder, id, false);
    }
    Page::new("Clones", holder)
}

fn implant_ids(c: &Character) -> Vec<i64> {
    let active = c.implants.iter().flatten();
    let stored = c.clones.iter().flat_map(|cl| cl.jump_clones.iter().flat_map(|j| j.implants.iter()));
    active.chain(stored).map(|i| i.type_id).collect::<HashSet<_>>().into_iter().collect()
}

fn load_prices(ctx: &Ctx, holder: &gtk::Box, id: i64, force: bool) {
    let Some(c) = ctx.core().character(id) else { return };
    let ids = implant_ids(&c);
    let core = ctx.core().clone();
    let (ctx, holder) = (ctx.clone(), holder.clone());
    if force {
        let p = PRICES.with(|p| p.borrow().clone());
        fill(&ctx, &holder, id, Some(p), true);
    }
    rt::run(async move { core.market.jita_sell(&core.esi, &ids, force).await }, move |prices| {
        PRICES.with(|p| p.borrow_mut().extend(prices.iter().map(|(k, v)| (*k, *v))));
        if holder.root().is_some() {
            fill(&ctx, &holder, id, Some(prices), false);
        }
    });
}

fn value_text(implants: &[&Implant], prices: &Option<Prices>) -> Option<(String, Option<String>)> {
    let p = prices.as_ref()?;
    let (value, unpriced) = extras::implants_value(implants.iter().map(|i| i.type_id), p);
    let star = if unpriced > 0 { "*" } else { "" };
    let tip = (unpriced > 0).then(|| format!("{unpriced} without a sell order at Jita 4-4, not counted"));
    Some((format!("~{} ISK{star}", fmt::isk_short(value)), tip))
}

fn value_label(implants: &[&Implant], prices: &Option<Prices>) -> Option<gtk::Label> {
    let (t, tip) = value_text(implants, prices)?;
    let l = fit(&t, &["body-medium", "numeric", "dim"]);
    l.set_tooltip_text(tip.as_deref());
    Some(l)
}

fn fill(ctx: &Ctx, holder: &gtk::Box, id: i64, prices: Option<Prices>, updating: bool) {
    clear(holder);
    let Some(c) = ctx.core().character(id) else { return };
    let Some(clones) = &c.clones else { return };
    let active: Vec<&Implant> = c.implants.iter().flatten().collect();
    let all: Vec<&Implant> = active.iter().copied().chain(clones.jump_clones.iter().flat_map(|j| j.implants.iter())).collect();

    // headline tiles
    let jump = match extras::clone_jump_wait(&c, chrono::Utc::now()) {
        None => w::stat_tile("Next clone jump", "Now", None),
        Some(d) => w::stat_tile("Next clone jump", &fmt::time_until(d), Some(&fmt::datetime(d))),
    };
    let home = clones.home.as_ref().map(|h| h.name.clone()).unwrap_or_else(|| "—".into());
    let home_tile = w::stat_tile("Home station", &home, clones.home.as_ref().and_then(|h| h.system_name.clone()).as_deref());
    // a station name is long for a stat value: show it smaller, in full on hover
    if let Some(v) = home_tile.first_child().and_then(|l| l.next_sibling()) {
        v.set_tooltip_text(Some(&home));
        v.remove_css_class("stat-value");
        v.add_css_class("title-medium");
        v.set_margin_top(6);
        if let Some(l) = v.downcast_ref::<gtk::Label>() {
            l.set_max_width_chars(24);
        }
    }
    let total = prices.as_ref().map(|p| extras::implants_value(all.iter().map(|i| i.type_id), p));
    let price_date = ctx.core().market.price_date(THE_FORGE, Some(JITA_44), &implant_ids(&c));
    let sub = match price_date {
        Some(d) => format!("Jita 4-4 sell · {}", d.with_timezone(&chrono::Local).format("%H:%M")),
        None => "Jita 4-4 sell".to_string(),
    };
    let value_tile = w::stat_tile("Implants value", &total.map(|(v, _)| format!("{} ISK", fmt::isk_short(v))).unwrap_or_else(|| "…".into()), Some(&sub));
    let update = w::button(if updating || prices.is_none() { "Updating…" } else { "Update prices" }, Btn::Text);
    update.set_halign(gtk::Align::Start);
    update.set_sensitive(!updating && prices.is_some());
    {
        let (ctx, holder) = (ctx.clone(), holder.clone());
        update.connect_clicked(move |_| load_prices(&ctx, &holder, id, true));
    }
    value_tile.append(&update);
    holder.append(&w::tile_row(vec![
        w::stat_tile("Jump clones", &format!("{} / {}", clones.jump_clones.len(), c.max_jump_clones()), None),
        jump,
        home_tile,
        value_tile,
    ]));

    // active clone
    let current = c.location.as_ref().map(|l| match &l.docked {
        Some(d) => d.name.clone(),
        None => format!("In space, {}", l.system.name),
    });
    let mut sub = current.unwrap_or_default();
    if let Some((v, _)) = value_text(&active, &prices) {
        if !sub.is_empty() {
            sub.push_str(" · ");
        }
        sub.push_str(&v);
    }
    let active_body: gtk::Widget = if c.implants.is_none() {
        parts::pending(&c, "implants", "esi-clones.read_implants.v1", "implants")
    } else {
        implant_list(&active, &prices)
    };
    holder.append(&card("Active Clone", "person", (!sub.is_empty()).then_some(sub.as_str()), &[], &active_body));

    // jump clones by location
    let mut groups: BTreeMap<i64, Vec<&JumpClone>> = BTreeMap::new();
    for jc in &clones.jump_clones {
        groups.entry(jc.location_id).or_default().push(jc);
    }
    let mut groups: Vec<(i64, Vec<&JumpClone>)> = groups.into_iter().collect();
    groups.sort_by_key(|(_, list)| list[0].place.name.to_lowercase());
    let body = w::vbox(4);
    if groups.is_empty() {
        body.append(&dim_text("No jump clones."));
    }
    let home_id = clones.home.as_ref().map(|h| h.id);
    for (loc, list) in &groups {
        let place = &list[0].place;
        let head = w::hbox(12);
        if let Some(s) = place.security {
            head.append(&sec_label(s));
        }
        let name = w::line(&place.name, &["body-large"]);
        name.set_hexpand(true);
        head.append(&name);
        if home_id == Some(*loc) {
            head.append(&w::status("Home", Status::Info));
        }
        let implants: Vec<&Implant> = list.iter().flat_map(|j| j.implants.iter()).collect();
        let mut count = format!("{} {}", list.len(), if list.len() == 1 { "clone" } else { "clones" });
        if let Some((v, _)) = value_text(&implants, &prices).filter(|_| !implants.is_empty()) {
            count.push_str(&format!(" · {v}"));
        }
        head.append(&fit(&count, &["body-medium", "numeric", "dim"]));

        let clones_box = w::vbox(12);
        for jc in list {
            let item = w::vbox(6);
            let title = w::hbox(8);
            title.append(&w::line(jc.name.as_deref().filter(|n| !n.is_empty()).unwrap_or("Unnamed Clone"), &["title-small"]));
            let n = jc.implants.len();
            let cnt = w::line(&format!("· {n} {}", if n == 1 { "implant" } else { "implants" }), &["body-medium", "dim"]);
            cnt.set_hexpand(true);
            title.append(&cnt);
            let imps: Vec<&Implant> = jc.implants.iter().collect();
            if n > 0 {
                if let Some(v) = value_label(&imps, &prices) {
                    title.append(&v);
                }
            }
            item.append(&title);
            item.append(&implant_list(&imps, &prices));
            clones_box.append(&item);
        }
        body.append(&collapsible(ctx, &format!("clones.loc.{loc}"), true, 36, &head.upcast(), &clones_box.upcast()));
    }
    let sub = format!("{} {}", groups.len(), if groups.len() == 1 { "location" } else { "locations" });
    holder.append(&card("Jump Clones", "people_outline", Some(&sub), &[], &body));
}

fn implant_list(implants: &[&Implant], prices: &Option<Prices>) -> gtk::Widget {
    if implants.is_empty() {
        return dim_text("No implants").upcast();
    }
    let mut sorted = implants.to_vec();
    sorted.sort_by_key(|i| i.slot.unwrap_or(99));
    let g = gtk::Grid::new();
    g.set_column_spacing(16);
    g.set_row_spacing(6);
    for (r, imp) in sorted.iter().enumerate() {
        let r = r as i32;
        let slot = fit(&imp.slot.map(|s| format!("Slot {s}")).unwrap_or_default(), &["body-small", "dim", "numeric"]);
        slot.set_width_chars(7);
        g.attach(&slot, 0, r, 1, 1);
        let name = if imp.name.is_empty() { format!("Type #{}", imp.type_id) } else { imp.name.clone() };
        let cell = type_cell(&w::eve_image(&config::type_icon(imp.type_id, 64), 28).upcast(), &name, None);
        g.attach(&cell, 1, r, 1, 1);
        let price = match prices.as_ref().map(|p| p.get(&imp.type_id).copied().flatten()) {
            None => String::new(),
            Some(Some(v)) => format!("{} ISK", fmt::isk_short(v)),
            Some(None) => "—".into(),
        };
        let pl = fit(&price, &["body-medium", "numeric", "dim"]);
        pl.set_xalign(1.0);
        if price == "—" {
            pl.set_tooltip_text(Some("No sell order at Jita 4-4"));
        }
        g.attach(&pl, 2, r, 1, 1);
    }
    g.upcast()
}
