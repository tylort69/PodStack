//! Character tab: research agents and the research points the character has with each (for datacores).

use super::summary::parts::{self, card, type_cell};
use crate::ctx::{Ctx, Page};
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::domains::social::ResearchAgent;
use podstack_core::features::character_extras as extras;
use podstack_core::{config, fmt};

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Research", body) };
    let Some(data) = &c.research else {
        return Page::new(
            "Research",
            parts::pending_page(&c, "research_agents", "esi-characters.read_agents_research.v1", "research agents", "science"),
        );
    };
    if data.agents.is_empty() {
        body.append(&w::empty_state("science", "No research agents", "Start research with an R&D agent in game."));
        return Page::new("Research", body);
    }
    let now = chrono::Utc::now();
    let mut agents: Vec<ResearchAgent> = data.agents.clone();
    agents.sort_by(|a, b| extras::research_points(b, now).total_cmp(&extras::research_points(a, now)));
    let per_day: f64 = agents.iter().map(|a| a.points_per_day).sum();
    let total: f64 = agents.iter().map(|a| extras::research_points(a, now)).sum();
    body.append(&w::tile_row(vec![
        w::stat_tile("Agents", &agents.len().to_string(), None),
        w::stat_tile("RP per day", &fmt::decimal(per_day, 1), None),
        w::stat_tile("Research points", &fmt::decimal(total, 0), Some("All agents")),
    ]));

    let table = w::Table::new(agents)
        .widget_sorted(
            "Agent",
            |a| type_cell(&w::avatar(&config::character_portrait(a.agent_id, 64), 28), &a.agent_name, None),
            |a, b| a.agent_name.cmp(&b.agent_name),
        )
        .expand()
        .text("Field", |a| a.skill_name.clone())
        .width(220)
        .num("RP / day", |a| fmt::decimal(a.points_per_day, 2), |a| a.points_per_day)
        .width(110)
        .num(
            "Research points",
            |a| fmt::decimal(extras::research_points(a, chrono::Utc::now()), 0),
            |a| extras::research_points(a, chrono::Utc::now()),
        )
        .width(150)
        .num("Since", |a| fmt::date(a.started_at), |a| a.started_at.timestamp() as f64)
        .width(120)
        .sort_by(3, true)
        .natural_height()
        .build();
    body.append(&card("Research Agents", "science", Some(&format!("{} RP/day in total", fmt::decimal(per_day, 1))), &[], &table));
    Page::new("Research", body)
}
