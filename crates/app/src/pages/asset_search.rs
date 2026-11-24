//! Asset Search: every character's assets in one search ("where is my Orca?"). Before anything is typed it lists
//! the most valuable items. Values are EVE's universe-wide average prices, as on the Assets tab. (Each pilot's stock
//! of doctrine ships is on the Doctrines page, in its Stock tab.)

use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::pages::industry::{icon_url, char_cell, on_search, place_cell, type_cell};
use crate::widgets::{self as w, Status, Table};
use gtk::prelude::*;
use podstack_core::features::industry::{self as ind, AssetHit};
use podstack_core::{fmt, Event};
use std::cell::RefCell;
use std::rc::Rc;

const ROW_LIMIT: usize = 500;
/// Shown before anything is searched for.
const TOP_ITEMS: usize = 50;
const KEY: &str = "asset_search.query";

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.set_vexpand(true);
    let chars = ctx.core().characters();
    let initial = ctx.recall(KEY).unwrap_or_default();

    let tiles = w::hbox(0);
    body.append(&tiles);
    let missing = chars.iter().filter(|c| c.assets.is_none()).count();
    if missing > 0 {
        body.append(&w::banner(
            "info",
            &format!("{missing} character(s) have no assets loaded (missing \"Read Assets\", or not refreshed yet)."),
            w::BannerKind::Info,
        ));
    }

    let search = w::search_entry("Search every character: items, ship or container names, stations…");
    search.set_text(&initial);
    body.append(&search);
    let heading = w::hbox(8);
    let heading_icon = w::icon("diamond");
    let heading_text = w::text("", &["title-medium"]);
    heading.append(&heading_icon);
    heading.append(&heading_text);
    body.append(&heading);
    let results = w::vbox(8);
    results.set_vexpand(true);
    body.append(&results);

    let render = Rc::new({
        let ctx = ctx.clone();
        move |text: &str| {
            let query = ind::normalize_query(text);
            let limit = if query.is_empty() { TOP_ITEMS } else { ROW_LIMIT };
            let s = ind::search_assets(&ctx.core().characters(), &query, limit);

            while let Some(c) = tiles.first_child() {
                tiles.remove(&c);
            }
            let mut list = vec![
                w::stat_tile("Items", &fmt::int(s.items as i64), Some(&format!("{} characters", s.characters_loaded))),
                w::stat_tile("Estimated value", &format!("{} ISK", fmt::isk_short(s.total_value)), Some("EVE average prices")),
            ];
            if !query.is_empty() {
                let t = w::stat_tile("Matches", &fmt::int(s.matches as i64), Some(&format!("~{} ISK", fmt::isk_short(s.match_value))));
                t.add_css_class("accent");
                list.push(t);
            }
            let row = w::tile_row(list);
            row.set_hexpand(true);
            tiles.append(&row);

            heading_icon.set_text(if query.is_empty() { "diamond" } else { "search" });
            heading_text.set_text(if query.is_empty() { "Most valuable items" } else { "Results" });

            while let Some(c) = results.first_child() {
                results.remove(&c);
            }
            if s.hits.is_empty() {
                results.append(&if query.is_empty() {
                    w::empty_state("inventory_2", "No assets loaded yet", "Assets appear once characters with \"Read Assets\" have refreshed.")
                } else {
                    w::empty_state("search_off", "Nothing matches", "Try an item, ship or station name.")
                });
                return;
            }
            if s.matches > s.hits.len() && !query.is_empty() {
                results.append(&w::text(
                    &format!("Showing the {} most valuable of {} matches. Search more precisely to narrow it down.", s.hits.len(), fmt::int(s.matches as i64)),
                    &["body-small", "dim"],
                ));
            }
            results.append(&table(&ctx, s.hits));
        }
    });
    render(&initial);
    let ctx2 = ctx.clone();
    let r2 = render.clone();
    on_search(&search, move |text| {
        ctx2.remember(KEY, text.clone());
        r2(&text);
    });

    // The page updates itself, so new data doesn't rebuild it under the user's typing: only an assets refresh
    // re-runs the search.
    let seen = RefCell::new(fingerprint(ctx));
    let ctx2 = ctx.clone();
    let entry = search.clone();
    ctx.on_event(&body, move |e| {
        if matches!(e, Event::CharacterUpdated(_) | Event::CharactersChanged) {
            let now = fingerprint(&ctx2);
            if *seen.borrow() != now {
                *seen.borrow_mut() = now;
                render(&entry.text());
            }
        }
    });
    let entry = search.clone();
    gtk::glib::idle_add_local_once(move || {
        entry.grab_focus();
        entry.set_position(-1);
    });
    Page::new("Asset Search", body).no_scroll().static_page()
}

/// When each character's assets were last refreshed.
fn fingerprint(ctx: &Ctx) -> Vec<(i64, Option<chrono::DateTime<chrono::Utc>>)> {
    ctx.core().characters().iter().map(|c| (c.id, c.assets.as_ref().and(c.refreshes.get("assets").and_then(|r| r.last)))).collect()
}

fn table(ctx: &Ctx, hits: Vec<AssetHit>) -> gtk::Widget {
    let ctx = ctx.clone();
    Table::new(hits)
        .widget_sorted(
            "Item",
            |h| {
                let pill = h.asset.is_blueprint_copy.then(|| w::status("BPC", Status::Info));
                type_cell(&icon_url(h.asset.type_id, &h.asset.name, h.asset.is_blueprint_copy), &h.asset.label(), None, pill)
            },
            |a, b| a.asset.label().to_lowercase().cmp(&b.asset.label().to_lowercase()),
        )
        .expand()
        .num("Quantity", |h| fmt::int(h.asset.count()), |h| h.asset.count() as f64)
        .widget_sorted("Character", |h| char_cell(h.character_id, &h.character_name), |a, b| a.character_name.cmp(&b.character_name))
        .widget_sorted(
            "Where",
            |h| place_cell(h.location.clone(), h.parent.as_ref().map(|p| format!("in {p}"))),
            |a, b| a.location.cmp(&b.location),
        )
        .expand()
        .num(
            "Value",
            |h| if h.asset.value() > 0.0 { format!("{} ISK", fmt::isk_short(h.asset.value())) } else { "—".into() },
            |h| h.asset.value(),
        )
        .sort_by(4, true)
        .on_activate(move |h| {
            // open the character's assets searching for this item
            ctx.remember(&format!("assets.{}.search", h.character_id), h.asset.label());
            ctx.navigate(Route::Character(h.character_id, CharTab::Assets));
        })
        .build()
}
