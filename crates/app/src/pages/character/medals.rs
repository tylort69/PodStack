//! Character tab: medals awarded to the character by corporations, newest first.

use super::summary::parts::{self, card};
use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Status};
use gtk::prelude::*;
use podstack_core::{config, fmt};

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Medals", body) };
    let Some(data) = &c.medals else {
        return Page::new("Medals", parts::pending_page(&c, "medals", "esi-characters.read_medals.v1", "medals", "military_tech"));
    };
    if data.medals.is_empty() {
        body.append(&w::empty_state("military_tech", "No medals yet", "Medals corporations award this character show here."));
        return Page::new("Medals", body);
    }
    let list = w::vbox(0);
    for (i, m) in data.medals.iter().enumerate() {
        if i > 0 {
            list.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        let row = w::hbox(16);
        row.add_css_class("cx-medal");
        let logo = w::avatar(&config::corporation_logo(m.corporation_id, 128), 48);
        logo.set_valign(gtk::Align::Start);
        row.append(&logo);
        let texts = w::vbox(4);
        texts.set_hexpand(true);
        let title = w::hbox(8);
        title.append(&w::line(&m.title, &["title-medium"]));
        if m.status == "private" {
            title.append(&w::status("Private", Status::Neutral));
        }
        texts.append(&title);
        let mut by = m.corporation.clone().unwrap_or_else(|| format!("Corporation #{}", m.corporation_id));
        if let Some(i) = &m.issuer {
            by.push_str(&format!(" · awarded by {i}"));
        }
        by.push_str(&format!(" · {}", fmt::date(m.date)));
        texts.append(&w::text(&by, &["body-medium", "dim"]));
        if !m.reason.is_empty() {
            let r = w::text(&m.reason, &["body-medium"]);
            r.set_margin_top(4);
            texts.append(&r);
        }
        if !m.description.is_empty() {
            texts.append(&w::text(&m.description, &["body-small", "dim"]));
        }
        row.append(&texts);
        list.append(&row);
    }
    body.append(&card("Medals", "military_tech", Some(&data.medals.len().to_string()), &[], &list));
    Page::new("Medals", body)
}
