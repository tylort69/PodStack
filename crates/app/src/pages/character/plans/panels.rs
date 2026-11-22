//! Panels from the skill plan feature that other pages embed.

use crate::ctx::Ctx;
use crate::widgets as w;
use chrono::{Local, NaiveDateTime, TimeZone, Utc};
use gtk::prelude::*;
use podstack_core::features::plan_tools::accelerator;
use podstack_core::fmt;

const INPUT_FORMAT: &str = "%Y-%m-%d %H:%M";

/// Cerebral accelerator status for the Summary tab (None when there's nothing to show): its bonus, which one it is
/// and when it ends. ESI only reveals the bonus, so the player can say which accelerator it is and correct the
/// start time; both are kept until it runs out.
pub fn accelerator_panel(ctx: &Ctx, id: i64) -> Option<gtk::Widget> {
    accelerator::refresh(ctx.core(), id);
    let c = ctx.core().character(id)?;
    let now = Utc::now();
    let status = accelerator::status(&c, now)?;
    let candidates = accelerator::candidates(status.bonus);

    let grid = gtk::Grid::new();
    grid.set_column_spacing(24);
    grid.set_row_spacing(10);

    grid.attach(&w::line("Accelerator", &["body-medium", "dim"]), 0, 0, 1, 1);
    if candidates.is_empty() {
        grid.attach(&w::line(&format!("Unknown +{} accelerator", status.bonus), &["body-medium", "dim"]), 1, 0, 1, 1);
    } else {
        let labels: Vec<String> = candidates.iter().map(|a| format!("{} ({}d)", a.name, a.days)).collect();
        let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let dd = gtk::DropDown::from_strings(&refs);
        dd.set_halign(gtk::Align::Start);
        if let Some(i) = status.item.and_then(|it| candidates.iter().position(|a| a.type_id == it.type_id)) {
            dd.set_selected(i as u32);
        }
        let ids: Vec<i64> = candidates.iter().map(|a| a.type_id).collect();
        let core = ctx.core().clone();
        dd.connect_selected_notify(move |d| {
            if let Some(t) = ids.get(d.selected() as usize) {
                accelerator::set_choice(&core, id, Some(*t), None);
            }
        });
        grid.attach(&dd, 1, 0, 1, 1);
    }

    grid.attach(&w::line("Started", &["body-medium", "dim"]), 0, 1, 1, 1);
    let start_row = w::hbox(8);
    let entry = gtk::Entry::new();
    entry.set_text(&status.start.with_timezone(&Local).format(INPUT_FORMAT).to_string());
    entry.set_width_chars(16);
    entry.set_tooltip_text(Some("When you plugged it in (local time, YYYY-MM-DD HH:MM). Press Enter to save."));
    {
        let ctx2 = ctx.clone();
        entry.connect_activate(move |e| {
            let parsed = NaiveDateTime::parse_from_str(e.text().trim(), INPUT_FORMAT).ok().and_then(|n| Local.from_local_datetime(&n).single());
            match parsed {
                Some(t) if t.with_timezone(&Utc) <= Utc::now() => accelerator::set_choice(ctx2.core(), id, None, Some(t.with_timezone(&Utc))),
                Some(_) => ctx2.toast("The start can't be in the future."),
                None => ctx2.toast("Enter the start as YYYY-MM-DD HH:MM."),
            }
        });
    }
    start_row.append(&entry);
    if status.estimated {
        start_row.append(&w::line("estimated", &["body-small", "dim"]));
    }
    grid.attach(&start_row, 1, 1, 1, 1);

    grid.attach(&w::line("Ends", &["body-medium", "dim"]), 0, 2, 1, 1);
    let ends = match (status.end, status.remaining) {
        (Some(end), Some(left)) if left > chrono::Duration::zero() => {
            w::markup(&format!("<span>in {}</span> · {}", fmt::duration(left, 3), fmt::datetime(end)), &["body-medium", "success-text"])
        }
        (Some(_), Some(_)) => w::text("Should have ended; it clears on the next refresh", &["body-medium", "warning-text"]),
        _ => w::text("Unknown", &["body-medium", "dim"]),
    };
    grid.attach(&ends, 1, 2, 1, 1);

    let body = w::vbox(12);
    body.append(&grid);
    let mut note = "EVE only reveals the bonus, so check which accelerator it is and when you plugged it in.".to_string();
    if status.biology > 0 {
        note.push_str(&format!(" Biology {} makes it last {}% longer.", status.biology, status.biology as u32 * 20));
    }
    body.append(&w::text(&note, &["body-small", "dim"]));
    let title_icon = w::icon_sized("bolt", 20);
    title_icon.add_css_class("tertiary-text");
    let card = w::card_with("Cerebral Accelerator", Some(&format!("+{} to all attributes", status.bonus)), &[title_icon.upcast()], &body);
    Some(card.upcast())
}
