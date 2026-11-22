//! Character tab: kill log. The 50 most recent kills and losses, each linking to zKillboard.

use super::summary::parts::{self, clear, sec_label, type_cell};
use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Btn, Status};
use gtk::prelude::*;
use podstack_core::domains::social::{Killmail, KILLMAIL_LIMIT};
use podstack_core::features::character_extras as extras;
use podstack_core::{config, fmt};

const VIEWS: [&str; 3] = ["All", "Kills", "Losses"];

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Kill Log", body) };
    let Some(data) = &c.killmails else {
        return Page::new("Kill Log", parts::pending_page(&c, "killmails", "esi-killmails.read_killmails.v1", "killmails", "gps_fixed"));
    };
    let kills = data.kills.iter().filter(|k| !k.loss).count();
    let losses = data.kills.len() - kills;
    let loss_tile = w::stat_tile("Losses", &losses.to_string(), None);
    if losses > 0 {
        loss_tile.add_css_class("cx-warn-tile");
    }
    let last = data.kills.iter().map(|k| k.time).max();
    body.append(&w::tile_row(vec![
        w::stat_tile("Kills", &kills.to_string(), None),
        loss_tile,
        w::stat_tile("Last activity", &last.map(fmt::date).unwrap_or_else(|| "—".into()), last.map(fmt::relative).as_deref()),
    ]));

    let view = ctx.recall("kills.view").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(2);
    let holder = w::vbox(0);
    let bar = w::hbox(12);
    let title = w::line(&format!("Latest {KILLMAIL_LIMIT}"), &["title-medium"]);
    title.set_hexpand(true);
    bar.append(&title);
    {
        let (ctx, holder) = (ctx.clone(), holder.clone());
        bar.append(&w::segmented(&VIEWS, view, move |i| {
            ctx.remember("kills.view", i.to_string());
            render(&ctx, &holder, id);
        }));
    }
    body.append(&bar);
    body.append(&holder);
    render(ctx, &holder, id);
    Page::new("Kill Log", body)
}

fn render(ctx: &Ctx, holder: &gtk::Box, id: i64) {
    clear(holder);
    let Some(c) = ctx.core().character(id) else { return };
    let view = ctx.recall("kills.view").and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
    let list: Vec<Killmail> = c
        .killmails
        .iter()
        .flat_map(|d| d.kills.iter())
        .filter(|k| match view {
            1 => !k.loss,
            2 => k.loss,
            _ => true,
        })
        .cloned()
        .collect();
    if list.is_empty() {
        holder.append(&w::empty_state("gps_fixed", "No killmails", "Kills and losses from the last while show here."));
        return;
    }
    let ctx2 = ctx.clone();
    let table = w::Table::new(list)
        .widget_sorted(
            "Ship",
            |k| {
                let row = w::hbox(8);
                row.append(&type_cell(&w::eve_image(&config::type_icon(k.victim.ship_type_id, 64), 32).upcast(), &k.victim.ship, None));
                row.append(&if k.loss { w::status("Loss", Status::Bad) } else { w::status("Kill", Status::Good) });
                row.upcast()
            },
            |a, b| a.victim.ship.cmp(&b.victim.ship),
        )
        .expand()
        .widget_sorted(
            "Victim",
            |k| {
                let org = [k.victim.corporation.clone(), k.victim.alliance.clone()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
                two_lines(&k.victim.name, &org)
            },
            |a, b| a.victim.name.to_lowercase().cmp(&b.victim.name.to_lowercase()),
        )
        .width(220)
        .widget_sorted(
            "Final blow",
            |k| {
                let ship = k.final_blow.ship.as_ref().map(|s| format!("{s} · ")).unwrap_or_default();
                let n = k.attackers;
                two_lines(&k.final_blow.name, &format!("{ship}{n} {}", if n == 1 { "attacker" } else { "attackers" }))
            },
            |a, b| a.final_blow.name.to_lowercase().cmp(&b.final_blow.name.to_lowercase()),
        )
        .width(220)
        .widget_sorted(
            "System",
            |k| {
                let row = w::hbox(8);
                if let Some(s) = k.security {
                    row.append(&sec_label(s));
                }
                row.append(&w::line(&k.system, &["body-medium"]));
                row.upcast()
            },
            |a, b| a.system.cmp(&b.system),
        )
        .width(150)
        .widget_sorted(
            "When",
            |k| {
                let l = w::line(&k.time.with_timezone(&chrono::Local).format("%b %e, %H:%M").to_string(), &["body-medium", "dim", "numeric"]);
                l.set_tooltip_text(Some(&format!("{} EVE time", fmt::eve_time(k.time))));
                l.upcast()
            },
            |a, b| a.time.cmp(&b.time),
        )
        .width(150)
        .widget("", move |k| {
            let b = w::button("zKill", Btn::Text);
            b.set_tooltip_text(Some("Open on zKillboard"));
            let (ctx, url) = (ctx2.clone(), extras::zkillboard_url(k.killmail_id));
            b.connect_clicked(move |_| w::open_url(&ctx, &url));
            b.upcast()
        })
        .width(80)
        .sort_by(4, true)
        .on_activate({
            let ctx = ctx.clone();
            move |k| w::open_url(&ctx, &extras::zkillboard_url(k.killmail_id))
        })
        .natural_height()
        .build();
    holder.append(&table);
}

fn two_lines(a: &str, b: &str) -> gtk::Widget {
    let v = w::vbox(2);
    v.append(&w::line(a, &["body-medium"]));
    if !b.is_empty() {
        v.append(&w::line(b, &["body-small", "dim"]));
    }
    v.upcast()
}
