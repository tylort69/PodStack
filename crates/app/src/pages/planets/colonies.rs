//! Planetary Industry → Colonies: headline numbers, each character's PI skills and free planets, and every colony as
//! a compact card (status and why, extractors with countdowns and rates, factories per schematic, storage fill,
//! exports and imports per hour, ISK per hour at hub prices). Filter by status, sort by urgency or output.
//! The character tab (`pages/character/planets.rs`) shows the same view for one character.

use super::common;
use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::pages::industry::form::{security_label, system_name};
use crate::pages::industry::scope_failed;
use crate::widgets::{self as w, Status, Table};
use chrono::{DateTime, Utc};
use gtk::prelude::*;
use podstack_core::domains::industry::{Colony, Extractor};
use podstack_core::features::industry::{self as ind, ExtractorState};
use podstack_core::features::planetary::{self as pl, ColonyReport, Health, PiCharacter, PiSettings, StorageFill};
use podstack_core::pi::{PinKind, PI};
use podstack_core::{config, fmt, Character};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// Seconds between countdown updates.
const TICK: u32 = 10;
/// Exports or imports listed on a card before the rest go into a "+N more".
const FLOWS_SHOWN: usize = 3;

/// Short column headers and full names for `pi::SKILLS`, in the same order (`PiCharacter::skills` follows it).
const SKILL_NAMES: [(&str, &str); 5] = [
    ("CCU", "Command Center Upgrades"),
    ("IC", "Interplanetary Consolidation"),
    ("PL", "Planetology"),
    ("APL", "Advanced Planetology"),
    ("RS", "Remote Sensing"),
];

const FILTERS: [Option<Health>; 5] = [None, Some(Health::Expired), Some(Health::Critical), Some(Health::Warning), Some(Health::Ok)];
const SORTS: [&str; 3] = ["Alerts first", "Output: high to low", "Output: low to high"];

const CSS: &str = r#"
.pi-card { min-width: 360px; }
.pi-section { color: @on_surface_variant; font-size: 11px; font-weight: 500; letter-spacing: 0.5px; margin-top: 2px; }
.pi-extractor { background-color: @surface_container; border-radius: 10px; padding: 6px 10px; }
.pi-storage { background-color: @surface_container; border-radius: 10px; padding: 6px 10px; }
.pi-character { padding: 2px 8px; border-radius: 999px; min-height: 0; }
.pi-value { font-weight: 500; }
"#;

thread_local! {
    /// Sell prices from the last load, per hub: rebuilds show values right away instead of waiting for the market.
    static PRICES: RefCell<Option<(String, HashMap<i64, f64>)>> = const { RefCell::new(None) };
}

fn cached_prices(hub: &str) -> Option<HashMap<i64, f64>> {
    PRICES.with(|p| p.borrow().as_ref().filter(|(h, _)| h == hub).map(|(_, m)| m.clone()))
}

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.append(&super::tab_bar(ctx, super::Tab::Colonies));
    let chars = ctx.core().characters();
    for b in banners(&chars) {
        body.append(&b);
    }
    body.append(&colonies_view(ctx, &chars, true));
    Page::new("Planetary Industry", body)
}

/// Notices for characters whose colonies can't be shown: the login lacks the scope, or they haven't loaded yet.
pub(super) fn banners(chars: &[Arc<Character>]) -> Vec<gtk::Box> {
    let no_scope = chars.iter().filter(|c| scope_failed(c, "planets")).count();
    let not_loaded = chars.iter().filter(|c| c.planets.is_none() && !scope_failed(c, "planets")).count();
    let mut out = Vec::new();
    if no_scope > 0 {
        out.push(w::banner(
            "lock",
            &format!("{no_scope} character(s) need to log in again with \"Read Planetary Colonies\" to show their colonies."),
            w::BannerKind::Notice,
        ));
    }
    if not_loaded > 0 {
        out.push(w::banner("hourglass_empty", &format!("{not_loaded} character(s) haven't loaded their colonies yet."), w::BannerKind::Info));
    }
    out
}

/// Extractors whose program has ended, across all characters.
#[allow(dead_code)] // the page's badge goes through planets::nav_badge
pub fn nav_badge(core: &podstack_core::Core) -> Option<u32> {
    Some(ind::count_expired_extractors(&core.characters(), Utc::now()) as u32)
}

/// One colony with its report.
struct Entry {
    character_id: i64,
    character_name: String,
    colony: Colony,
    report: ColonyReport,
}

/// The view's state that survives refilling the cards.
struct State {
    filter: Cell<usize>,
    sort: Cell<usize>,
    /// Only this character's colonies (None: everyone's).
    character: Cell<Option<i64>>,
    prices: RefCell<Option<HashMap<i64, f64>>>,
}

/// The stat tile's value label (its second child).
fn tile_value(tile: &gtk::Box) -> Option<gtk::Label> {
    tile.first_child().and_then(|c| c.next_sibling()).and_then(|c| c.downcast::<gtk::Label>().ok())
}

fn gross(entries: &[Entry], prices: &HashMap<i64, f64>) -> f64 {
    entries.iter().map(|e| e.report.value_per_hour(prices)).sum()
}

/// Stat tiles, characters' PI skills and colony cards for `characters`.
pub(crate) fn colonies_view(ctx: &Ctx, characters: &[Arc<Character>], show_character: bool) -> gtk::Box {
    crate::theme::add_css(common::CSS);
    crate::theme::add_css(CSS);
    let key = if show_character { "planets.colonies" } else { "planets.colonies.character_tab" };
    let now = Utc::now();
    let settings = common::settings();
    let hub = common::hub(&settings).0;
    let entries: Vec<Entry> = {
        let s = &settings;
        characters
            .iter()
            .flat_map(|c| {
                c.planets.iter().flat_map(|p| &p.colonies).map(move |col| Entry {
                    character_id: c.id,
                    character_name: c.name(),
                    colony: col.clone(),
                    report: pl::report(col, now, s),
                })
            })
            .collect()
    };
    let pis: Vec<PiCharacter> = characters.iter().map(|c| pl::character(c)).collect();

    let view = w::vbox(16);

    // ---- tiles
    let mut tiles = Vec::new();
    if show_character {
        let with = characters.iter().filter(|c| c.planets.as_ref().is_some_and(|p| !p.colonies.is_empty())).count();
        tiles.push(w::stat_tile("Characters", &fmt::int(characters.len() as i64), Some(&format!("{with} with colonies"))));
    }
    let used: usize = pis.iter().map(|p| p.planets).sum();
    let max: i64 = pis.iter().map(|p| p.max_planets).sum();
    let free = (max - used as i64).max(0);
    tiles.push(w::stat_tile(
        "Planets",
        &format!("{} / {}", fmt::int(used as i64), fmt::int(max)),
        Some(&if free > 0 { format!("{free} free slot{}", if free == 1 { "" } else { "s" }) } else { "Every slot used".into() }),
    ));
    let running: usize = entries.iter().map(|e| e.report.extractors_running).sum();
    let stopped: usize = entries.iter().map(|e| e.report.extractors_stopped).sum();
    let next = entries.iter().filter_map(|e| e.report.next_expiry).min();
    let ext_sub = match (stopped, next) {
        (0, Some(n)) => format!("Next stops in {}", fmt::duration(n - now, 2)),
        (0, None) => "None running".into(),
        (s, _) => format!("{s} stopped"),
    };
    let ext_tile = w::stat_tile("Extractors running", &fmt::int(running as i64), Some(&ext_sub));
    if stopped > 0 {
        ext_tile.add_css_class("accent");
    }
    tiles.push(ext_tile);
    let count = |h: Health| entries.iter().filter(|e| e.report.health == h).count();
    let alerts = entries.len() - count(Health::Ok);
    let alert_sub = if alerts == 0 {
        "Every colony is fine".to_string()
    } else {
        [(Health::Expired, "expired"), (Health::Critical, "critical"), (Health::Warning, "warning")]
            .iter()
            .filter_map(|(h, l)| {
                let n = count(*h);
                (n > 0).then(|| format!("{n} {l}"))
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let alert_tile = w::stat_tile("Alerts", &fmt::int(alerts as i64), Some(&alert_sub));
    if count(Health::Expired) + count(Health::Critical) > 0 {
        alert_tile.add_css_class("accent");
    }
    tiles.push(alert_tile);
    let cached = cached_prices(hub);
    let gross_tile = w::stat_tile(
        "Gross output / hour",
        &cached.as_ref().map(|p| fmt::isk_short(gross(&entries, p))).unwrap_or_else(|| "…".into()),
        Some(&format!("Exports at {hub} sell")),
    );
    gross_tile.set_tooltip_text(Some("What the colonies make and don't use themselves, at the lowest sell order: a theoretical maximum before taxes and hauling."));
    let gross_value = tile_value(&gross_tile);
    tiles.push(gross_tile);
    view.append(&w::tile_row(tiles));

    if !characters.is_empty() {
        view.append(&characters_table(ctx, characters, &pis, &entries, show_character));
    }

    if entries.is_empty() {
        view.append(&w::empty_state("public", "No planetary colonies", "Colonies set up in-game show up here, with their extractors, factories and storage."));
        return view;
    }

    // ---- filters
    let ids: Vec<i64> = characters.iter().map(|c| c.id).collect();
    let state = Rc::new(State {
        filter: Cell::new(ctx.recall(&format!("{key}.filter")).and_then(|v| v.parse().ok()).filter(|i| *i < FILTERS.len()).unwrap_or(0)),
        sort: Cell::new(ctx.recall(&format!("{key}.sort")).and_then(|v| v.parse().ok()).filter(|i| *i < SORTS.len()).unwrap_or(0)),
        character: Cell::new(ctx.recall(&format!("{key}.character")).and_then(|v| v.parse().ok()).filter(|id| ids.contains(id))),
        prices: RefCell::new(cached),
    });
    let entries = Rc::new(entries);
    let cards = w::vbox(0);
    let fill: Rc<dyn Fn()> = {
        let (ctx, cards, entries, state, settings) = (ctx.clone(), cards.clone(), entries.clone(), state.clone(), settings.clone());
        Rc::new(move || {
            while let Some(c) = cards.first_child() {
                cards.remove(&c);
            }
            let prices = state.prices.borrow();
            let filter = FILTERS[state.filter.get()];
            let who = state.character.get();
            let mut list: Vec<&Entry> =
                entries.iter().filter(|e| filter.is_none_or(|h| e.report.health == h)).filter(|e| who.is_none_or(|id| e.character_id == id)).collect();
            let value = |e: &Entry| prices.as_ref().map(|p| e.report.value_per_hour(p)).unwrap_or(0.0);
            match state.sort.get() {
                1 => list.sort_by(|a, b| value(b).total_cmp(&value(a)).then_with(|| alert_order(a, b))),
                2 => list.sort_by(|a, b| value(a).total_cmp(&value(b)).then_with(|| alert_order(a, b))),
                _ => list.sort_by(|a, b| alert_order(a, b)),
            }
            if list.is_empty() {
                cards.append(&w::empty_state("filter_alt_off", "No colonies match", "No colonies have this status right now."));
                return;
            }
            let widgets: Vec<gtk::Widget> = list.iter().map(|e| colony_card(&ctx, e, prices.as_ref(), &settings, show_character).upcast()).collect();
            let flow = w::flow(widgets, 4);
            flow.set_homogeneous(false);
            flow.set_min_children_per_line(1);
            cards.append(&flow);
        })
    };

    let bar = w::hbox(12);
    let chips = gtk::FlowBox::new();
    chips.set_selection_mode(gtk::SelectionMode::None);
    chips.set_column_spacing(8);
    chips.set_row_spacing(8);
    chips.set_max_children_per_line(FILTERS.len() as u32);
    chips.set_hexpand(true);
    chips.set_valign(gtk::Align::Center);
    let in_scope = |e: &&Entry| state.character.get().is_none_or(|id| e.character_id == id);
    let mut first: Option<gtk::ToggleButton> = None;
    for (i, f) in FILTERS.iter().enumerate() {
        let n = entries.iter().filter(in_scope).filter(|e| f.is_none_or(|h| e.report.health == h)).count();
        let chip = w::chip(&format!("{} ({n})", f.map(|h| h.label()).unwrap_or("All")), i == state.filter.get());
        if let Some(fc) = &first {
            chip.set_group(Some(fc));
        } else {
            first = Some(chip.clone());
        }
        let (ctx, state, fill) = (ctx.clone(), state.clone(), fill.clone());
        let k = format!("{key}.filter");
        chip.connect_toggled(move |c| {
            if c.is_active() && state.filter.get() != i {
                state.filter.set(i);
                ctx.remember(&k, i.to_string());
                fill();
            }
        });
        let child = gtk::FlowBoxChild::new();
        child.set_child(Some(&chip));
        child.set_focusable(false);
        chips.append(&child);
    }
    bar.append(&chips);
    if show_character && characters.len() > 1 {
        let mut names: Vec<(i64, String)> = characters.iter().filter(|c| c.planets.as_ref().is_some_and(|p| !p.colonies.is_empty())).map(|c| (c.id, c.name())).collect();
        names.sort_by_key(|n| n.1.to_lowercase());
        let mut items = vec!["All characters".to_string()];
        items.extend(names.iter().map(|n| n.1.clone()));
        let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
        let sel = state.character.get().and_then(|id| names.iter().position(|n| n.0 == id)).map(|i| i + 1).unwrap_or(0);
        let dd = w::dropdown(&refs, sel as u32);
        dd.set_tooltip_text(Some("Show one character's colonies"));
        let (ctx, state) = (ctx.clone(), state.clone());
        let k = format!("{key}.character");
        dd.connect_selected_notify(move |d| {
            let id = (d.selected() as usize).checked_sub(1).and_then(|i| names.get(i)).map(|n| n.0);
            state.character.set(id);
            ctx.remember(&k, id.map(|i| i.to_string()).unwrap_or_default());
            // the chip counts follow the character, so rebuild the whole page
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        });
        bar.append(&dd);
    }
    let sort = w::dropdown(&SORTS, state.sort.get() as u32);
    sort.set_tooltip_text(Some("Order of the colonies"));
    {
        let (ctx, state, fill) = (ctx.clone(), state.clone(), fill.clone());
        let k = format!("{key}.sort");
        sort.connect_selected_notify(move |d| {
            state.sort.set(d.selected() as usize);
            ctx.remember(&k, d.selected().to_string());
            fill();
        });
    }
    bar.append(&sort);
    view.append(&bar);
    fill();
    view.append(&cards);

    // ---- prices, then the output values
    let types: Vec<i64> = entries.iter().flat_map(|e| e.report.exports().map(|(t, _)| t)).collect();
    if !types.is_empty() {
        let (state, entries, fill) = (state.clone(), entries.clone(), fill.clone());
        let weak = cards.downgrade();
        common::load_prices(ctx, types, false, move |p| {
            let sell = p.sell();
            PRICES.with(|c| {
                let mut c = c.borrow_mut();
                let mut merged = c.take().filter(|(h, _)| *h == p.hub_name).map(|(_, m)| m).unwrap_or_default();
                merged.extend(sell.iter().map(|(k, v)| (*k, *v)));
                *c = Some((p.hub_name.clone(), merged));
            });
            if weak.upgrade().is_none() {
                return;
            }
            if let Some(l) = &gross_value {
                l.set_text(&fmt::isk_short(gross(&entries, &sell)));
            }
            let changed = state.prices.borrow().as_ref() != Some(&sell);
            if changed {
                *state.prices.borrow_mut() = Some(sell);
                fill();
            }
        });
    } else if let Some(l) = &gross_value {
        l.set_text("0");
    }
    view
}

/// Worst status first, then the extractor that stops soonest, then the fullest storage.
fn alert_order(a: &Entry, b: &Entry) -> std::cmp::Ordering {
    let fill = |e: &Entry| e.report.fullest().map(|f| f.fraction()).unwrap_or(0.0);
    a.report
        .health
        .cmp(&b.report.health)
        .then(a.report.next_expiry.unwrap_or(DateTime::<Utc>::MAX_UTC).cmp(&b.report.next_expiry.unwrap_or(DateTime::<Utc>::MAX_UTC)))
        .then(fill(b).total_cmp(&fill(a)))
        .then_with(|| a.colony.name().cmp(&b.colony.name()))
}

// ---------- characters ----------

#[derive(Clone)]
struct CharRow {
    id: i64,
    name: String,
    pi: PiCharacter,
    skills_known: bool,
    colonies_known: bool,
    /// Colonies per health, in `Health` order.
    health: [usize; 4],
}

fn health_index(h: Health) -> usize {
    match h {
        Health::Expired => 0,
        Health::Critical => 1,
        Health::Warning => 2,
        Health::Ok => 3,
    }
}

fn health_status(h: Health) -> Status {
    match h {
        Health::Expired | Health::Critical => Status::Bad,
        Health::Warning => Status::Warn,
        Health::Ok => Status::Good,
    }
}

fn characters_table(ctx: &Ctx, characters: &[Arc<Character>], pis: &[PiCharacter], entries: &[Entry], show_character: bool) -> gtk::Widget {
    let rows: Vec<CharRow> = characters
        .iter()
        .zip(pis)
        .map(|(c, pi)| {
            let mut health = [0; 4];
            for e in entries.iter().filter(|e| e.character_id == c.id) {
                health[health_index(e.report.health)] += 1;
            }
            CharRow { id: c.id, name: c.name(), pi: pi.clone(), skills_known: c.skills.is_some(), colonies_known: c.planets.is_some(), health }
        })
        .collect();
    let mut t = Table::new(rows)
        .widget_sorted(
            "Character",
            |r| {
                let b = w::hbox(8);
                b.append(&w::avatar(&config::character_portrait(r.id, 64), 24));
                let n = w::line(&r.name, &[]);
                n.set_ellipsize(gtk::pango::EllipsizeMode::End);
                b.append(&n);
                b.upcast()
            },
            |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        )
        .expand()
        .widget_sorted(
            "Planets",
            |r| {
                let b = w::vbox(0);
                b.set_valign(gtk::Align::Center);
                let max = if r.skills_known { r.pi.max_planets.to_string() } else { "?".into() };
                b.append(&w::line(&format!("{} / {max}", r.pi.planets), &["numeric"]));
                if r.skills_known {
                    let free = (r.pi.max_planets - r.pi.planets as i64).max(0);
                    b.append(&w::line(&if free > 0 { format!("{free} free") } else { "full".into() }, &["body-small", if free > 0 { "primary-text" } else { "dim" }]));
                }
                b.upcast()
            },
            |a, b| (a.pi.max_planets - a.pi.planets as i64).cmp(&(b.pi.max_planets - b.pi.planets as i64)),
        );
    for (i, (short, long)) in SKILL_NAMES.iter().enumerate() {
        t = t.widget_sorted(
            short,
            move |r| {
                if !r.skills_known {
                    return w::line("—", &["dim"]).upcast();
                }
                let lvl = r.pi.skills.get(i).map(|s| s.1).unwrap_or(0);
                let pips = w::skill_pips(lvl.clamp(0, 5) as u8, false, 0);
                pips.set_tooltip_text(Some(&format!("{long} {lvl}")));
                pips.upcast()
            },
            move |a, b| a.pi.skills.get(i).map(|s| s.1).cmp(&b.pi.skills.get(i).map(|s| s.1)),
        );
    }
    let ctx2 = ctx.clone();
    let table = t
        .widget_sorted(
            "Colonies",
            |r| {
                let b = w::hbox(4);
                if !r.colonies_known {
                    b.append(&w::line("Not loaded", &["dim"]));
                } else if r.health.iter().sum::<usize>() == 0 {
                    b.append(&w::line("None", &["dim"]));
                }
                for h in [Health::Expired, Health::Critical, Health::Warning, Health::Ok] {
                    let n = r.health[health_index(h)];
                    if n > 0 {
                        b.append(&w::status(&format!("{n} {}", h.label().to_lowercase()), health_status(h)));
                    }
                }
                b.upcast()
            },
            |a, b| {
                // most trouble first when sorted ascending
                let key = |r: &CharRow| (std::cmp::Reverse(r.health[0]), std::cmp::Reverse(r.health[1]), std::cmp::Reverse(r.health[2]));
                key(a).cmp(&key(b))
            },
        )
        .min_height(120)
        .max_height(320)
        .on_activate(move |r| {
            if show_character {
                ctx2.navigate(Route::Character(r.id, CharTab::Planets));
            }
        })
        .build();
    let b = w::vbox(6);
    b.append(&table);
    let legend: Vec<String> = SKILL_NAMES.iter().map(|(s, l)| format!("{s} {l}")).collect();
    let note = w::line(&legend.join(" · "), &["body-small", "dim"]);
    note.set_wrap(true);
    note.set_xalign(0.0);
    b.append(&note);
    b.upcast()
}

// ---------- colony cards ----------

/// Type id of a planet type, for its icon.
fn planet_type_id(kind: &str) -> Option<i64> {
    Some(match kind {
        "temperate" => 11,
        "ice" => 12,
        "gas" => 13,
        "oceanic" => 2014,
        "lava" => 2015,
        "barren" => 2016,
        "storm" => 2017,
        "plasma" => 2063,
        "shattered" => 30889,
        _ => return None,
    })
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn section(title: &str, extra: Option<(&str, &str)>) -> gtk::Box {
    let b = w::hbox(6);
    b.append(&w::line(title, &["pi-section"]));
    if let Some((t, class)) = extra {
        b.append(&w::line(t, &["body-small", class]));
    }
    b
}

fn colony_card(ctx: &Ctx, e: &Entry, prices: Option<&HashMap<i64, f64>>, s: &PiSettings, show_character: bool) -> gtk::Box {
    let colony = &e.colony;
    let r = &e.report;
    let card = w::vbox(8);
    card.add_css_class("card");
    card.add_css_class("pi-card");
    card.set_valign(gtk::Align::Start);

    // header: planet picture, name, kind · system · security · CC level, status
    let head = w::hbox(10);
    match planet_type_id(&colony.planet_type) {
        Some(t) => {
            let img = w::eve_image(&config::type_icon(t, 64), 40);
            img.add_css_class("rounded");
            head.append(&img);
        }
        None => head.append(&w::icon_sized("public", 40)),
    }
    let titles = w::vbox(2);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    let name = w::line(&colony.name(), &["title-medium"]);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    titles.append(&name);
    let facts = w::hbox(6);
    let kind = PI.kind(&colony.planet_type).map(|k| k.label.to_string()).unwrap_or_else(|| capitalize(&colony.planet_type));
    facts.append(&w::line(&kind, &["body-small", "dim"]));
    facts.append(&w::line("·", &["body-small", "dim"]));
    let sys = colony.system_name.clone().unwrap_or_else(|| system_name(colony.solar_system_id));
    let sys_l = w::line(&sys, &["body-small", "dim"]);
    sys_l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    facts.append(&sys_l);
    let sec = security_label(colony.solar_system_id);
    sec.add_css_class("body-small");
    facts.append(&sec);
    facts.append(&w::line("·", &["body-small", "dim"]));
    facts.append(&w::line(&format!("CC {}", r.level), &["body-small", "dim"]));
    titles.append(&facts);
    head.append(&titles);
    let state = w::status(r.health.label(), health_status(r.health));
    state.set_valign(gtk::Align::Start);
    head.append(&state);
    card.append(&head);

    if !r.reasons.is_empty() {
        let class = match r.health {
            Health::Expired | Health::Critical => "error-text",
            _ => "warning-text",
        };
        let why = w::line(&capitalize(&r.reasons.join(", ")), &["body-small", class]);
        why.set_wrap(true);
        why.set_xalign(0.0);
        card.append(&why);
    }

    // owner and value per hour
    let line = w::hbox(8);
    if show_character {
        let who = gtk::Button::new();
        who.add_css_class("pi-character");
        who.add_css_class("text-button");
        let b = w::hbox(6);
        b.append(&w::avatar(&config::character_portrait(e.character_id, 64), 20));
        let n = w::line(&e.character_name, &["body-medium"]);
        n.set_ellipsize(gtk::pango::EllipsizeMode::End);
        b.append(&n);
        who.set_child(Some(&b));
        who.set_tooltip_text(Some("Open this character's PI tab"));
        let ctx = ctx.clone();
        let id = e.character_id;
        who.connect_clicked(move |_| ctx.navigate(Route::Character(id, CharTab::Planets)));
        line.append(&who);
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    line.append(&spacer);
    let value = match prices {
        Some(p) => format!("{} ISK/h", fmt::isk_short(r.value_per_hour(p))),
        None => "… ISK/h".into(),
    };
    let v = w::line(&value, &["title-small", "primary-text", "pi-value", "numeric"]);
    v.set_tooltip_text(Some("Exports per hour at hub sell prices"));
    line.append(&v);
    card.append(&line);

    // extractors, with one ticker per card moving every countdown
    let mut timers: Vec<(gtk::Label, gtk::ProgressBar, Extractor)> = Vec::new();
    if !colony.extractors.is_empty() {
        let per_hour: f64 = r.extraction.values().sum();
        let extra = (per_hour > 0.0).then(|| format!("{}/h", fmt::int(per_hour.round() as i64)));
        card.append(&section("EXTRACTORS", extra.as_deref().map(|t| (t, "dim"))));
        for x in &colony.extractors {
            let (row, label, bar) = extractor_row(x);
            card.append(&row);
            timers.push((label, bar, x.clone()));
        }
    }

    if !r.factories.is_empty() {
        let idle = (r.idle_factories > 0).then(|| format!("{} idle", r.idle_factories));
        let total: usize = r.factories.values().sum();
        let head = section(&format!("FACTORIES · {total}"), idle.as_deref().map(|t| (t, "warning-text")));
        card.append(&head);
        let chips = gtk::FlowBox::new();
        chips.set_selection_mode(gtk::SelectionMode::None);
        chips.set_column_spacing(6);
        chips.set_row_spacing(6);
        chips.set_max_children_per_line(4);
        chips.set_halign(gtk::Align::Start);
        let mut by: Vec<(i64, usize)> = r.factories.iter().map(|(k, v)| (*k, *v)).collect();
        by.sort_by_key(|a| std::cmp::Reverse(a.1));
        for (sid, n) in by {
            let name = PI.schematics.get(&sid).map(|s| s.name.clone()).unwrap_or_else(|| format!("Schematic #{sid}"));
            let l = if n > 1 { format!("{name} ×{n}") } else { name };
            chips.append(&w::status(&l, Status::Info));
        }
        card.append(&chips);
    }

    let storage: Vec<&StorageFill> = r.storage.iter().filter(|f| f.kind != PinKind::CommandCenter || !f.contents.is_empty()).collect();
    if !storage.is_empty() {
        card.append(&section("STORAGE", None));
        for f in storage {
            card.append(&storage_row(f, s));
        }
    }

    let exports: Vec<(i64, f64)> = r.exports().collect();
    let imports: Vec<(i64, f64)> = r.imports().collect();
    if !exports.is_empty() || !imports.is_empty() {
        let flows = w::hbox(12);
        flows.set_homogeneous(true);
        flows.append(&flow_list("EXPORTS / H", &exports, "success-text"));
        flows.append(&flow_list("IMPORTS / H", &imports, "warning-text"));
        card.append(&flows);
    }

    let foot = w::line(&format!("Updated in-game {}", fmt::relative(colony.last_update)), &["body-small", "dim"]);
    foot.set_tooltip_text(Some(&fmt::datetime(colony.last_update)));
    card.append(&foot);

    if !timers.is_empty() {
        let update = move || {
            let now = Utc::now();
            for (l, bar, x) in &timers {
                set_extractor_state(l, bar, x, now);
            }
        };
        update();
        w::ticker(&card, TICK, update);
    }
    show_short_labels(card.upcast_ref());
    card
}

/// Short labels (section titles, numbers, countdowns, facts) show in full: in a wrapping FlowBox the cards can be
/// measured narrower than they end up, and these would ellipsize to "6,0…" despite the room. Labels that stretch
/// (names) keep ellipsizing.
fn show_short_labels(w: &gtk::Widget) {
    if let Some(l) = w.downcast_ref::<gtk::Label>() {
        if !l.hexpands() && !l.wraps() {
            l.set_ellipsize(gtk::pango::EllipsizeMode::None);
        }
    }
    let mut c = w.first_child();
    while let Some(child) = c {
        show_short_labels(&child);
        c = child.next_sibling();
    }
}

/// An extractor: product, rate, countdown and progress. Returns the countdown label and bar for the ticker.
fn extractor_row(x: &Extractor) -> (gtk::Box, gtk::Label, gtk::ProgressBar) {
    let b = w::vbox(4);
    b.add_css_class("pi-extractor");
    let head = w::hbox(8);
    match x.product_type_id {
        Some(t) => head.append(&w::eve_image(&config::type_icon(t, 64), 20)),
        None => head.append(&w::icon_sized("precision_manufacturing", 18)),
    }
    let name = w::line(x.product_name.as_deref().unwrap_or("Not set up"), &["body-medium"]);
    name.set_hexpand(true);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    head.append(&name);
    if x.expiry_time.is_some() {
        if let Some(h) = ind::extractor_per_hour(x) {
            let mut t = format!("~{}/h", fmt::int(h.round() as i64));
            if x.heads > 0 {
                t.push_str(&format!(" · {}h", x.heads));
            }
            let rate = w::line(&t, &["body-small", "dim", "numeric"]);
            rate.set_tooltip_text(Some("Average units per hour over the program, and extractor heads"));
            head.append(&rate);
        }
    }
    // not ellipsized: the countdown matters more than the product name next to it
    let left = w::line("", &["body-medium", "numeric"]);
    if let Some(t) = x.expiry_time {
        left.set_tooltip_text(Some(&format!("Ends {}", fmt::datetime(t))));
    }
    head.append(&left);
    b.append(&head);
    let bar = w::progress(0.0);
    b.append(&bar);
    (b, left, bar)
}

fn set_extractor_state(l: &gtk::Label, bar: &gtk::ProgressBar, x: &Extractor, now: DateTime<Utc>) {
    for c in ["error-text", "warning-text"] {
        l.remove_css_class(c);
    }
    bar.remove_css_class("error");
    match ind::extractor_state(x, now) {
        ExtractorState::Idle => {
            l.set_text("No program");
            l.add_css_class("warning-text");
        }
        ExtractorState::Expired => {
            l.set_text("Stopped");
            l.add_css_class("error-text");
            bar.add_css_class("error");
        }
        ExtractorState::Running => l.set_text(&fmt::duration(x.expiry_time.unwrap_or(now) - now, 2)),
    }
    bar.set_fraction(ind::extractor_progress(x, now));
}

/// A launchpad or storage facility: how full, and what fills it most.
fn storage_row(f: &StorageFill, s: &PiSettings) -> gtk::Box {
    let b = w::vbox(4);
    b.add_css_class("pi-storage");
    let head = w::hbox(8);
    let name = w::line(&pl::short_pin(&f.name), &["body-medium"]);
    name.set_hexpand(true);
    head.append(&name);
    let frac = f.fraction();
    let class = if frac >= s.storage_critical {
        "error-text"
    } else if frac >= s.storage_warning {
        "warning-text"
    } else {
        "dim"
    };
    head.append(&w::line(&format!("{:.0}%", frac * 100.0), &["body-small", "numeric", class]));
    head.set_tooltip_text(Some(&format!("{} of {} m³", fmt::int(f.used.round() as i64), fmt::int(f.capacity.round() as i64))));
    b.append(&head);
    let bar = w::progress(frac);
    if frac >= s.storage_critical {
        bar.add_css_class("error");
    } else if frac >= s.storage_warning {
        bar.add_css_class("warning");
    }
    b.append(&bar);
    if !f.contents.is_empty() {
        let top: Vec<String> = f.contents.iter().take(3).map(|(t, n)| format!("{} {}", PI.name(*t), fmt::isk_short(*n as f64))).collect();
        let mut text = top.join(" · ");
        if f.contents.len() > 3 {
            text.push_str(&format!(" · +{}", f.contents.len() - 3));
        }
        let l = w::line(&text, &["body-small", "dim"]);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let all: Vec<String> = f.contents.iter().map(|(t, n)| format!("{} × {}", fmt::int(*n), PI.name(*t))).collect();
        l.set_tooltip_text(Some(&all.join("\n")));
        b.append(&l);
    }
    b
}

/// Exports or imports per hour, a few shown and the rest in a tooltip.
fn flow_list(title: &str, items: &[(i64, f64)], class: &str) -> gtk::Box {
    let b = w::vbox(4);
    b.set_valign(gtk::Align::Start);
    b.append(&w::line(title, &["pi-section"]));
    if items.is_empty() {
        b.append(&w::line("None", &["body-small", "dim"]));
        return b;
    }
    let mut sorted = items.to_vec();
    sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (t, rate) in sorted.iter().take(FLOWS_SHOWN) {
        let row = w::hbox(6);
        row.append(&w::eve_image(&common::icon_url(*t), 16));
        let n = w::line(&PI.name(*t), &["body-small"]);
        n.set_hexpand(true);
        n.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&n);
        row.append(&w::line(&fmt::int(rate.round() as i64), &["body-small", "numeric", class]));
        b.append(&row);
    }
    if sorted.len() > FLOWS_SHOWN {
        let more = w::line(&format!("+{} more", sorted.len() - FLOWS_SHOWN), &["body-small", "dim"]);
        let all: Vec<String> = sorted.iter().map(|(t, r)| format!("{}: {}/h", PI.name(*t), fmt::int(r.round() as i64))).collect();
        more.set_tooltip_text(Some(&all.join("\n")));
        b.append(&more);
    }
    b
}
