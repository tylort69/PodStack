//! Industry Jobs: every character's unfinished jobs, with headline counts, a status filter and live countdowns.
//! The character tab (`pages/character/industry.rs`) shows the same view for one character.
//!
//! Also home to small cell builders the asset and PI pages share (type cells, character cells, debouncing).

use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::widgets::{self as w, Status, Table};
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::domains::industry::Job;
use podstack_core::features::industry::{self as ind, JobState};
use podstack_core::{config, fmt, Character};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Seconds between countdown updates.
const TICK: u32 = 10;

const FILTERS: [&str; 4] = ["All", "In progress", "Ready", "Paused"];

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.set_vexpand(true);
    let chars = ctx.core().characters();
    let no_scope = chars.iter().filter(|c| scope_failed(c, "industry_jobs")).count();
    let not_loaded = chars.iter().filter(|c| c.industry.is_none()).count() - no_scope;
    if no_scope > 0 {
        body.append(&w::banner(
            "lock",
            &format!("{no_scope} character(s) need to log in again with \"Read Industry Jobs\" to show their jobs."),
            w::BannerKind::Notice,
        ));
    }
    if not_loaded > 0 {
        body.append(&w::banner("hourglass_empty", &format!("{not_loaded} character(s) haven't loaded industry jobs yet."), w::BannerKind::Info));
    }
    body.append(&jobs_view(ctx, &chars, true, "industry"));
    Page::new("Industry Jobs", body).no_scroll()
}

/// Jobs ready to deliver, across all characters.
pub fn nav_badge(core: &podstack_core::Core) -> Option<u32> {
    Some(ind::count_ready_jobs(&core.characters(), Utc::now()) as u32)
}

/// Whether a data kind failed for a missing scope.
pub(crate) fn scope_failed(c: &Character, key: &str) -> bool {
    c.refreshes.get(key).and_then(|r| r.error.as_deref()) == Some("scope")
}

#[derive(Clone)]
struct Row {
    character_id: i64,
    character_name: String,
    job: Job,
}

/// Stat tiles, the status filter and the jobs table for `characters`. `key` names the remembered filter.
pub(crate) fn jobs_view(ctx: &Ctx, characters: &[Arc<Character>], show_character: bool, key: &str) -> gtk::Box {
    let now = Utc::now();
    let open = ind::open_jobs(characters, now);
    let counts = ind::job_counts(&open, now);
    let view = w::vbox(16);
    view.set_vexpand(true);

    let next = ind::next_completion(&open, now);
    let ready_tile = w::stat_tile("Ready to deliver", &fmt::int(counts.ready as i64), Some(if counts.ready > 0 { "Deliver them in-game" } else { "Nothing waiting" }));
    if counts.ready > 0 {
        ready_tile.add_css_class("accent");
    }
    let next_tile = match next {
        Some((c, j)) => {
            let sub = if show_character { format!("{} · {}", c.name(), j.blueprint_label()) } else { j.blueprint_label() };
            let tile = w::stat_tile("Next completion", &fmt::time_until(j.end_date), Some(&sub));
            // stat_tile's second child is its value label
            if let Some(value) = tile.first_child().and_then(|c| c.next_sibling()).and_then(|c| c.downcast::<gtk::Label>().ok()) {
                let end = j.end_date;
                let weak = value.downgrade();
                w::ticker(&value, 1, move || {
                    if let Some(l) = weak.upgrade() {
                        l.set_text(&if end > Utc::now() { fmt::time_until(end) } else { "Ready".into() });
                    }
                });
            }
            tile
        }
        None => w::stat_tile("Next completion", "—", Some("No jobs running")),
    };
    view.append(&w::tile_row(vec![
        w::stat_tile("Open jobs", &fmt::int(counts.open as i64), Some(&format!("{} paused", counts.paused))),
        w::stat_tile("In progress", &fmt::int(counts.active as i64), None),
        ready_tile,
        next_tile,
    ]));

    let rows: Vec<Row> = open.iter().map(|(c, j)| Row { character_id: c.id, character_name: c.name(), job: (*j).clone() }).collect();
    let mem_key = format!("{key}.filter");
    let selected: usize = ctx.recall(&mem_key).and_then(|v| v.parse().ok()).unwrap_or(0);

    let table_box = w::vbox(0);
    table_box.set_vexpand(true);
    let rows = Rc::new(rows);
    let fill = {
        let ctx = ctx.clone();
        let table_box = table_box.clone();
        let rows = rows.clone();
        move |filter: usize| {
            while let Some(child) = table_box.first_child() {
                table_box.remove(&child);
            }
            let now = Utc::now();
            let list: Vec<Row> = rows
                .iter()
                .filter(|r| match filter {
                    1 => ind::job_state(&r.job, now) == JobState::Active,
                    2 => ind::job_state(&r.job, now) == JobState::Ready,
                    3 => ind::job_state(&r.job, now) == JobState::Paused,
                    _ => true,
                })
                .cloned()
                .collect();
            table_box.append(&jobs_table(&ctx, list, show_character, rows.is_empty()));
        }
    };
    fill(selected);

    let bar = w::hbox(12);
    let ctx2 = ctx.clone();
    let fill = Rc::new(fill);
    let f2 = fill.clone();
    bar.append(&w::segmented(&FILTERS, selected, move |i| {
        ctx2.remember(&mem_key, i.to_string());
        f2(i);
    }));
    view.append(&bar);
    view.append(&table_box);
    view
}

fn jobs_table(ctx: &Ctx, rows: Vec<Row>, show_character: bool, none_at_all: bool) -> gtk::Widget {
    if rows.is_empty() {
        return if none_at_all {
            w::empty_state("precision_manufacturing", "No industry jobs running", "Jobs started in-game show up here, with a countdown to when they can be delivered.").upcast()
        } else {
            w::empty_state("filter_alt_off", "No jobs match", "No jobs have this status right now.").upcast()
        };
    }
    let mut t = Table::new(rows);
    if show_character {
        t = t.widget_sorted("Character", |r| char_cell(r.character_id, &r.character_name), |a, b| a.character_name.cmp(&b.character_name));
    }
    let ctx2 = ctx.clone();
    t.widget_sorted(
        "Activity",
        |r| {
            let b = w::hbox(8);
            b.append(&w::icon_sized(ind::activity_icon(r.job.activity_id), 18));
            b.append(&w::line(&ind::activity_name(r.job.activity_id), &[]));
            b.upcast()
        },
        |a, b| ind::activity_name(a.job.activity_id).cmp(&ind::activity_name(b.job.activity_id)),
    )
    .widget_sorted(
        "Blueprint",
        |r| {
            let sub = r.job.distinct_product().map(|p| format!("→ {p}"));
            let name = r.job.blueprint_label();
            type_cell(&icon_url(r.job.blueprint_type_id, &name, false), &name, sub.as_deref(), None)
        },
        |a, b| a.job.blueprint_label().cmp(&b.job.blueprint_label()),
    )
    .expand()
    .num("Runs", |r| fmt::int(r.job.runs), |r| r.job.runs as f64)
    .widget_sorted(
        "Location",
        |r| place_cell(r.job.location_name(), r.job.location.as_ref().and_then(place_sub)),
        |a, b| a.job.location_name().cmp(&b.job.location_name()),
    )
    .expand()
    .widget("Progress", |r| progress_cell(r.job.clone()))
    .width(200)
    .widget_sorted("Ends", |r| ends_cell(r.job.clone()), |a, b| a.job.end_date.cmp(&b.job.end_date))
    .sort_by(if show_character { 6 } else { 5 }, false)
    .on_activate(move |r| {
        if show_character {
            ctx2.navigate(Route::Character(r.character_id, CharTab::Industry));
        }
    })
    .build()
}

fn progress_cell(job: Job) -> gtk::Widget {
    let b = w::hbox(8);
    let bar = w::progress(0.0);
    bar.set_hexpand(true);
    let pct = w::line("", &["body-small", "dim", "numeric"]);
    pct.set_width_chars(5);
    pct.set_xalign(1.0);
    b.append(&bar);
    b.append(&pct);
    let update = {
        let bar = bar.downgrade();
        let pct = pct.downgrade();
        move || {
            let (Some(bar), Some(pct)) = (bar.upgrade(), pct.upgrade()) else { return };
            let now = Utc::now();
            let p = ind::job_progress(&job, now);
            bar.set_fraction(p);
            pct.set_text(&format!("{:.0}%", p * 100.0));
            for c in ["success", "warning"] {
                bar.remove_css_class(c);
            }
            match ind::job_state(&job, now) {
                JobState::Ready => bar.add_css_class("success"),
                JobState::Paused => bar.add_css_class("warning"),
                _ => {}
            }
        }
    };
    update();
    w::ticker(&b, TICK, update);
    b.upcast()
}

fn ends_cell(job: Job) -> gtk::Widget {
    let b = w::hbox(0);
    b.set_halign(gtk::Align::End);
    let update = {
        let weak = b.downgrade();
        let last: RefCell<Option<JobState>> = RefCell::new(None);
        let label = RefCell::new(None::<gtk::Label>);
        move || {
            let Some(b) = weak.upgrade() else { return };
            let now = Utc::now();
            let state = ind::job_state(&job, now);
            if *last.borrow() != Some(state) {
                while let Some(c) = b.first_child() {
                    b.remove(&c);
                }
                let child = match state {
                    JobState::Ready => w::status("Ready", Status::Good),
                    JobState::Paused => w::status("Paused", Status::Warn),
                    JobState::Closed => w::status("Closed", Status::Neutral),
                    JobState::Active => w::line("", &["numeric"]),
                };
                b.append(&child);
                b.set_tooltip_text(Some(&format!("Ends {}", fmt::datetime(job.end_date))));
                *label.borrow_mut() = (state == JobState::Active).then_some(child);
                *last.borrow_mut() = Some(state);
            }
            if let Some(l) = label.borrow().as_ref() {
                l.set_text(&fmt::time_until(job.end_date));
            }
        }
    };
    update();
    w::ticker(&b, TICK, update);
    b.upcast()
}

// ---------- cells shared with the asset and PI pages ----------

/// Icon address for a type. Blueprints and reaction formulas have no plain icon on the image server, only their
/// original / copy artwork.
pub(crate) fn icon_url(type_id: i64, name: &str, copy: bool) -> String {
    if name.ends_with("Blueprint") || name.ends_with("Formula") {
        format!("{}/types/{type_id}/{}?size=64", config::IMAGE_SERVER, if copy { "bpc" } else { "bp" })
    } else {
        config::type_icon(type_id, 64)
    }
}

/// Type icon (from [`icon_url`]), name (with an optional pill after it) and an optional dim second line.
pub(crate) fn type_cell(icon: &str, title: &str, sub: Option<&str>, pill: Option<gtk::Label>) -> gtk::Widget {
    let b = w::hbox(10);
    let icon = w::eve_image(icon, 32);
    icon.add_css_class("rounded");
    b.append(&icon);
    let texts = w::vbox(0);
    texts.set_valign(gtk::Align::Center);
    let head = w::hbox(6);
    head.append(&w::line(title, &[]));
    if let Some(p) = pill {
        head.append(&p);
    }
    texts.append(&head);
    if let Some(s) = sub {
        texts.append(&w::line(s, &["body-small", "dim"]));
    }
    b.append(&texts);
    b.set_tooltip_text(Some(title));
    b.upcast()
}

/// Portrait and name.
pub(crate) fn char_cell(id: i64, name: &str) -> gtk::Widget {
    let b = w::hbox(8);
    b.append(&w::avatar(&config::character_portrait(id, 64), 24));
    b.append(&w::line(name, &[]));
    b.upcast()
}

/// A location name with its system / region on a second line.
pub(crate) fn place_cell(name: String, sub: Option<String>) -> gtk::Widget {
    let b = w::vbox(0);
    b.set_valign(gtk::Align::Center);
    b.append(&w::line(&name, &[]));
    if let Some(s) = sub {
        b.append(&w::line(&s, &["body-small", "dim"]));
    }
    b.set_tooltip_text(Some(&name));
    b.upcast()
}

/// "Jita 0.9 · The Forge" for a place (None for a bare system, where it would repeat the name).
pub(crate) fn place_sub(p: &podstack_core::universe::Place) -> Option<String> {
    let mut parts = vec![];
    if p.kind != "system" {
        if let Some(s) = &p.system_name {
            parts.push(match p.security {
                Some(sec) => format!("{s} {}", fmt::security(sec)),
                None => s.clone(),
            });
        }
    } else if let Some(sec) = p.security {
        parts.push(format!("Security {}", fmt::security(sec)));
    }
    if let Some(r) = &p.region_name {
        parts.push(r.clone());
    }
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Keeps keyboard focus in `entry` across live rebuilds of its page (the shell rebuilds pages when data changes).
pub(crate) fn keep_focus(ctx: &Ctx, entry: &gtk::SearchEntry, key: &str) {
    let key = format!("{key}.focus");
    if ctx.recall(&key).as_deref() == Some("1") {
        let e = entry.clone();
        gtk::glib::idle_add_local_once(move || {
            e.grab_focus();
            e.set_position(-1);
        });
    }
    let focus = gtk::EventControllerFocus::new();
    let (c1, k1) = (ctx.clone(), key.clone());
    focus.connect_enter(move |_| c1.remember(&k1, "1"));
    let c2 = ctx.clone();
    focus.connect_leave(move |_| c2.remember(&key, "0"));
    entry.add_controller(focus);
}

/// Calls `f` with the search text; GTK already waits for a pause in typing before `search-changed`.
pub(crate) fn on_search(entry: &gtk::SearchEntry, f: impl Fn(String) + 'static) {
    entry.connect_search_changed(move |e| f(e.text().to_string()));
}
