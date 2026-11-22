//! A character's pages: the header (portrait, status, headline numbers, refresh) and the tab bar, around the
//! selected tab's page.

pub mod assets;
pub mod calendar;
pub mod clones;
pub mod contacts;
pub mod contracts;
pub mod esi;
pub mod industry;
pub mod kills;
pub mod mail;
pub mod market;
pub mod medals;
pub mod notifications;
pub mod planets;
pub mod plans;
pub mod research;
pub mod skills;
pub mod standings;
pub mod summary;
pub mod wallet;

use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::widgets::{self as w, Status};
use gtk::prelude::*;
use podstack_core::{fmt, training::roman, Character, Event};

pub fn build(ctx: &Ctx, id: i64, tab: CharTab) -> Page {
    let Some(c) = ctx.core().character(id) else {
        return Page::new("Character", w::empty_state("person_off", "Character not found", "It may have been removed."));
    };
    let inner = match tab {
        CharTab::Summary => summary::build(ctx, id),
        CharTab::Skills => skills::build(ctx, id),
        CharTab::Plans => plans::build(ctx, id),
        CharTab::Clones => clones::build(ctx, id),
        CharTab::Mail => mail::build(ctx, id),
        CharTab::Wallet => wallet::build(ctx, id),
        CharTab::Market => market::build(ctx, id),
        CharTab::Contracts => contracts::build(ctx, id),
        CharTab::Assets => assets::build(ctx, id),
        CharTab::Industry => industry::build(ctx, id),
        CharTab::Research => research::build(ctx, id),
        CharTab::Planets => planets::build(ctx, id),
        CharTab::Notifications => notifications::build(ctx, id),
        CharTab::Calendar => calendar::build(ctx, id),
        CharTab::Standings => standings::build(ctx, id),
        CharTab::Contacts => contacts::build(ctx, id),
        CharTab::Kills => kills::build(ctx, id),
        CharTab::Medals => medals::build(ctx, id),
        CharTab::Esi => esi::build(ctx, id),
    };

    let body = w::vbox(16);
    body.set_margin_start(24);
    body.set_margin_end(24);
    body.set_margin_top(8);
    body.set_margin_bottom(24);
    body.append(&header(ctx, &c));
    body.append(&tab_bar(ctx, id, tab));
    if !inner.scroll {
        inner.content.set_vexpand(true);
    }
    body.append(&inner.content);

    Page { title: c.name(), content: body.upcast(), scroll: inner.scroll, live: inner.live, actions: inner.actions }
}

fn header(ctx: &Ctx, c: &Character) -> gtk::Widget {
    let core = ctx.core().clone();
    let id = c.id;
    let card = w::hbox(24);
    card.add_css_class("character-header");

    card.append(&w::avatar(&c.portrait_url(256), 96));

    let names = w::vbox(4);
    names.set_valign(gtk::Align::Center);
    names.set_hexpand(true);
    names.append(&w::line(&c.name(), &["headline-small"]));
    let mut org = String::new();
    if let Some(i) = &c.info {
        if let Some(corp) = &i.corporation {
            org.push_str(&corp.name);
            if !corp.ticker.is_empty() {
                org.push_str(&format!(" [{}]", corp.ticker));
            }
        }
        if let Some(a) = &i.alliance {
            org.push_str(&format!("  ·  {}", a.name));
        }
    }
    if !org.is_empty() {
        names.append(&w::line(&org, &["body-medium", "dim"]));
    }

    let chips = w::hbox(8);
    chips.set_margin_top(6);
    match c.is_omega() {
        Some(true) => chips.append(&w::status("Omega", Status::Good)),
        Some(false) => chips.append(&w::status("Alpha", Status::Warn)),
        None => chips.append(&w::status("Clone state unknown", Status::Neutral)),
    }
    if let Some(a) = core.auth.get(id) {
        if a.needs_reauth() {
            chips.append(&w::status("Login expired — add the character again", Status::Bad));
        } else if a.uses_legacy_client() {
            chips.append(&w::status("Old login: re-authorize for all features", Status::Warn));
        } else if !a.missing_scopes().is_empty() {
            chips.append(&w::status(&format!("{} scopes missing", a.missing_scopes().len()), Status::Warn));
        }
    }
    if let Some(loc) = &c.location {
        let sec = loc.system.security.map(|s| format!(" {}", fmt::security(s))).unwrap_or_default();
        let where_ = match &loc.docked {
            Some(d) => d.name.clone(),
            None => format!("{}{sec}", loc.system.name),
        };
        chips.append(&w::status(&where_, Status::Info));
    }
    if let Some(s) = &c.ship {
        chips.append(&w::status(&format!("{} · {}", s.type_name, s.ship_name), Status::Neutral));
    }
    names.append(&chips);
    card.append(&names);

    // headline numbers
    let stats = w::hbox(32);
    stats.set_valign(gtk::Align::Center);
    let sp_value = w::line(&fmt::int(c.total_sp()), &["title-large", "numeric"]);
    stats.append(&stat("Skill points", &sp_value, c.unallocated_sp().gt(&0).then(|| format!("+{} unallocated", fmt::int(c.unallocated_sp())))));
    let wallet = w::line(&c.wallet.map(fmt::isk_short).unwrap_or_else(|| "—".into()), &["title-large", "numeric"]);
    stats.append(&stat("Wallet", &wallet, c.wallet.map(|v| format!("{} ISK", fmt::decimal(v, 0)))));
    let (training, sub) = match c.current_skill() {
        Some(q) => (
            format!("{} {}", q.skill_name, roman(q.finished_level)),
            c.queue_time_left().map(|t| format!("Queue ends in {}", fmt::duration(t, 2))),
        ),
        None => ("Not training".to_string(), None),
    };
    let tl = w::line(&training, &["title-medium"]);
    tl.set_max_width_chars(28);
    stats.append(&stat("Training", &tl, sub));
    card.append(&stats);

    // keep SP counting up while training
    if c.is_training() {
        let core2 = core.clone();
        let sp = sp_value.clone();
        w::ticker(&sp_value, 1, move || {
            if let Some(c) = core2.character(id) {
                sp.set_text(&fmt::int(c.total_sp()));
            }
        });
    }

    // refresh button + progress
    let side = w::vbox(4);
    side.set_valign(gtk::Align::Center);
    let refresh = w::icon_button("refresh", "Refresh from ESI");
    refresh.add_css_class("tonal");
    let tip = match c.last_updated() {
        Some(t) => format!("Refresh from ESI\nLast updated {}", fmt::relative(t)),
        None => "Refresh from ESI".into(),
    };
    refresh.set_tooltip_text(Some(&tip));
    let bar = gtk::ProgressBar::new();
    bar.set_size_request(40, -1);
    bar.set_visible(false);
    if let Some((done, total)) = core.refresh_progress(id) {
        bar.set_visible(true);
        bar.set_fraction(done as f64 / total.max(1) as f64);
        refresh.set_sensitive(false);
    }
    {
        let ctx = ctx.clone();
        refresh.connect_clicked(move |_| match ctx.core().refresh_now(id) {
            Ok(()) => ctx.toast("Refreshing from ESI…"),
            Err(wait) => ctx.toast(&format!("You can refresh again in {}", fmt::duration(wait, 2))),
        });
    }
    {
        let bar = bar.clone();
        let refresh = refresh.clone();
        ctx.on_event(&card, move |e| {
            if let Event::RefreshProgress { id: rid, done, total } = e {
                if *rid == id {
                    let running = done < total;
                    bar.set_visible(running);
                    bar.set_fraction(*done as f64 / (*total).max(1) as f64);
                    refresh.set_sensitive(!running);
                }
            }
        });
    }
    side.append(&refresh);
    side.append(&bar);
    card.append(&side);
    card.upcast()
}

fn stat(label: &str, value: &gtk::Label, sub: Option<String>) -> gtk::Box {
    let b = w::vbox(2);
    b.append(&w::line(label, &["label-medium", "dim"]));
    b.append(value);
    if let Some(s) = sub {
        b.append(&w::line(&s, &["body-small", "dim"]));
    }
    b
}

fn tab_bar(ctx: &Ctx, id: i64, current: CharTab) -> gtk::Widget {
    let tabs = w::hbox(0);
    tabs.add_css_class("m3-tabs");
    let character = ctx.core().character(id);
    let unread = |tab: CharTab| -> usize {
        let Some(c) = character.as_deref() else { return 0 };
        match tab {
            CharTab::Mail => podstack_core::features::comms::unread_mail_count(c),
            CharTab::Notifications => podstack_core::features::character_extras::unread_notification_count(c),
            _ => 0,
        }
    };
    for tab in CharTab::ALL {
        let b = gtk::ToggleButton::new();
        b.add_css_class("m3-tab");
        let row = w::hbox(6);
        row.append(&w::icon_sized(tab.icon(), 18));
        row.append(&gtk::Label::new(Some(tab.label())));
        let n = unread(tab);
        if n > 0 {
            row.append(&w::badge(n.min(u32::MAX as usize) as u32));
        }
        b.set_child(Some(&row));
        b.set_active(tab == current);
        let ctx = ctx.clone();
        b.connect_clicked(move |b| {
            if tab != current {
                ctx.navigate(Route::Character(id, tab));
            } else {
                b.set_active(true);
            }
        });
        tabs.append(&b);
    }
    let sw = gtk::ScrolledWindow::new();
    sw.set_vscrollbar_policy(gtk::PolicyType::Never);
    sw.set_hscrollbar_policy(gtk::PolicyType::Automatic);
    sw.set_child(Some(&tabs));
    sw.upcast()
}
