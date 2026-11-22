//! Character tab: EVE's in-game notifications (the Notifications window, not mail), newest first, with a search and
//! "Mark all read" (remembered by PodStack, as ESI can't mark them read).

use super::summary::parts::{self, card, clear, collapsible, dim_text, fit};
use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::character_extras as extras;
use podstack_core::fmt;

const ROW_LIMIT: usize = 300;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Notifications", body) };
    let Some(data) = &c.notifications else {
        return Page::new(
            "Notifications",
            parts::pending_page(&c, "notifications", "esi-characters.read_notifications.v1", "notifications", "notifications"),
        );
    };
    let unread = extras::unread_notification_count(&c);
    let mut sub = format!("{} · {unread} unread", fmt::int(data.notifications.len() as i64));
    if let Some(last) = c.refreshes.get("notifications").and_then(|r| r.last) {
        sub.push_str(&format!(" · Updated {}", fmt::relative(last)));
    }

    let mark = w::button_icon("done_all", "Mark all read", Btn::Tonal);
    mark.set_sensitive(unread > 0);
    {
        let core = ctx.core().clone();
        mark.connect_clicked(move |_| extras::mark_all_notifications_read(&core, id));
    }

    let content = w::vbox(12);
    let search = w::search_entry("Search type, sender or details…");
    search.set_text(&ctx.recall("notifications.search").unwrap_or_default());
    content.append(&search);
    let list = w::vbox(2);
    content.append(&list);
    render(ctx, &list, id);
    {
        let (ctx, list) = (ctx.clone(), list.clone());
        search.connect_search_changed(move |s| {
            ctx.remember("notifications.search", s.text().to_string());
            render(&ctx, &list, id);
        });
    }
    body.append(&card("Notifications", "notifications", Some(&sub), &[mark.upcast()], &content));
    Page::new("Notifications", body)
}

fn render(ctx: &Ctx, list: &gtk::Box, id: i64) {
    clear(list);
    let Some(c) = ctx.core().character(id) else { return };
    let query = ctx.recall("notifications.search").unwrap_or_default();
    let seen = extras::seen_notifications(&c);
    let all = c.notifications.as_ref().map(|d| d.notifications.as_slice()).unwrap_or(&[]);
    let matching: Vec<_> = all.iter().filter(|n| extras::notification_matches(n, &query)).collect();
    if matching.is_empty() {
        list.append(&dim_text(if query.trim().is_empty() { "No notifications." } else { "Nothing matches." }));
        return;
    }
    for n in matching.iter().take(ROW_LIMIT) {
        let unread = extras::is_notification_unread(n, &seen);
        let head = w::hbox(12);
        let dot = w::hbox(0);
        dot.set_valign(gtk::Align::Center);
        if unread {
            dot.add_css_class("cx-unread-dot");
            dot.set_tooltip_text(Some("Unread"));
        } else {
            dot.add_css_class("cx-dot-spacer");
        }
        head.append(&dot);
        let mut classes = vec!["body-large"];
        if unread {
            classes.push("cx-unread");
        }
        let title = w::line(&extras::notification_title(&n.kind), &classes);
        title.set_hexpand(true);
        head.append(&title);
        if let Some(s) = &n.sender_name {
            head.append(&fit(s, &["body-medium", "dim"]));
        }
        let when = fit(&fmt::relative(n.timestamp), &["body-medium", "dim", "numeric"]);
        when.set_tooltip_text(Some(&fmt::datetime(n.timestamp)));
        when.set_width_chars(12);
        when.set_xalign(1.0);
        head.append(&when);

        let lines = extras::notification_details(n.text.as_deref());
        let details = w::text(if lines.is_empty() { "No details." } else { "" }, &["cx-mono"]);
        if !lines.is_empty() {
            details.set_text(&lines.join("\n"));
        }
        details.set_selectable(true);
        list.append(&collapsible(ctx, &format!("notifications.open.{}", n.notification_id), false, 36, &head.upcast(), &details.upcast()));
    }
    if matching.len() > ROW_LIMIT {
        list.append(&dim_text(&format!("Showing the latest {ROW_LIMIT}. Search to narrow it down.")));
    }
}
