//! Character tab: assets, grouped into expandable cards per station / structure / system, with a search over item
//! names, ship and container names and locations. Values are EVE's universe-wide average prices (blueprint copies
//! count as 0). A location's items are only built when it's opened, so big hangars stay responsive.

use crate::ctx::{Ctx, Page};
use crate::pages::industry::{icon_url, keep_focus, on_search, place_sub, scope_failed, type_cell};
use crate::widgets::{self as w, Status, Table};
use gtk::prelude::*;
use podstack_core::domains::assets::{Asset, AssetsData, SCOPE};
use podstack_core::features::industry::{self as ind, AssetGroup};
use podstack_core::fmt;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// Rows shown per location before asking the user to narrow the search.
const ROW_LIMIT: usize = 300;
/// Locations opened by themselves while searching (each builds its table).
const AUTO_OPEN: usize = 8;

const CSS: &str = r#"
button.asset-group-head { border-radius: 16px; padding: 12px 16px; min-height: 56px; color: @on_surface; }
button.asset-group-head:hover { background-color: alpha(@on_surface, 0.06); }
.asset-group-head .chevron { color: @on_surface_variant; transition: transform 150ms ease; }
.asset-group-body { padding: 0 12px 12px 12px; }
"#;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    crate::theme::add_css(CSS);
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Assets", body) };
    if scope_failed(&c, "assets") {
        body.append(&w::scope_notice(SCOPE));
        return Page::new("Assets", body);
    }
    let Some(data) = c.assets.clone() else {
        body.append(&w::empty_state("hourglass_empty", "Assets not loaded yet", "They'll appear after the next refresh from ESI."));
        return Page::new("Assets", body);
    };
    if data.items.is_empty() {
        body.append(&w::empty_state("inventory_2", "No assets", "This character doesn't own any items."));
        return Page::new("Assets", body);
    }
    let data = Arc::new(data);

    let locations: HashSet<i64> = data.items.iter().map(|a| a.root_location_id).collect();
    let ships = data.items.iter().filter(|a| a.custom_name.is_some() && a.parent_id.is_none()).count();
    let updated = c.refreshes.get("assets").and_then(|r| r.last);
    body.append(&w::tile_row(vec![
        w::stat_tile("Items", &fmt::int(data.items.len() as i64), Some(&format!("{} named ships & containers", ships))),
        w::stat_tile("Estimated value", &format!("{} ISK", fmt::isk_short(data.total_value())), Some("EVE average prices")),
        w::stat_tile("Locations", &fmt::int(locations.len() as i64), None),
        w::stat_tile("Updated", &updated.map(fmt::relative).unwrap_or_else(|| "—".into()), updated.map(fmt::datetime).as_deref()),
    ]));

    let search_key = format!("assets.{id}.search");
    let search = w::search_entry("Search items, ship or container names, locations…");
    let initial = ctx.recall(&search_key).unwrap_or_default();
    search.set_text(&initial);
    keep_focus(ctx, &search, &search_key);
    let summary = w::line("", &["body-medium", "dim"]);
    let bar = w::hbox(16);
    bar.append(&search);
    bar.append(&summary);
    body.append(&bar);

    let groups_box = w::vbox(12);
    body.append(&groups_box);

    let state = Rc::new(State { ctx: ctx.clone(), id, data, open: RefCell::new(recall_open(ctx, id)) });
    let render = {
        let state = state.clone();
        let groups_box = groups_box.clone();
        let summary = summary.clone();
        move |text: &str| {
            let query = ind::normalize_query(text);
            let groups = ind::group_assets(&state.data, &query);
            let shown: usize = groups.iter().map(|g| g.items.len()).sum();
            summary.set_text(&if query.is_empty() {
                String::new()
            } else {
                format!(
                    "{} {} in {} {}",
                    fmt::int(shown as i64),
                    if shown == 1 { "match" } else { "matches" },
                    groups.len(),
                    if groups.len() == 1 { "location" } else { "locations" }
                )
            });
            while let Some(c) = groups_box.first_child() {
                groups_box.remove(&c);
            }
            if groups.is_empty() {
                groups_box.append(&w::empty_state("search_off", "Nothing matches", "Try an item, ship or station name."));
            }
            for (i, g) in groups.into_iter().enumerate() {
                let open = match state.open.borrow().get(&g.location_id) {
                    Some(v) => *v,
                    None => !query.is_empty() && i < AUTO_OPEN,
                };
                groups_box.append(&group_card(&state, g, open));
            }
        }
    };
    render(&initial);
    let ctx2 = ctx.clone();
    on_search(&search, move |text| {
        ctx2.remember(&search_key, text.clone());
        render(&text);
    });
    Page::new("Assets", body)
}

struct State {
    ctx: Ctx,
    id: i64,
    data: Arc<AssetsData>,
    /// Locations the user opened (true) or closed (false); others follow the default.
    open: RefCell<HashMap<i64, bool>>,
}

impl State {
    fn set_open(&self, location: i64, open: bool) {
        self.open.borrow_mut().insert(location, open);
        let list: Vec<String> = self.open.borrow().iter().map(|(k, v)| format!("{k}:{}", *v as u8)).collect();
        self.ctx.remember(&format!("assets.{}.open", self.id), list.join(","));
    }
}

fn recall_open(ctx: &Ctx, id: i64) -> HashMap<i64, bool> {
    ctx.recall(&format!("assets.{id}.open"))
        .unwrap_or_default()
        .split(',')
        .filter_map(|p| {
            let (k, v) = p.split_once(':')?;
            Some((k.parse().ok()?, v == "1"))
        })
        .collect()
}

fn location_icon(kind: &str) -> &'static str {
    match kind {
        "station" => "apartment",
        "structure" => "domain",
        "system" => "public",
        _ => "place",
    }
}

fn group_card(state: &Rc<State>, g: AssetGroup, open: bool) -> gtk::Box {
    let card = w::vbox(0);
    card.add_css_class("card");
    card.add_css_class("flush");

    let place = state.data.locations.get(&g.location_id);
    let head = gtk::Button::new();
    head.add_css_class("asset-group-head");
    let row = w::hbox(16);
    let chevron = w::icon(if open { "expand_less" } else { "expand_more" });
    chevron.add_css_class("chevron");
    row.append(&w::icon(location_icon(place.map(|p| p.kind.as_str()).unwrap_or(""))));
    let titles = w::vbox(2);
    titles.set_hexpand(true);
    titles.append(&w::line(&g.name, &["title-small"]));
    if let Some(sub) = place.and_then(place_sub) {
        titles.append(&w::line(&sub, &["body-small", "dim"]));
    }
    row.append(&titles);
    let n = g.items.len();
    row.append(&w::line(&format!("{} {}", fmt::int(n as i64), if n == 1 { "item" } else { "items" }), &["body-medium", "dim", "numeric"]));
    let value = w::line(&format!("{} ISK", fmt::isk_short(g.value)), &["title-small", "numeric"]);
    value.set_width_chars(10);
    value.set_xalign(1.0);
    row.append(&value);
    row.append(&chevron);
    head.set_child(Some(&row));
    card.append(&head);

    let revealer = gtk::Revealer::new();
    revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
    card.append(&revealer);
    let built = Rc::new(RefCell::new(false));
    let g = Rc::new(g);
    let fill = {
        let state = state.clone();
        let revealer = revealer.clone();
        let g = g.clone();
        move || {
            if !*built.borrow() {
                *built.borrow_mut() = true;
                revealer.set_child(Some(&group_body(&state, &g)));
            }
        }
    };
    if open {
        fill();
        revealer.set_transition_duration(0);
        revealer.set_reveal_child(true);
        revealer.set_transition_duration(200);
    }
    let state2 = state.clone();
    head.connect_clicked(move |_| {
        let now_open = !revealer.reveals_child();
        if now_open {
            fill();
        }
        revealer.set_reveal_child(now_open);
        chevron.set_text(if now_open { "expand_less" } else { "expand_more" });
        state2.set_open(g.location_id, now_open);
    });
    card
}

#[derive(Clone)]
struct Row {
    asset: Asset,
    parent: Option<String>,
}

fn group_body(state: &State, g: &AssetGroup) -> gtk::Widget {
    let by_id = state.data.by_id();
    let rows: Vec<Row> = g
        .items
        .iter()
        .take(ROW_LIMIT)
        .map(|&i| {
            let a = &state.data.items[i];
            Row { asset: a.clone(), parent: a.parent_id.and_then(|p| by_id.get(&p)).map(|p| p.label()) }
        })
        .collect();
    let b = w::vbox(8);
    b.add_css_class("asset-group-body");
    let table = Table::new(rows)
        .widget_sorted(
            "Item",
            |r| {
                let pill = r.asset.is_blueprint_copy.then(|| w::status("BPC", Status::Info));
                type_cell(&icon_url(r.asset.type_id, &r.asset.name, r.asset.is_blueprint_copy), &r.asset.label(), None, pill)
            },
            |a, b| a.asset.label().to_lowercase().cmp(&b.asset.label().to_lowercase()),
        )
        .expand()
        .text("Where", |r| match &r.parent {
            Some(p) => format!("{} · {p}", ind::flag_label(&r.asset.flag)),
            None => ind::flag_label(&r.asset.flag),
        })
        .expand()
        .num("Quantity", |r| fmt::int(r.asset.count()), |r| r.asset.count() as f64)
        .num(
            "Value",
            |r| if r.asset.value() > 0.0 { format!("{} ISK", fmt::isk_short(r.asset.value())) } else { "—".into() },
            |r| r.asset.value(),
        )
        .natural_height()
        .build();
    b.append(&table);
    if g.items.len() > ROW_LIMIT {
        b.append(&w::text(
            &format!("Showing the {ROW_LIMIT} most valuable of {} items. Search to narrow it down.", fmt::int(g.items.len() as i64)),
            &["body-small", "dim"],
        ));
    }
    b.upcast()
}
