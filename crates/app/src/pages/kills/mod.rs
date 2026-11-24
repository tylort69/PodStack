//! Kills & Losses: every character's killmails together, from PodStack's killmail archive (history beyond ESI's
//! 90 days), laid out like the Wallet page: headline numbers for a chosen period, kills and losses over time, the
//! characters side by side, the ships flown, what was killed, where, who the enemies are and when the fights
//! happen, the biggest kills and losses, and the mails themselves, each opening a detail view. A mail several of
//! the user's characters were on counts once.

mod chart;
mod detail;

use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::pages::character::summary::parts::sec_label;
use crate::pages::character::wallet::CSS as WALLET_CSS;
use crate::pages::market_orders::{character_cell, label_cell, set_child, type_cell};
use crate::widgets::{self as w, BannerKind, Btn, Status, Table};
use chart::Slot;
use gtk::prelude::*;
use podstack_core::features::killboard::{self as kb, Analysis, Archive, CharacterStats, Entry, Group, Tally};
use podstack_core::features::wallet_flow::{BucketSize, Period};
use podstack_core::{config, fmt, Character};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const PERIOD_KEY: &str = "killboard.period";
const CHAR_KEY: &str = "killboard.char";
const CHART_KEY: &str = "killboard.chart";
const VICTIM_KEY: &str = "killboard.victims";
const PLACE_KEY: &str = "killboard.place";
const ENEMY_KEY: &str = "killboard.enemies";
const LIST_KEY: &str = "killboard.list";
const SEARCH_KEY: &str = "killboard.search";

const CSS: &str = "
.kl-split > * { min-width: 0; }
.kl-row { padding: 6px 8px; border-radius: 12px; }
.kl-swatch { min-width: 10px; min-height: 10px; border-radius: 5px; }
.kl-swatch-kill { background-color: @primary; }
.kl-swatch-loss { background-color: @tertiary; }
.kl-attacker { padding: 6px 8px; border-radius: 12px; }
.kl-mine { background-color: alpha(@primary, 0.12); }
.kl-item { padding: 2px 0; }
.kl-kill-text { color: @primary; }
.kl-loss-text { color: @tertiary; }
";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(WALLET_CSS);
    crate::theme::add_css(CSS);
    let body = w::page_body();
    let all_chars = ctx.core().characters();
    if all_chars.is_empty() {
        body.append(&w::empty_state("gps_fixed", "No characters yet", "Add a character to see its kills and losses here."));
        return Page::new("Kills & Losses", body);
    }

    let now = chrono::Utc::now();
    let period = ctx.recall(PERIOD_KEY).and_then(|k| Period::from_key(&k)).unwrap_or(Period::Days(30));
    let since = period.since(now);
    let only: Option<i64> = ctx.recall(CHAR_KEY).and_then(|v| v.parse().ok()).filter(|id| all_chars.iter().any(|c| c.id == *id));
    let chars: Vec<Arc<Character>> = all_chars.iter().filter(|c| only.is_none_or(|id| c.id == id)).cloned().collect();
    let names: Rc<HashMap<i64, String>> = Rc::new(all_chars.iter().map(|c| (c.id, c.name())).collect());
    let all_ids: Vec<i64> = all_chars.iter().map(|c| c.id).collect();
    let mine: HashSet<i64> = all_ids.iter().copied().collect();

    let archive = kb::archive_snapshot();
    // every mail is classified against all the user's characters, so a fight between two of them reads the same
    // whichever one is shown
    let all_entries = kb::entries(&archive, &mine, since);
    let everyone = kb::analyze(&archive, &all_entries, &all_ids, since, now);
    let entries: Vec<Entry> = match only {
        Some(id) => all_entries.iter().filter(|e| e.chars.contains(&id)).cloned().collect(),
        None => all_entries,
    };
    let a = match only {
        Some(_) => kb::analyze(&archive, &entries, &all_ids, since, now),
        None => everyone.clone(),
    };

    body.append(&filter_bar(ctx, &all_chars, period, only));

    let missing = kb::missing_scope(&chars);
    if !missing.is_empty() {
        if only.is_some() || missing.len() == chars.len() {
            body.append(&w::scope_notice(kb::SCOPE));
        } else {
            let who: Vec<String> = missing.iter().map(|c| c.name()).collect();
            body.append(&w::banner(
                "lock",
                &format!(
                    "Not counted: {}. Their logins don't include {}; log in again with \"Add character\" to grant it.",
                    who.join(", "),
                    kb::SCOPE
                ),
                BannerKind::Notice,
            ));
        }
    }
    let waiting = chars.iter().filter(|c| c.killmails.is_none() && !missing.iter().any(|m| m.id == c.id)).count();
    if waiting > 0 {
        body.append(&w::text(
            &format!("Waiting for the killmails of {waiting} character{}; they appear after the next refresh.", if waiting == 1 { "" } else { "s" }),
            &["body-medium", "dim"],
        ));
    }

    let ever: HashSet<i64> = chars.iter().map(|c| c.id).collect();
    if !archive.kills.values().any(|k| kb::entry(k, &mine).is_some_and(|e| e.chars.iter().any(|c| ever.contains(c)))) {
        body.append(&w::empty_state(
            "gps_fixed",
            "No kills or losses yet",
            "Your characters' kills and losses show here once PodStack has seen them. ESI lists the last 90 days; PodStack keeps every mail it sees, so the history grows from there.",
        ));
        body.append(&sources_card(ctx));
        return Page::new("Kills & Losses", body);
    }

    body.append(&tiles(&a, period));
    body.append(&over_time(ctx, &a));

    body.append(&w::card_with(
        "Characters",
        Some(if only.is_some() {
            "All characters, this period · double-click to show one"
        } else {
            "A shared mail counts for each character on it · double-click to show one"
        }),
        &[],
        &characters_table(ctx, &everyone.characters, &all_chars),
    ));

    let split = w::hbox(16);
    split.add_css_class("kl-split");
    split.set_homogeneous(true);
    split.append(&w::card_with(
        "Ships you flew",
        Some("Per hull · a shared kill counts for each hull on it"),
        &[],
        &groups_table(&a.my_ships, true, "No ships in this period."),
    ));
    split.append(&victims_card(ctx, &a));
    body.append(&split);

    let split = w::hbox(16);
    split.add_css_class("kl-split");
    split.set_homogeneous(true);
    split.append(&places_card(ctx, &a));
    split.append(&enemies_card(ctx, &a));
    body.append(&split);

    body.append(&time_card(&a));

    let archive_rc = archive.clone();
    let split = w::hbox(16);
    split.add_css_class("kl-split");
    split.set_homogeneous(true);
    split.append(&w::card_with(
        "Top kills",
        Some("Most valuable this period"),
        &[],
        &top_list(ctx, &archive_rc, &a.top_kills, &entries, &mine, "No kills in this period."),
    ));
    split.append(&w::card_with(
        "Most expensive losses",
        Some("This period"),
        &[],
        &top_list(ctx, &archive_rc, &a.top_losses, &entries, &mine, "No losses in this period."),
    ));
    body.append(&split);

    body.append(&mail_list(ctx, archive, entries, names, &mine, period, &chars));
    body.append(&sources_card(ctx));
    Page::new("Kills & Losses", body)
}

/// Where the mails come from, and the switch for zKillboard.
fn sources_card(ctx: &Ctx) -> gtk::Widget {
    let core = ctx.core().clone();
    let row = w::switch_row(
        "Include kills you took part in (from zKillboard)",
        Some(
            "ESI only lists your losses and the kills where you landed the final blow. zKillboard has the rest, and your \
             history from before PodStack; to ask for them, PodStack sends zKillboard your characters' public character \
             IDs, about once an hour. Takes effect on the next refresh.",
        ),
        kb::zkill_enabled(&core),
        move |on| core.set_setting(kb::ZKILL_SETTING, &Some(on)),
    );
    w::card_with("Sources", Some("Every killmail ESI lists, and optionally zKillboard's"), &[], &row).upcast()
}

/// Remembers a choice and rebuilds the page once the clicked widget's handler has returned (the rebuild replaces it).
fn choose(ctx: &Ctx, key: &str, value: String) {
    ctx.remember(key, value);
    let ctx = ctx.clone();
    gtk::glib::idle_add_local_once(move || ctx.rebuild());
}

fn filter_bar(ctx: &Ctx, chars: &[Arc<Character>], period: Period, only: Option<i64>) -> gtk::Box {
    let bar = w::hbox(12);
    bar.add_css_class("comms-filter-bar");
    let labels: Vec<String> = Period::ALL.iter().map(|p| p.label()).collect();
    let labels: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let selected = Period::ALL.iter().position(|p| *p == period).unwrap_or(1);
    {
        let ctx = ctx.clone();
        bar.append(&w::segmented(&labels, selected, move |i| choose(&ctx, PERIOD_KEY, Period::ALL[i].key())));
    }
    let mut items = vec!["All characters".to_string()];
    items.extend(chars.iter().map(|c| c.name()));
    let items: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    let pick = w::dropdown(&items, only.and_then(|id| chars.iter().position(|c| c.id == id)).map(|i| i as u32 + 1).unwrap_or(0));
    {
        let (ctx, ids) = (ctx.clone(), chars.iter().map(|c| c.id).collect::<Vec<_>>());
        pick.connect_selected_notify(move |d| {
            let v = (d.selected() as usize).checked_sub(1).and_then(|i| ids.get(i)).map(|id| id.to_string()).unwrap_or_default();
            choose(&ctx, CHAR_KEY, v);
        });
    }
    bar.append(&pick);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let updated = chars.iter().filter(|c| only.is_none_or(|id| c.id == id)).filter_map(|c| c.refreshes.get("killmails").and_then(|r| r.last)).max();
    if let Some(t) = updated {
        let l = w::line(&format!("Updated {}", fmt::relative(t)), &["body-small", "dim"]);
        l.set_valign(gtk::Align::Center);
        bar.append(&l);
    }
    bar
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{} {}", fmt::int(n as i64), if n == 1 { one } else { many })
}

fn efficiency_text(t: &Tally) -> String {
    t.efficiency().map(|e| format!("{:.1}%", e * 100.0)).unwrap_or_else(|| "—".into())
}

fn tiles(a: &Analysis, period: Period) -> gtk::Grid {
    let grid = gtk::Grid::new();
    grid.add_css_class("comms-tile-grid");
    grid.set_row_spacing(12);
    grid.set_column_spacing(12);
    grid.set_column_homogeneous(true);
    let t = &a.totals;
    let when = match (period, a.first) {
        (Period::All, Some(first)) => format!("since {}", first.format("%b %-d, %Y")),
        (Period::All, None) => "all kept".into(),
        (p, _) => p.label(),
    };
    let isk_tile = |label: &str, v: f64, sub: &str| {
        let tile = w::stat_tile(label, &format!("{} ISK", fmt::isk_short(v)), Some(sub));
        tile.set_tooltip_text(Some(&fmt::isk(v)));
        tile
    };
    let kills = w::stat_tile(&format!("Kills · {when}"), &fmt::int(t.tally.kills as i64), Some(&format!("{} solo · {} gang", t.solo, t.gang)));
    kills.add_css_class("accent");
    grid.attach(&kills, 0, 0, 1, 1);
    let losses = w::stat_tile(&format!("Losses · {when}"), &fmt::int(t.tally.losses as i64), None);
    if t.tally.losses > 0 {
        losses.add_css_class("cx-warn-tile");
    }
    grid.attach(&losses, 1, 0, 1, 1);
    grid.attach(&isk_tile("ISK destroyed", t.tally.destroyed, &plural(t.tally.kills, "kill", "kills")), 2, 0, 1, 1);
    grid.attach(&isk_tile("ISK lost", t.tally.lost, &plural(t.tally.losses, "loss", "losses")), 3, 0, 1, 1);

    let eff = w::stat_tile("ISK efficiency", &efficiency_text(&t.tally), Some("destroyed ÷ destroyed + lost"));
    if let (Some(e), Some(v)) = (t.tally.efficiency(), eff.first_child().and_then(|l| l.next_sibling())) {
        v.add_css_class(if e >= 0.5 { "comms-isk-in" } else { "comms-isk-out" });
    }
    grid.attach(&eff, 0, 1, 1, 1);
    let solo_share = if t.tally.kills > 0 { format!("{:.0}% of kills", t.solo as f64 / t.tally.kills as f64 * 100.0) } else { "—".into() };
    grid.attach(&w::stat_tile("Solo kills", &fmt::int(t.solo as i64), Some(&solo_share)), 1, 1, 1, 1);
    let fb_share = if t.tally.kills > 0 { format!("on {:.0}% of kills", t.final_blows as f64 / t.tally.kills as f64 * 100.0) } else { "—".into() };
    grid.attach(&w::stat_tile("Final blows", &fmt::int(t.final_blows as i64), Some(&fb_share)), 2, 1, 1, 1);
    let shared = w::stat_tile("Shared mails", &fmt::int(t.shared as i64), Some("several of your characters on one"));
    shared.set_tooltip_text(Some("A mail several of your characters were on counts once in these totals"));
    grid.attach(&shared, 3, 1, 1, 1);
    if t.unvalued > 0 {
        grid.set_tooltip_text(Some(&format!("{} not valued yet (they count as 0 ISK until prices load)", plural(t.unvalued, "mail", "mails"))));
    }
    grid
}

fn over_time(ctx: &Ctx, a: &Analysis) -> gtk::Widget {
    let counts = ctx.recall(CHART_KEY).as_deref() == Some("count");
    let size = a.bucket_size.unwrap_or(BucketSize::Day);
    let body: gtk::Widget = if a.buckets.iter().any(|b| b.tally.count() > 0) {
        let slots = a
            .buckets
            .iter()
            .map(|b| Slot {
                label: if size == BucketSize::Month { b.start.format("%b %Y").to_string() } else { b.start.format("%b %-d").to_string() },
                kills: if counts { b.tally.kills as f64 } else { b.tally.destroyed },
                losses: if counts { b.tally.losses as f64 } else { b.tally.lost },
                title: match size {
                    BucketSize::Day => b.start.format("%A, %b %-d").to_string(),
                    BucketSize::Week => format!("Week of {}", b.start.format("%b %-d")),
                    BucketSize::Month => b.start.format("%B %Y").to_string(),
                },
                detail: tally_detail(&b.tally),
            })
            .collect();
        chart::pairs(slots, 200, !counts, 8)
    } else {
        w::text("No kills or losses in this period.", &["body-medium", "dim"]).upcast()
    };
    let per = match a.bucket_size {
        Some(BucketSize::Week) => "Per week",
        Some(BucketSize::Month) => "Per month",
        _ => "Per day",
    };
    let switch = {
        let ctx = ctx.clone();
        w::segmented(&["ISK", "Count"], counts as usize, move |i| choose(&ctx, CHART_KEY, if i == 1 { "count".into() } else { "isk".into() }))
    };
    let card = w::card_with(
        "Kills and losses over time",
        Some(&format!("{per}, EVE time · {}", if counts { "number of mails" } else { "ISK destroyed and lost" })),
        &[chart::legend("Kills", "Losses"), switch.upcast()],
        &body,
    );
    card.upcast()
}

fn tally_detail(t: &Tally) -> String {
    format!(
        "{} ({}) · {} ({})",
        plural(t.kills, "kill", "kills"),
        fmt::isk_short(t.destroyed),
        plural(t.losses, "loss", "losses"),
        fmt::isk_short(t.lost)
    )
}

// ---------- characters and ships ----------

#[derive(Clone)]
struct CharRow {
    id: i64,
    name: String,
    s: CharacterStats,
    ship: Option<String>,
}

fn cmp_f(a: f64, b: f64) -> std::cmp::Ordering {
    a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
}

fn efficiency_cell(t: &Tally) -> gtk::Widget {
    let l = w::line(&efficiency_text(t), &["numeric"]);
    l.set_xalign(1.0);
    if let Some(e) = t.efficiency() {
        l.add_css_class(if e >= 0.5 { "comms-isk-in" } else { "comms-isk-out" });
    }
    l.upcast()
}

fn characters_table(ctx: &Ctx, stats: &[CharacterStats], chars: &[Arc<Character>]) -> gtk::Widget {
    let archive = kb::archive_snapshot();
    let rows: Vec<CharRow> = stats
        .iter()
        .filter_map(|s| {
            let c = chars.iter().find(|c| c.id == s.char_id)?;
            Some(CharRow { id: c.id, name: c.name(), s: s.clone(), ship: s.top_ship.map(|t| archive.type_name(t)) })
        })
        .collect();
    let ctx = ctx.clone();
    Table::new(rows)
        .widget_sorted("Character", |r| character_cell(r.id, &r.name), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .num("Kills", |r| fmt::int(r.s.tally.kills as i64), |r| r.s.tally.kills as f64)
        .num("Losses", |r| fmt::int(r.s.tally.losses as i64), |r| r.s.tally.losses as f64)
        .num("Destroyed", |r| fmt::isk_short(r.s.tally.destroyed), |r| r.s.tally.destroyed)
        .num("Lost", |r| fmt::isk_short(r.s.tally.lost), |r| r.s.tally.lost)
        .widget_sorted(
            "Eff.",
            |r| efficiency_cell(&r.s.tally),
            |a, b| cmp_f(a.s.tally.efficiency().unwrap_or(-1.0), b.s.tally.efficiency().unwrap_or(-1.0)),
        )
        .width(84)
        .num("Solo", |r| fmt::int(r.s.solo as i64), |r| r.s.solo as f64)
        .num("Final blows", |r| fmt::int(r.s.final_blows as i64), |r| r.s.final_blows as f64)
        .num("Shared", |r| fmt::int(r.s.shared as i64), |r| r.s.shared as f64)
        .widget("Flies most", |r| match (&r.s.top_ship, &r.ship) {
            (Some(t), Some(n)) => type_cell(*t, n),
            _ => label_cell("—", &["dim"]),
        })
        .width(160)
        .sort_by(3, true)
        .on_activate(move |r| choose(&ctx, CHAR_KEY, r.id.to_string()))
        .natural_height()
        .build()
}

/// A table of breakdown rows; `types` shows the id as a type icon.
fn groups_table(groups: &[Group], types: bool, empty: &str) -> gtk::Widget {
    Table::new(groups.to_vec())
        .widget_sorted(
            if types { "Ship" } else { "Name" },
            move |g| match (types, g.id) {
                (true, Some(id)) => type_cell(id, &g.label),
                _ => label_cell(&g.label, &[]),
            },
            |a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()),
        )
        .expand()
        .num("Kills", |g| fmt::int(g.tally.kills as i64), |g| g.tally.kills as f64)
        .num("Losses", |g| fmt::int(g.tally.losses as i64), |g| g.tally.losses as f64)
        .num("Destroyed", |g| fmt::isk_short(g.tally.destroyed), |g| g.tally.destroyed)
        .num("Lost", |g| fmt::isk_short(g.tally.lost), |g| g.tally.lost)
        .widget_sorted(
            "Eff.",
            |g| efficiency_cell(&g.tally),
            |a, b| cmp_f(a.tally.efficiency().unwrap_or(-1.0), b.tally.efficiency().unwrap_or(-1.0)),
        )
        .width(84)
        .sort_by(1, true)
        .max_height(380)
        .empty_text(empty)
        .build()
}

// ---------- breakdown lists ----------

/// One row per group: its name, a kills bar over a losses bar (each against the biggest), and the counts.
fn tally_list(groups: &[(String, Tally)], limit: usize, empty: &str) -> gtk::Widget {
    let groups: Vec<&(String, Tally)> = groups.iter().filter(|g| g.1.count() > 0).collect();
    if groups.is_empty() {
        return w::text(empty, &["body-medium", "dim"]).upcast();
    }
    let most = groups.iter().map(|g| g.1.kills.max(g.1.losses)).max().unwrap_or(1).max(1) as f64;
    let list = w::vbox(2);
    for (label, t) in groups.iter().take(limit).map(|g| (&g.0, &g.1)) {
        let row = w::hbox(12);
        row.add_css_class("kl-row");
        let mid = w::vbox(4);
        mid.set_hexpand(true);
        let top = w::hbox(8);
        let name = w::line(label, &["body-medium"]);
        name.set_hexpand(true);
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        top.append(&name);
        let eff = w::line(&efficiency_text(t), &["body-small", "dim", "numeric"]);
        eff.set_ellipsize(gtk::pango::EllipsizeMode::None);
        eff.set_tooltip_text(Some("ISK efficiency"));
        top.append(&eff);
        mid.append(&top);
        mid.append(&w::progress(t.kills as f64 / most));
        let p = w::progress(t.losses as f64 / most);
        p.add_css_class("tertiary");
        mid.append(&p);
        row.append(&mid);
        let counts = w::vbox(0);
        counts.set_valign(gtk::Align::Center);
        let k = w::line(&fmt::int(t.kills as i64), &["title-small", "numeric", "kl-kill-text"]);
        k.set_xalign(1.0);
        counts.append(&k);
        let l = w::line(&fmt::int(t.losses as i64), &["title-small", "numeric", "kl-loss-text"]);
        l.set_xalign(1.0);
        counts.append(&l);
        counts.set_size_request(48, -1);
        row.append(&counts);
        row.set_tooltip_text(Some(&tally_detail(t)));
        list.append(&row);
    }
    if groups.len() > limit {
        let more = w::line(&format!("and {} more", groups.len() - limit), &["body-small", "dim"]);
        more.set_margin_start(8);
        more.set_margin_top(4);
        list.append(&more);
    }
    list.upcast()
}

fn switch(ctx: &Ctx, key: &str, labels: &[&str]) -> (usize, gtk::Widget) {
    let i = ctx.recall(key).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(labels.len() - 1);
    let (ctx, key) = (ctx.clone(), key.to_string());
    (i, w::segmented(labels, i, move |n| choose(&ctx, &key, n.to_string())).upcast())
}

fn as_rows(groups: &[Group]) -> Vec<(String, Tally)> {
    groups.iter().map(|g| (g.label.clone(), g.tally)).collect()
}

fn victims_card(ctx: &Ctx, a: &Analysis) -> gtk::Widget {
    let (i, sw) = switch(ctx, VICTIM_KEY, &["Class", "Group"]);
    let groups = if i == 1 { &a.victim_groups } else { &a.victim_classes };
    w::card_with(
        "Ships killed and lost",
        Some("By the victim's ship · kills above, losses below"),
        &[sw],
        &tally_list(&as_rows(groups), 12, "No kills or losses in this period."),
    )
    .upcast()
}

fn places_card(ctx: &Ctx, a: &Analysis) -> gtk::Widget {
    let (i, sw) = switch(ctx, PLACE_KEY, &["Regions", "Systems"]);
    let content = w::vbox(12);
    let bands: Vec<(String, Tally)> = a.bands.iter().map(|(b, t)| (b.label().to_string(), *t)).collect();
    content.append(&tally_list(&bands, 6, "No kills or losses in this period."));
    let groups = if i == 1 { &a.systems } else { &a.regions };
    if !groups.is_empty() {
        content.append(&w::line(if i == 1 { "Systems" } else { "Regions" }, &["label-large", "dim"]));
        content.append(&tally_list(&as_rows(groups), 8, ""));
    }
    w::card_with("Where", Some("Security band, then region or system"), &[sw], &content).upcast()
}

fn enemies_card(ctx: &Ctx, a: &Analysis) -> gtk::Widget {
    let (i, sw) = switch(ctx, ENEMY_KEY, &["Corporations", "Alliances"]);
    let groups = if i == 1 { &a.enemy_alliances } else { &a.enemy_corps };
    let alliance = i == 1;
    let table = Table::new(groups.clone())
        .widget_sorted(
            if alliance { "Alliance" } else { "Corporation" },
            move |g| {
                let row = w::hbox(10);
                if let Some(id) = g.id {
                    let url = if alliance { config::alliance_logo(id, 64) } else { config::corporation_logo(id, 64) };
                    row.append(&w::eve_image(&url, 24));
                }
                row.append(&w::line(&g.label, &[]));
                row.upcast()
            },
            |a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()),
        )
        .expand()
        .num("You killed", |g| fmt::int(g.tally.kills as i64), |g| g.tally.kills as f64)
        .num("Killed you", |g| fmt::int(g.tally.losses as i64), |g| g.tally.losses as f64)
        .num("Destroyed", |g| fmt::isk_short(g.tally.destroyed), |g| g.tally.destroyed)
        .num("Lost to", |g| fmt::isk_short(g.tally.lost), |g| g.tally.lost)
        .sort_by(1, true)
        .max_height(380)
        .empty_text("No enemies in this period.")
        .build();
    w::card_with("Enemies", Some("Who you kill most and who kills you most · sort by a column"), &[sw], &table).upcast()
}

fn time_card(a: &Analysis) -> gtk::Widget {
    let content = w::hbox(24);
    content.set_homogeneous(false);
    let left = w::vbox(8);
    left.set_hexpand(true);
    left.append(&w::line("Hour of day (EVE time)", &["label-large", "dim"]));
    let hours = a
        .hours
        .iter()
        .enumerate()
        .map(|(h, t)| Slot {
            label: format!("{h:02}"),
            kills: t.kills as f64,
            losses: t.losses as f64,
            title: format!("{h:02}:00–{h:02}:59 EVE"),
            detail: tally_detail(t),
        })
        .collect();
    left.append(&chart::pairs(hours, 160, false, 12));
    content.append(&left);
    let right = w::vbox(8);
    right.set_size_request(360, -1);
    right.append(&w::line("Weekday", &["label-large", "dim"]));
    const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    let days = a
        .weekdays
        .iter()
        .enumerate()
        .map(|(d, t)| Slot {
            label: DAYS[d][..3].to_string(),
            kills: t.kills as f64,
            losses: t.losses as f64,
            title: DAYS[d].to_string(),
            detail: tally_detail(t),
        })
        .collect();
    right.append(&chart::pairs(days, 160, false, 7));
    content.append(&right);
    w::card_with("When you fight", Some("Mails per hour and weekday"), &[chart::legend("Kills", "Losses")], &content).upcast()
}

// ---------- top kills and losses ----------

fn top_list(ctx: &Ctx, archive: &Arc<Archive>, ids: &[i64], entries: &[Entry], mine: &HashSet<i64>, empty: &str) -> gtk::Widget {
    if ids.is_empty() {
        return w::text(empty, &["body-medium", "dim"]).upcast();
    }
    let list = w::vbox(2);
    for (rank, id) in ids.iter().enumerate() {
        let (Some(k), Some(e)) = (archive.get(*id), entries.iter().find(|e| e.id == *id)) else { continue };
        let row = w::hbox(12);
        row.add_css_class("kl-row");
        let rank_l = w::line(&format!("{}", rank + 1), &["body-medium", "dim", "numeric"]);
        rank_l.set_width_chars(2);
        row.append(&rank_l);
        row.append(&w::eve_image(&config::type_icon(k.mail.victim.ship_type_id, 64), 32));
        let mid = w::vbox(2);
        mid.set_hexpand(true);
        let ship = w::line(&archive.type_name(k.mail.victim.ship_type_id), &["body-medium"]);
        ship.set_xalign(0.0);
        mid.append(&ship);
        let sub = w::line(&format!("{} · {} · {}", archive.victim_name(k), k.system, fmt::date(k.time())), &["body-small", "dim"]);
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        mid.append(&sub);
        row.append(&mid);
        if e.shared() {
            let s = w::status(&format!("{} of yours", e.chars.len()), Status::Info);
            row.append(&s);
        }
        let v = w::line(&fmt::isk_short(e.value), &["title-small", "numeric"]);
        v.set_width_chars(8);
        v.set_xalign(1.0);
        row.append(&v);
        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_child(Some(&row));
        button.set_tooltip_text(Some(&format!("{} ISK · click for the details", fmt::isk(e.value))));
        let (ctx, archive, id, mine) = (ctx.clone(), archive.clone(), *id, mine.clone());
        button.connect_clicked(move |_| detail::open(&ctx, archive.clone(), id, &mine));
        list.append(&button);
    }
    list.upcast()
}

// ---------- the mails ----------

/// A mail as the list shows it, with its text worked out once.
#[derive(Clone)]
struct MailRow {
    e: Entry,
    ship_id: i64,
    ship: String,
    victim: String,
    org: String,
    system: String,
    region: String,
    security: Option<f64>,
    yours: String,
    search: String,
}

fn mail_rows(a: &Archive, entries: &[Entry], names: &HashMap<i64, String>) -> Vec<MailRow> {
    entries
        .iter()
        .filter_map(|e| {
            let k = a.get(e.id)?;
            let v = &k.mail.victim;
            let ship = a.type_name(v.ship_type_id);
            let victim = a.victim_name(k);
            let org =
                [v.corporation_id, v.alliance_id].into_iter().flatten().filter_map(|i| a.name(i).map(str::to_string)).collect::<Vec<_>>().join(" · ");
            let yours = e.chars.iter().map(|c| names.get(c).cloned().unwrap_or_else(|| format!("#{c}"))).collect::<Vec<_>>().join(", ");
            let (class, group) = kb::ship_class(v.ship_type_id);
            let attackers: Vec<String> = k.mail.attackers.iter().map(|x| a.attacker_name(x)).collect();
            let region = k.region.clone().unwrap_or_default();
            let search =
                [&ship, &victim, &org, &k.system, &region, &yours, &class, &group, &attackers.join(" ")].map(|s| s.to_lowercase()).join("\n");
            Some(MailRow {
                e: e.clone(),
                ship_id: v.ship_type_id,
                ship,
                victim,
                org,
                system: k.system.clone(),
                region,
                security: k.security,
                yours,
                search,
            })
        })
        .collect()
}

fn mail_list(
    ctx: &Ctx,
    archive: Arc<Archive>,
    entries: Vec<Entry>,
    names: Rc<HashMap<i64, String>>,
    mine: &HashSet<i64>,
    period: Period,
    chars: &[Arc<Character>],
) -> gtk::Widget {
    let rows = Rc::new(mail_rows(&archive, &entries, &names));
    let view = ctx.recall(LIST_KEY).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(3);
    let state = Rc::new(RefCell::new((view, ctx.recall(SEARCH_KEY).unwrap_or_default())));
    let holder = w::vbox(0);
    let multi = names.len() > 1;
    let refill = {
        let (ctx, holder, state, rows, archive, mine) = (ctx.clone(), holder.clone(), state.clone(), rows.clone(), archive.clone(), mine.clone());
        let when = if period == Period::All { "kept".to_string() } else { format!("in the last {}", period.label()) };
        Rc::new(move || {
            let (view, query) = state.borrow().clone();
            let q = query.trim().to_lowercase();
            let shown: Vec<MailRow> = rows
                .iter()
                .filter(|r| match view {
                    1 => !r.e.loss,
                    2 => r.e.loss,
                    3 => r.e.shared(),
                    _ => true,
                })
                .filter(|r| q.is_empty() || q.split_whitespace().all(|word| r.search.contains(word)))
                .cloned()
                .collect();
            let empty = if q.is_empty() { format!("No mails {when}.") } else { "Nothing matches.".to_string() };
            set_child(&holder, &mail_table(&ctx, archive.clone(), shown, &mine, multi, &empty));
        })
    };

    let count = |f: &dyn Fn(&MailRow) -> bool| rows.iter().filter(|r| f(r)).count();
    let labels = [
        format!("All ({})", rows.len()),
        format!("Kills ({})", count(&|r| !r.e.loss)),
        format!("Losses ({})", count(&|r| r.e.loss)),
        format!("Shared ({})", count(&|r| r.e.shared())),
    ];
    let labels: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let bar = w::hbox(12);
    bar.add_css_class("comms-filter-bar");
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        bar.append(&w::segmented(&labels, view, move |i| {
            ctx.remember(LIST_KEY, i.to_string());
            state.borrow_mut().0 = i;
            refill();
        }));
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let search = w::search_entry("Search ship, class, pilot, corporation, system, region, your character");
    search.set_text(&state.borrow().1);
    search.set_hexpand(false);
    search.set_max_width_chars(44);
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        search.connect_search_changed(move |s| {
            ctx.remember(SEARCH_KEY, s.text().as_str());
            state.borrow_mut().1 = s.text().to_string();
            refill();
        });
    }
    bar.append(&search);

    let section = w::vbox(8);
    section.append(&bar);
    section.append(&holder);
    refill();
    let actions: Vec<gtk::Widget> = match chars {
        [c] => {
            let open = w::button_icon("open_in_new", "Character kill log", Btn::Text);
            open.set_tooltip_text(Some("Open the character's own Kill Log tab"));
            let (ctx, id) = (ctx.clone(), c.id);
            open.connect_clicked(move |_| ctx.navigate(Route::Character(id, CharTab::Kills)));
            vec![open.upcast()]
        }
        _ => vec![],
    };
    let since = archive.oldest().map(|t| format!(" · archive goes back to {}", fmt::date(t))).unwrap_or_default();
    w::card_with("Killmails", Some(&format!("Double-click a mail for its fit and attackers{since}")), &actions, &section).upcast()
}

fn two_lines(a: &str, b: &str) -> gtk::Widget {
    let v = w::vbox(2);
    v.append(&w::line(a, &["body-medium"]));
    if !b.is_empty() {
        v.append(&w::line(b, &["body-small", "dim"]));
    }
    v.upcast()
}

fn mail_table(ctx: &Ctx, archive: Arc<Archive>, rows: Vec<MailRow>, mine: &HashSet<i64>, multi: bool, empty: &str) -> gtk::Widget {
    let mut t = Table::new(rows)
        .widget_sorted(
            "Ship",
            |r| {
                let row = w::hbox(8);
                row.append(&type_cell(r.ship_id, &r.ship));
                row.append(&if r.e.loss { w::status("Loss", Status::Bad) } else { w::status("Kill", Status::Good) });
                if r.e.solo {
                    row.append(&w::status("Solo", Status::Neutral));
                }
                row.upcast()
            },
            |a, b| a.ship.cmp(&b.ship),
        )
        .expand()
        .widget_sorted("Victim", |r| two_lines(&r.victim, &r.org), |a, b| a.victim.to_lowercase().cmp(&b.victim.to_lowercase()))
        .width(260);
    if multi {
        t = t
            .widget_sorted(
                "Yours",
                |r| {
                    let l = w::line(&r.yours, &["body-medium"]);
                    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    if r.e.shared() {
                        l.add_css_class("primary-text");
                        l.set_tooltip_text(Some(&format!("Shared: {}", r.yours)));
                    }
                    l.upcast()
                },
                |a, b| a.e.chars.len().cmp(&b.e.chars.len()).then(a.yours.cmp(&b.yours)),
            )
            .width(180);
    }
    let ctx2 = ctx.clone();
    let (archive2, mine2) = (archive.clone(), mine.clone());
    t.widget_sorted(
        "System",
        |r| {
            let row = w::hbox(8);
            if let Some(s) = r.security {
                row.append(&sec_label(s));
            }
            row.append(&two_lines(&r.system, &r.region));
            row.upcast()
        },
        |a, b| a.system.cmp(&b.system),
    )
    .width(190)
    .num("Value", |r| if r.e.valued { fmt::isk_short(r.e.value) } else { "—".into() }, |r| r.e.value)
    .widget_sorted(
        "When",
        |r| {
            let l = w::line(&r.e.time.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string(), &["body-medium", "dim", "numeric"]);
            l.set_tooltip_text(Some(&format!("{} EVE time", fmt::eve_time(r.e.time))));
            l.upcast()
        },
        |a, b| a.e.time.cmp(&b.e.time),
    )
    .width(160)
    .widget("", move |r| {
        let b = w::button("zKill", Btn::Text);
        b.set_tooltip_text(Some("Open on zKillboard"));
        let (ctx, url) = (ctx2.clone(), kb::zkillboard_url(r.e.id));
        b.connect_clicked(move |_| w::open_url(&ctx, &url));
        b.upcast()
    })
    .width(80)
    .sort_by(if multi { 5 } else { 4 }, true)
    .on_activate({
        let ctx = ctx.clone();
        move |r| detail::open(&ctx, archive2.clone(), r.e.id, &mine2)
    })
    .max_height(620)
    .empty_text(empty)
    .build()
}
