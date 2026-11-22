//! Character tab: upcoming events from the character's in-game calendar, soonest first.

use super::summary::parts::{self, card};
use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Status};
use gtk::prelude::*;
use podstack_core::domains::social::CalendarEvent;
use podstack_core::features::character_extras as extras;
use podstack_core::fmt;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Calendar", body) };
    let Some(data) = &c.calendar else {
        return Page::new("Calendar", parts::pending_page(&c, "calendar", "esi-calendar.read_calendar_events.v1", "calendar events", "event"));
    };
    let now = chrono::Utc::now();
    let events: Vec<CalendarEvent> = data.events.iter().filter(|e| extras::is_upcoming(e.event_date, now)).cloned().collect();
    if events.is_empty() {
        body.append(&w::empty_state("event_available", "Nothing on the calendar", "Upcoming events from the in-game calendar show here."));
        return Page::new("Calendar", body);
    }
    let next = &events[0];
    let accepted = events.iter().filter(|e| e.event_response == "accepted").count();
    let important = events.iter().filter(|e| e.importance == 1).count();
    body.append(&w::tile_row(vec![
        w::stat_tile("Next event", &if next.event_date > now { fmt::time_until(next.event_date) } else { "Now".into() }, Some(&next.title)),
        w::stat_tile("Upcoming", &events.len().to_string(), None),
        w::stat_tile("Accepted", &accepted.to_string(), None),
        w::stat_tile("Important", &important.to_string(), None),
    ]));

    let table = w::Table::new(events)
        .widget_sorted(
            "When",
            |e| {
                let v = w::vbox(2);
                v.append(&w::line(&e.event_date.with_timezone(&chrono::Local).format("%a %b %e, %H:%M").to_string(), &["body-medium"]));
                v.append(&w::line(&format!("{} EVE", e.event_date.format("%H:%M")), &["body-small", "dim"]));
                v.upcast()
            },
            |a, b| a.event_date.cmp(&b.event_date),
        )
        .width(180)
        .num(
            "In",
            |e| if e.event_date > chrono::Utc::now() { fmt::time_until(e.event_date) } else { "Now".into() },
            |e| e.event_date.timestamp() as f64,
        )
        .width(130)
        .widget_sorted(
            "Event",
            |e| {
                let row = w::hbox(8);
                if e.importance == 1 {
                    let i = w::icon_sized("priority_high", 18);
                    i.add_css_class("error-text");
                    i.set_tooltip_text(Some("Important"));
                    row.append(&i);
                }
                row.append(&w::line(&e.title, &["body-medium"]));
                row.upcast()
            },
            |a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()),
        )
        .expand()
        .widget_sorted("Reply", |e| response(&e.event_response).upcast(), |a, b| a.event_response.cmp(&b.event_response))
        .width(120)
        .natural_height()
        .build();
    body.append(&card("Upcoming Events", "event", None, &[], &table));
    Page::new("Calendar", body)
}

fn response(r: &str) -> gtk::Label {
    match r {
        "accepted" => w::status("Accepted", Status::Good),
        "tentative" => w::status("Tentative", Status::Warn),
        "declined" => w::status("Declined", Status::Bad),
        "not_responded" => w::status("No reply", Status::Neutral),
        other => w::status(other, Status::Neutral),
    }
}
