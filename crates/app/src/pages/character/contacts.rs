//! Character tab: contacts. The character's personal address book (People & Places), best standing first, with a
//! search over names, types and labels.

use super::summary::parts::{self, clear, standing_label, type_cell};
use crate::ctx::{Ctx, Page};
use crate::widgets::{self as w, Status};
use gtk::prelude::*;
use podstack_core::config;
use podstack_core::domains::social::Contact;
use podstack_core::features::character_extras as extras;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Contacts", body) };
    let Some(data) = &c.contacts else {
        return Page::new("Contacts", parts::pending_page(&c, "contacts", "esi-characters.read_contacts.v1", "contacts", "contacts"));
    };
    if data.contacts.is_empty() {
        body.append(&w::empty_state("contacts", "No contacts", "People and places you add in game show here."));
        return Page::new("Contacts", body);
    }

    let count = |f: &dyn Fn(&Contact) -> bool| data.contacts.iter().filter(|c| f(c)).count() as i64;
    body.append(&w::tile_row(vec![
        w::stat_tile("Contacts", &count(&|_| true).to_string(), None),
        w::stat_tile("Positive", &count(&|c| c.standing > 0.0).to_string(), None),
        w::stat_tile("Negative", &count(&|c| c.standing < 0.0).to_string(), None),
        w::stat_tile("Watched", &count(&|c| c.is_watched).to_string(), None),
        w::stat_tile("Blocked", &count(&|c| c.is_blocked).to_string(), None),
    ]));

    let search = w::search_entry("Search names, types or labels…");
    search.set_text(&ctx.recall("contacts.search").unwrap_or_default());
    body.append(&search);
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    body.append(&holder);
    render(ctx, &holder, id);
    {
        let (ctx, holder) = (ctx.clone(), holder.clone());
        search.connect_search_changed(move |s| {
            ctx.remember("contacts.search", s.text().to_string());
            render(&ctx, &holder, id);
        });
    }
    body.set_vexpand(true);
    Page::new("Contacts", body).no_scroll()
}

fn image(c: &Contact) -> String {
    match c.contact_type.as_str() {
        "character" => config::character_portrait(c.contact_id, 64),
        "alliance" => config::alliance_logo(c.contact_id, 64),
        _ => config::corporation_logo(c.contact_id, 64),
    }
}

fn capitalize(s: &str) -> String {
    let mut ch = s.chars();
    match ch.next() {
        Some(f) => f.to_uppercase().collect::<String>() + ch.as_str(),
        None => String::new(),
    }
}

fn render(ctx: &Ctx, holder: &gtk::Box, id: i64) {
    clear(holder);
    let Some(c) = ctx.core().character(id) else { return };
    let query = ctx.recall("contacts.search").unwrap_or_default();
    let mut list: Vec<Contact> =
        c.contacts.iter().flat_map(|d| d.contacts.iter()).filter(|x| extras::contact_matches(x, &query)).cloned().collect();
    list.sort_by(|a, b| b.standing.total_cmp(&a.standing).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    if list.is_empty() {
        holder.append(&w::empty_state("search_off", "Nothing matches", "Try another name, type or label."));
        return;
    }
    let table = w::Table::new(list)
        .widget_sorted(
            "Contact",
            |c| {
                let row = w::hbox(8);
                row.append(&type_cell(&w::avatar(&image(c), 28), &c.name, None));
                if c.is_watched {
                    let i = w::icon_sized("visibility", 18);
                    i.add_css_class("dim");
                    i.set_tooltip_text(Some("On the watch list"));
                    row.append(&i);
                }
                if c.is_blocked {
                    row.append(&w::status("Blocked", Status::Bad));
                }
                row.upcast()
            },
            |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        )
        .expand()
        .text("Type", |c| capitalize(&c.contact_type))
        .width(130)
        .text("Labels", |c| c.labels.join(", "))
        .width(200)
        .widget_sorted("Standing", |c| standing_label(c.standing).upcast(), |a, b| a.standing.total_cmp(&b.standing))
        .width(110)
        .build();
    holder.append(&table);
}
