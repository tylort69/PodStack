//! Planetary Industry → Maintenance: everything to do across all characters' colonies, most urgent first (restart
//! extractors, pick up full storage, deliver inputs between planets, check stalled factories), as one prioritized
//! list or one action list per character (log in once, do everything). Tasks are checked off as you go; done ones
//! fold into a "Done" section until the colony's data changes (which gives the task a new key).

use super::colonies::banners;
use super::{common, tab_bar, Tab};
use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::widgets::{self as w, Btn, Status};
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::features::planetary::{self as pl, Priority, Task, TaskKind};
use podstack_core::{config, fmt};
use std::collections::BTreeMap;

const KEY_VIEW: &str = "planets.maintenance.view";
const KEY_CHAR: &str = "planets.maintenance.character";
const KEY_KIND: &str = "planets.maintenance.kind";
const KEY_DONE: &str = "planets.maintenance.show_done";

const KINDS: [TaskKind; 4] = [TaskKind::RestartExtractors, TaskKind::PickUp, TaskKind::Deliver, TaskKind::CheckFactories];
const PRIORITIES: [Priority; 3] = [Priority::Urgent, Priority::High, Priority::Normal];

const CSS: &str = r#"
.pi-task { padding: 8px 4px; border-bottom: 1px solid @outline_variant; }
.pi-task:last-child { border-bottom: none; }
.pi-task-done label { color: @on_surface_variant; }
.pi-group-title { color: @on_surface_variant; font-size: 12px; font-weight: 500; letter-spacing: 0.5px; margin-top: 8px; }
.pi-priority { min-width: 56px; }
"#;

fn kind_icon(k: TaskKind) -> &'static str {
    match k {
        TaskKind::RestartExtractors => "restart_alt",
        TaskKind::PickUp => "inventory_2",
        TaskKind::Deliver => "local_shipping",
        TaskKind::CheckFactories => "precision_manufacturing",
    }
}

fn priority_pill(p: Priority) -> gtk::Label {
    let l = w::status(
        p.label(),
        match p {
            Priority::Urgent => Status::Bad,
            Priority::High => Status::Warn,
            Priority::Normal => Status::Info,
        },
    );
    l.add_css_class("pi-priority");
    l
}

/// Rebuilds once the current signal handler has returned (the rebuild replaces the widget being clicked).
fn rebuild_soon(ctx: &Ctx) {
    let ctx = ctx.clone();
    gtk::glib::idle_add_local_once(move || ctx.rebuild());
}

/// Marks tasks done or open again, and saves.
fn mark(ctx: &Ctx, keys: &[String], done: bool) {
    let mut doc = pl::load();
    let now = Utc::now();
    for k in keys {
        doc.set_done(k, done, now);
    }
    pl::save(ctx.core(), &doc);
    rebuild_soon(ctx);
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(common::CSS);
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Maintenance));
    let chars = ctx.core().characters();
    for b in banners(&chars) {
        body.append(&b);
    }
    let now = Utc::now();
    let s = common::settings();
    let mut doc = pl::load();
    doc.prune_done(now);
    let all = pl::tasks(&chars, now, &s);
    let (done, open): (Vec<Task>, Vec<Task>) = all.into_iter().partition(|t| doc.is_done(&t.key));

    // ---- tiles
    let count = |p: Priority| open.iter().filter(|t| t.priority == p).count();
    let mut who: BTreeMap<i64, String> = BTreeMap::new();
    for t in &open {
        who.insert(t.character_id, t.character_name.clone());
    }
    let cross = open.iter().filter(|t| t.cross_character()).count();
    let urgent = w::stat_tile("Urgent", &fmt::int(count(Priority::Urgent) as i64), Some("Stopped or about to"));
    if count(Priority::Urgent) > 0 {
        urgent.add_css_class("accent");
    }
    body.append(&w::tile_row(vec![
        urgent,
        w::stat_tile("High", &fmt::int(count(Priority::High) as i64), Some("Within the warning window")),
        w::stat_tile("Normal", &fmt::int(count(Priority::Normal) as i64), Some("When convenient")),
        w::stat_tile(
            "Characters to log in",
            &fmt::int(who.len() as i64),
            Some(&if cross > 0 { format!("{cross} cross-character deliver{}", if cross == 1 { "y" } else { "ies" }) } else { "To act in-game".into() }),
        ),
    ]));

    let has_colonies = chars.iter().any(|c| c.planets.as_ref().is_some_and(|p| !p.colonies.is_empty()));
    if !has_colonies {
        body.append(&w::empty_state("public", "No planetary colonies", "Once colonies load, what they need (restarts, pickups, deliveries) shows up here."));
        return Page::new("Planetary Industry", body);
    }

    // ---- filters
    let by_character = ctx.recall(KEY_VIEW).as_deref() == Some("character");
    let mut names: Vec<(i64, String)> = chars.iter().filter(|c| c.planets.as_ref().is_some_and(|p| !p.colonies.is_empty())).map(|c| (c.id, c.name())).collect();
    names.sort_by_key(|n| n.1.to_lowercase());
    let character: Option<i64> = ctx.recall(KEY_CHAR).and_then(|v| v.parse().ok()).filter(|id| names.iter().any(|n| n.0 == *id));
    let kind: Option<TaskKind> = ctx.recall(KEY_KIND).and_then(|v| v.parse::<usize>().ok()).and_then(|i| KINDS.get(i).copied());
    let keep = |t: &&Task| character.is_none_or(|id| t.character_id == id) && kind.is_none_or(|k| t.kind == k);

    let bar = w::hbox(12);
    {
        let ctx = ctx.clone();
        bar.append(&w::segmented(&["By priority", "By character"], by_character as usize, move |i| {
            ctx.remember(KEY_VIEW, if i == 1 { "character" } else { "priority" });
            rebuild_soon(&ctx);
        }));
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    if names.len() > 1 {
        let mut items = vec!["All characters".to_string()];
        items.extend(names.iter().map(|n| n.1.clone()));
        let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
        let sel = character.and_then(|id| names.iter().position(|n| n.0 == id)).map(|i| i + 1).unwrap_or(0);
        let dd = w::dropdown(&refs, sel as u32);
        dd.set_tooltip_text(Some("One character's tasks"));
        let ctx = ctx.clone();
        let ids: Vec<i64> = names.iter().map(|n| n.0).collect();
        dd.connect_selected_notify(move |d| {
            let id = (d.selected() as usize).checked_sub(1).and_then(|i| ids.get(i));
            ctx.remember(KEY_CHAR, id.map(|i| i.to_string()).unwrap_or_default());
            rebuild_soon(&ctx);
        });
        bar.append(&dd);
    }
    let mut kinds = vec!["All tasks"];
    kinds.extend(KINDS.iter().map(|k| k.label()));
    let kd = w::dropdown(&kinds, kind.and_then(|k| KINDS.iter().position(|x| *x == k)).map(|i| i + 1).unwrap_or(0) as u32);
    kd.set_tooltip_text(Some("One kind of task"));
    {
        let ctx = ctx.clone();
        kd.connect_selected_notify(move |d| {
            let v = (d.selected() as usize).checked_sub(1).map(|i| i.to_string()).unwrap_or_default();
            ctx.remember(KEY_KIND, v);
            rebuild_soon(&ctx);
        });
    }
    bar.append(&kd);
    body.append(&bar);

    // ---- open tasks
    let shown: Vec<&Task> = open.iter().filter(keep).collect();
    if shown.is_empty() {
        let (title, text) = if open.is_empty() {
            ("Nothing to do", "Every colony is running fine: no extractors stopping soon, no full storage, no inputs running out.")
        } else {
            ("Nothing to do here", "No open tasks for this character and kind. Others are waiting under a different filter.")
        };
        body.append(&w::empty_state("task_alt", title, text));
    } else if by_character {
        let mut groups: Vec<(i64, String, Vec<&Task>)> = Vec::new();
        for t in &shown {
            match groups.iter_mut().find(|g| g.0 == t.character_id) {
                Some(g) => g.2.push(t),
                None => groups.push((t.character_id, t.character_name.clone(), vec![t])),
            }
        }
        // the character with the most urgent work first (tasks are already sorted by priority)
        groups.sort_by(|a, b| a.2[0].priority.cmp(&b.2[0].priority).then(b.2.len().cmp(&a.2.len())).then_with(|| a.1.cmp(&b.1)));
        for (id, name, tasks) in groups {
            body.append(&character_group(ctx, id, &name, &tasks));
        }
    } else {
        for p in PRIORITIES {
            let tasks: Vec<&Task> = shown.iter().copied().filter(|t| t.priority == p).collect();
            if tasks.is_empty() {
                continue;
            }
            body.append(&w::line(&format!("{} · {}", p.label().to_uppercase(), tasks.len()), &["pi-group-title"]));
            let list = w::vbox(0);
            list.add_css_class("card");
            for t in tasks {
                list.append(&task_row(ctx, t, true, false));
            }
            body.append(&list);
        }
    }

    // ---- done
    let done_shown: Vec<&Task> = done.iter().filter(keep).collect();
    if !done_shown.is_empty() {
        let list = w::vbox(0);
        list.add_css_class("card");
        for t in &done_shown {
            list.append(&task_row(ctx, t, true, true));
        }
        let keys: Vec<String> = done_shown.iter().map(|t| t.key.clone()).collect();
        let reopen = w::button("Reopen all", Btn::Text);
        reopen.set_halign(gtk::Align::End);
        {
            let ctx = ctx.clone();
            reopen.connect_clicked(move |_| mark(&ctx, &keys, false));
        }
        let inner = w::vbox(8);
        inner.append(&list);
        inner.append(&reopen);
        let exp = gtk::Expander::new(Some(&format!("Done ({})", done_shown.len())));
        exp.set_margin_top(8);
        exp.set_expanded(ctx.recall(KEY_DONE).as_deref() == Some("1"));
        exp.set_child(Some(&inner));
        let ctx2 = ctx.clone();
        exp.connect_expanded_notify(move |e| ctx2.remember(KEY_DONE, if e.is_expanded() { "1" } else { "0" }));
        body.append(&exp);
    }
    let note = w::line(
        "Done marks last until the colony's data next changes in-game (then the task comes back if it still applies), at most 14 days.",
        &["body-small", "dim"],
    );
    note.set_wrap(true);
    note.set_xalign(0.0);
    body.append(&note);
    Page::new("Planetary Industry", body)
}

/// One character's action list: who to log in, what to do there, and a button to check it all off.
fn character_group(ctx: &Ctx, id: i64, name: &str, tasks: &[&Task]) -> gtk::Box {
    let card = w::vbox(4);
    card.add_css_class("card");
    let head = w::hbox(12);
    head.append(&w::avatar(&config::character_portrait(id, 64), 36));
    let titles = w::vbox(0);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    let n = w::line(name, &["title-medium"]);
    n.set_ellipsize(gtk::pango::EllipsizeMode::End);
    titles.append(&n);
    let planets: std::collections::BTreeSet<i64> = tasks.iter().map(|t| t.planet_id).collect();
    titles.append(&w::line(
        &format!("{} task{} on {} planet{}", tasks.len(), if tasks.len() == 1 { "" } else { "s" }, planets.len(), if planets.len() == 1 { "" } else { "s" }),
        &["body-small", "dim"],
    ));
    head.append(&titles);
    head.append(&priority_pill(tasks[0].priority));
    let open_char = w::icon_button("open_in_new", "Open this character's PI tab");
    {
        let ctx = ctx.clone();
        open_char.connect_clicked(move |_| ctx.navigate(Route::Character(id, CharTab::Planets)));
    }
    head.append(&open_char);
    let all = w::button_icon("done_all", "All done", Btn::Tonal);
    all.set_valign(gtk::Align::Center);
    {
        let ctx = ctx.clone();
        let keys: Vec<String> = tasks.iter().map(|t| t.key.clone()).collect();
        all.connect_clicked(move |_| mark(&ctx, &keys, true));
    }
    head.append(&all);
    card.append(&head);
    let list = w::vbox(0);
    for t in tasks {
        list.append(&task_row(ctx, t, false, false));
    }
    card.append(&list);
    card
}

/// A task: check button, priority, kind, (character), planet and detail, delivery source, and when it's due.
fn task_row(ctx: &Ctx, t: &Task, show_character: bool, done: bool) -> gtk::Box {
    let row = w::hbox(12);
    row.add_css_class("pi-task");
    if done {
        row.add_css_class("pi-task-done");
    }
    let check = gtk::CheckButton::new();
    check.set_active(done);
    check.set_valign(gtk::Align::Center);
    check.set_tooltip_text(Some(if done { "Mark as not done" } else { "Mark as done" }));
    {
        let ctx = ctx.clone();
        let key = t.key.clone();
        check.connect_toggled(move |c| mark(&ctx, std::slice::from_ref(&key), c.is_active()));
    }
    row.append(&check);
    row.append(&priority_pill(t.priority));

    let kind = w::hbox(6);
    kind.set_size_request(140, -1);
    kind.append(&w::icon_sized(kind_icon(t.kind), 18));
    let kl = w::line(t.kind.label(), &["body-medium"]);
    kl.set_ellipsize(gtk::pango::EllipsizeMode::End);
    kind.append(&kl);
    row.append(&kind);

    if show_character {
        let who = w::hbox(6);
        who.set_size_request(150, -1);
        who.append(&w::avatar(&config::character_portrait(t.character_id, 64), 24));
        let n = w::line(&t.character_name, &["body-medium"]);
        n.set_ellipsize(gtk::pango::EllipsizeMode::End);
        who.append(&n);
        who.set_tooltip_text(Some(&t.character_name));
        row.append(&who);
    }

    let what = w::vbox(2);
    what.set_hexpand(true);
    what.set_valign(gtk::Align::Center);
    let planet = w::line(&t.planet_name, &["title-small"]);
    planet.set_ellipsize(gtk::pango::EllipsizeMode::End);
    what.append(&planet);
    let detail = w::line(&t.detail, &["body-small", "dim"]);
    detail.set_wrap(true);
    detail.set_xalign(0.0);
    what.append(&detail);
    if t.kind == TaskKind::Deliver {
        let src = w::hbox(6);
        match &t.source {
            Some((_, cname, pname)) => {
                let l = w::line(&format!("from {pname} ({cname})"), &["body-small"]);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                src.append(&l);
                if t.cross_character() {
                    let flag = w::status("Cross-character", Status::Warn);
                    flag.set_tooltip_text(Some("Made by another character's colony: haul it over or contract it"));
                    src.append(&flag);
                }
            }
            None => src.append(&w::line("None of your colonies makes this: buy it or haul it in", &["body-small", "dim"])),
        }
        what.append(&src);
    }
    row.append(&what);

    let due = w::line("", &["body-medium", "numeric"]);
    due.set_xalign(1.0);
    due.set_size_request(80, -1);
    match t.due {
        Some(end) => {
            due.set_tooltip_text(Some(&fmt::datetime(end)));
            let weak = due.downgrade();
            let update = move || {
                if let Some(l) = weak.upgrade() {
                    let now = Utc::now();
                    if end > now {
                        l.set_text(&fmt::duration(end - now, 2));
                    } else {
                        l.set_text("Now");
                        l.add_css_class("error-text");
                    }
                }
            };
            update();
            w::ticker(&due, 10, update);
        }
        None if t.kind == TaskKind::RestartExtractors && t.priority == Priority::Urgent => {
            due.set_text("Now");
            due.add_css_class("error-text");
        }
        None => {
            due.set_text("—");
            due.add_css_class("dim");
        }
    }
    row.append(&due);
    row
}
