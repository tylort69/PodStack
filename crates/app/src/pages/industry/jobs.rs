//! Industry → Jobs: how many manufacturing, science and reaction slots each character has free, then every
//! character's unfinished jobs, with headline counts, status and activity filters and live countdowns.
//! The character tab (`pages/character/industry.rs`) shows the same view for one character.
//!
//! Also home to small cell builders the asset and PI pages share (type cells, character cells, debouncing).

use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::widgets::{self as w, Status, Table};
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::domains::industry::Job;
use podstack_core::features::industry::{self as ind, JobState, SlotKind};
use podstack_core::{config, fmt, Character};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// Seconds between countdown updates.
const TICK: u32 = 10;

const FILTERS: [&str; 4] = ["All", "In progress", "Ready", "Paused"];

/// The activity filter: everything, then each `SlotKind`.
const ACTIVITIES: [&str; 4] = ["All activities", "Manufacturing", "Science", "Reactions"];

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&super::tab_bar(ctx, super::Tab::Jobs));
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
    if !chars.is_empty() {
        body.append(&slots_section(ctx, &chars));
    }
    body.append(&jobs_view(ctx, &chars, true, "industry.jobs"));
    Page::new("Industry", body).no_scroll()
}

// ---------- slots ----------

const SLOTS_HIDDEN_KEY: &str = "industry.jobs.slots_hidden";

/// One character's slots (or, with `total`, everyone's).
struct SlotRow {
    character_id: i64,
    name: String,
    /// Slots per `SlotKind::index`; None until skills load.
    slots: Option<[i64; 3]>,
    /// None until jobs load.
    usage: Option<ind::SlotUsage>,
    total: bool,
}

impl SlotRow {
    fn idle(&self, kind: SlotKind) -> Option<i64> {
        let (s, u) = (self.slots?, self.usage?);
        Some((s[kind.index()] - u.used[kind.index()]).max(0))
    }
    /// None until loaded, or when there are no slots at all (nothing loaded into the totals row yet).
    fn idle_total(&self) -> Option<i64> {
        if self.slots.is_some_and(|s| s.iter().sum::<i64>() == 0) {
            return None;
        }
        SlotKind::ALL.iter().map(|k| self.idle(*k)).sum()
    }
}

/// Free slots per kind across all characters, then a table of every character's slots: the view that matters when
/// many characters each have a few jobs to restart.
fn slots_section(ctx: &Ctx, chars: &[Arc<Character>]) -> gtk::Box {
    let now = Utc::now();
    let mut rows: Vec<SlotRow> = chars
        .iter()
        .map(|c| SlotRow {
            character_id: c.id,
            name: c.name(),
            slots: ind::character_slots(c),
            usage: c.industry.as_ref().map(|d| ind::slot_usage(&d.jobs, now)),
            total: false,
        })
        .collect();
    // Totals over the characters whose skills and jobs are both known, so idle counts aren't guesses.
    let known: Vec<&SlotRow> = rows.iter().filter(|r| r.slots.is_some() && r.usage.is_some()).collect();
    let mut slots = [0i64; 3];
    let mut usage = ind::SlotUsage::default();
    for r in &known {
        let (s, u) = (r.slots.unwrap_or_default(), r.usage.unwrap_or_default());
        for i in 0..3 {
            slots[i] += s[i];
            usage.used[i] += u.used[i];
        }
        usage.ready += u.ready;
        usage.next_end = match (usage.next_end, u.next_end) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    let total = SlotRow { character_id: 0, name: format!("All characters ({})", known.len()), slots: Some(slots), usage: Some(usage), total: true };

    let section = w::vbox(12);
    let hidden = ctx.recall(SLOTS_HIDDEN_KEY).as_deref() == Some("1");
    let head = w::hbox(8);
    let title = w::line("Slots", &["title-medium"]);
    title.set_valign(gtk::Align::Center);
    head.append(&title);
    let idle_chars = known.iter().filter(|r| r.idle_total().unwrap_or(0) > 0).count();
    let summary = if known.is_empty() {
        "Waiting for skills and jobs to load".to_string()
    } else {
        format!(
            "{} of {} in use · {idle_chars} character{} with idle slots",
            fmt::int(usage.used.iter().sum()),
            fmt::int(slots.iter().sum()),
            if idle_chars == 1 { "" } else { "s" }
        )
    };
    let sub = w::line(&summary, &["body-small", "dim"]);
    sub.set_valign(gtk::Align::Center);
    sub.set_hexpand(true);
    head.append(&sub);
    let toggle = w::icon_button(if hidden { "expand_more" } else { "expand_less" }, if hidden { "Show slots" } else { "Hide slots" });
    {
        let ctx = ctx.clone();
        toggle.connect_clicked(move |_| {
            ctx.remember(SLOTS_HIDDEN_KEY, if hidden { "0" } else { "1" });
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        });
    }
    head.append(&toggle);
    section.append(&head);
    if hidden {
        return section;
    }

    let tiles: Vec<gtk::Box> = SlotKind::ALL
        .iter()
        .map(|k| {
            let i = k.index();
            let idle = (slots[i] - usage.used[i]).max(0);
            let tile = w::stat_tile(&format!("Idle {}", k.label().to_lowercase()), &fmt::int(idle), Some(&format!("{} of {} in use", usage.used[i], slots[i])));
            if idle > 0 {
                tile.add_css_class("accent");
            }
            tile
        })
        .collect();
    section.append(&w::tile_row(tiles));

    if rows.len() > 1 {
        rows.push(total);
    }
    let ctx2 = ctx.clone();
    let mut t = Table::new(rows).widget_sorted(
        "Character",
        |r| if r.total { w::line(&r.name, &["label-large"]).upcast() } else { char_cell(r.character_id, &r.name) },
        |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    );
    t = t.expand();
    for kind in SlotKind::ALL {
        t = t.widget_sorted(
            kind.label(),
            move |r| slot_cell(r, kind),
            move |a, b| a.idle(kind).cmp(&b.idle(kind)).then_with(|| a.name.cmp(&b.name)),
        );
    }
    let table = t
        .widget_sorted(
            "Idle",
            |r| match r.idle_total() {
                None => w::line("—", &["dim"]).upcast(),
                Some(0) => w::status("Busy", Status::Good).upcast(),
                Some(n) => w::status(&format!("{n} idle"), Status::Warn).upcast(),
            },
            |a, b| a.idle_total().cmp(&b.idle_total()).then_with(|| b.name.to_lowercase().cmp(&a.name.to_lowercase())),
        )
        .widget_sorted(
            "Next done",
            |r| next_end_cell(r.usage.and_then(|u| u.next_end)),
            // characters with nothing running last
            |a, b| {
                let end = |r: &SlotRow| r.usage.and_then(|u| u.next_end).unwrap_or(chrono::DateTime::<Utc>::MAX_UTC);
                end(a).cmp(&end(b))
            },
        )
        .num(
            "Ready",
            |r| r.usage.map(|u| fmt::int(u.ready as i64)).unwrap_or_else(|| "—".into()),
            |r| r.usage.map_or(-1.0, |u| u.ready as f64),
        )
        .pinned_last(|r| r.total)
        .sort_by(4, true)
        .max_height(280)
        .on_activate(move |r| {
            if !r.total {
                ctx2.navigate(Route::Character(r.character_id, CharTab::Industry));
            }
        })
        .build();
    section.append(&table);
    section
}

/// "3 / 5" slots in use; tinted when some are idle.
fn slot_cell(r: &SlotRow, kind: SlotKind) -> gtk::Widget {
    let i = kind.index();
    let text = match (r.slots, r.usage) {
        (Some(s), Some(u)) => format!("{} / {}", u.used[i], s[i]),
        (None, Some(u)) => format!("{} / ?", u.used[i]),
        (Some(s), None) => format!("? / {}", s[i]),
        (None, None) => "—".into(),
    };
    let classes: &[&str] = match r.idle(kind) {
        Some(n) if n > 0 => &["numeric", "warning-text"],
        None => &["numeric", "dim"],
        _ => &["numeric"],
    };
    let l = w::line(&text, classes);
    l.set_xalign(1.0);
    l.upcast()
}

/// A countdown to `end`.
fn next_end_cell(end: Option<chrono::DateTime<Utc>>) -> gtk::Widget {
    let Some(end) = end else { return w::line("—", &["dim"]).upcast() };
    let l = w::line("", &["numeric"]);
    l.set_tooltip_text(Some(&format!("Ends {}", fmt::datetime(end))));
    let update = {
        let weak = l.downgrade();
        move || {
            if let Some(l) = weak.upgrade() {
                l.set_text(&if end > Utc::now() { fmt::time_until(end) } else { "Ready".into() });
            }
        }
    };
    update();
    w::ticker(&l, TICK, update);
    l.upcast()
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
    let status_key = format!("{key}.filter");
    let activity_key = format!("{key}.activity");
    let status: usize = ctx.recall(&status_key).and_then(|v| v.parse().ok()).unwrap_or(0);
    let activity: usize = ctx.recall(&activity_key).and_then(|v| v.parse().ok()).filter(|i| *i < ACTIVITIES.len()).unwrap_or(0);
    let filter = Rc::new(std::cell::Cell::new((status, activity)));

    let table_box = w::vbox(0);
    table_box.set_vexpand(true);
    let rows = Rc::new(rows);
    let fill = {
        let ctx = ctx.clone();
        let table_box = table_box.clone();
        let rows = rows.clone();
        let filter = filter.clone();
        Rc::new(move || {
            while let Some(child) = table_box.first_child() {
                table_box.remove(&child);
            }
            let now = Utc::now();
            let (status, activity) = filter.get();
            let list: Vec<Row> = rows
                .iter()
                .filter(|r| match status {
                    1 => ind::job_state(&r.job, now) == JobState::Active,
                    2 => ind::job_state(&r.job, now) == JobState::Ready,
                    3 => ind::job_state(&r.job, now) == JobState::Paused,
                    _ => true,
                })
                .filter(|r| activity == 0 || SlotKind::of_activity(r.job.activity_id) == SlotKind::ALL[activity - 1])
                .cloned()
                .collect();
            table_box.append(&jobs_table(&ctx, list, show_character, rows.is_empty()));
        })
    };
    fill();

    let bar = w::hbox(12);
    {
        let (ctx, filter, fill) = (ctx.clone(), filter.clone(), fill.clone());
        bar.append(&w::segmented(&FILTERS, status, move |i| {
            ctx.remember(&status_key, i.to_string());
            filter.set((i, filter.get().1));
            fill();
        }));
    }
    let activities = w::dropdown(&ACTIVITIES, activity as u32);
    activities.set_tooltip_text(Some("Jobs by the slot they take"));
    {
        let (ctx, filter, fill) = (ctx.clone(), filter.clone(), fill.clone());
        activities.connect_selected_notify(move |d| {
            let i = d.selected() as usize;
            ctx.remember(&activity_key, i.to_string());
            filter.set((filter.get().0, i));
            fill();
        });
    }
    bar.append(&activities);
    view.append(&bar);
    view.append(&table_box);
    view
}

fn jobs_table(ctx: &Ctx, rows: Vec<Row>, show_character: bool, none_at_all: bool) -> gtk::Widget {
    if rows.is_empty() {
        return if none_at_all {
            w::empty_state("precision_manufacturing", "No industry jobs running", "Jobs started in-game show up here, with a countdown to when they can be delivered.").upcast()
        } else {
            w::empty_state("filter_alt_off", "No jobs match", "No jobs have this status and activity right now.").upcast()
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
