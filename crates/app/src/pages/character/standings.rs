//! Character tab: standings. Factional warfare enlistment, then NPC standings toward the character by faction,
//! corporation and agent, with the standing EVE actually uses after Connections and Diplomacy.

use super::summary::parts::{self, card, collapsible, dim_text, standing_label, type_cell};
use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::domains::social::{FwData, Standing};
use podstack_core::features::character_extras as extras;
use podstack_core::{config, fmt};

const GROUPS: [(&str, &str, &str, &str); 3] = [
    ("faction", "Factions", "Faction", "flag"),
    ("npc_corp", "Corporations", "Corporation", "business"),
    ("agent", "Agents", "Agent", "support_agent"),
];

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Standings", body) };
    if let Some(fw) = c.fw.as_ref().filter(|f| f.enlisted()) {
        body.append(&factional_warfare(fw));
    }
    let Some(data) = &c.standings else {
        body.append(&parts::pending_page(&c, "standings", "esi-characters.read_standings.v1", "standings", "handshake"));
        return Page::new("Standings", body);
    };
    let (connections, diplomacy) = extras::standing_skills(&c);
    body.append(&w::banner(
        "info",
        &format!(
            "Effective standings include Connections {connections} (raises positive standings) and Diplomacy {diplomacy} \
             (raises negative ones), 4% of the distance to +10 per level."
        ),
        w::BannerKind::Info,
    ));

    for (kind, title, singular, icon) in GROUPS {
        let mut rows: Vec<Standing> = data.standings.iter().filter(|s| s.from_type == kind).cloned().collect();
        rows.sort_by(|a, b| b.standing.total_cmp(&a.standing));
        let n = rows.len();
        let content: gtk::Widget = if rows.is_empty() {
            dim_text("None.").upcast()
        } else {
            w::Table::new(rows)
                .widget_sorted(
                    singular,
                    |s| {
                        let url = if s.from_type == "agent" { config::character_portrait(s.from_id, 64) } else { config::corporation_logo(s.from_id, 64) };
                        type_cell(&w::avatar(&url, 28), &s.name, None)
                    },
                    |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                )
                .expand()
                .widget_sorted("Standing", |s| standing_label(s.standing).upcast(), |a, b| a.standing.total_cmp(&b.standing))
                .width(120)
                .widget_sorted(
                    "Effective",
                    move |s| standing_label(extras::effective_standing(s.standing, connections, diplomacy)).upcast(),
                    |a, b| a.standing.total_cmp(&b.standing),
                )
                .width(120)
                .natural_height()
                .build()
        };
        let head = w::hbox(12);
        let i = w::icon_sized(icon, 20);
        i.add_css_class("primary-text");
        head.append(&i);
        let t = w::line(title, &["title-medium"]);
        t.set_hexpand(true);
        head.append(&t);
        head.append(&w::badge(n as u32));
        let section = w::vbox(0);
        section.add_css_class("card");
        section.append(&collapsible(ctx, &format!("standings.{kind}"), true, 0, &head.upcast(), &content));
        body.append(&section);
    }
    Page::new("Standings", body)
}

fn factional_warfare(fw: &FwData) -> gtk::Box {
    let mut sub = fw.faction.clone().unwrap_or_else(|| "Enlisted".into());
    if let Some(d) = fw.enlisted_on {
        sub.push_str(&format!(" · since {}", fmt::date(d)));
    }
    let tiles = w::tile_row(vec![
        w::stat_tile(
            "Rank",
            &fw.current_rank.map(|r| r.to_string()).unwrap_or_else(|| "—".into()),
            fw.highest_rank.map(|h| format!("Highest {h}")).as_deref(),
        ),
        w::stat_tile(
            "Kills",
            &fmt::int(fw.kills.total),
            Some(&format!("{} last week · {} yesterday", fmt::int(fw.kills.last_week), fmt::int(fw.kills.yesterday))),
        ),
        w::stat_tile(
            "Victory points",
            &fmt::isk_short(fw.victory_points.total as f64),
            Some(&format!("{} last week · {} yesterday", fmt::int(fw.victory_points.last_week), fmt::int(fw.victory_points.yesterday))),
        ),
    ]);
    let head_logo = fw.faction_id.map(|f| w::avatar(&config::corporation_logo(f, 64), 40));
    let b = w::vbox(12);
    if let Some(l) = head_logo {
        let row = w::hbox(12);
        row.append(&l);
        row.append(&parts::fit(&sub, &["body-large"]));
        b.append(&row);
    }
    b.append(&tiles);
    card("Factional Warfare", "military_tech", None, &[], &b)
}
