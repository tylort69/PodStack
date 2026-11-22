//! Planetary Industry: every character's colonies as cards, each with its extractors (and when they stop), what its
//! factories make and what's stored there. Colonies needing attention (expired extractors) come first.
//! The character tab (`pages/character/planets.rs`) shows the same view for one character.

use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::pages::industry::{char_cell, scope_failed};
use crate::widgets::{self as w, Status};
use chrono::Utc;
use gtk::prelude::*;
use podstack_core::domains::industry::{Colony, Extractor};
use podstack_core::features::industry::{self as ind, ExtractorState};
use podstack_core::{config, fmt, Character};
use std::sync::Arc;

/// Seconds between countdown updates.
const TICK: u32 = 10;
/// Stored goods listed before the rest folds away.
const STORED_SHOWN: usize = 4;

const CSS: &str = r#"
.pi-card { min-width: 320px; }
.pi-section { color: @on_surface_variant; font-size: 12px; font-weight: 500; letter-spacing: 0.5px; margin-top: 4px; }
.pi-extractor { background-color: @surface_container; border-radius: 12px; padding: 10px 12px; }
.pi-character { padding: 4px 8px; border-radius: 999px; min-height: 0; }
.pi-card expander title { color: @on_surface_variant; font-size: 12px; }
"#;

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    let chars = ctx.core().characters();
    let no_scope = chars.iter().filter(|c| scope_failed(c, "planets")).count();
    if no_scope > 0 {
        body.append(&w::banner(
            "lock",
            &format!("{no_scope} character(s) need to log in again with \"Read Planetary Colonies\" to show their colonies."),
            w::BannerKind::Notice,
        ));
    }
    body.append(&colonies_view(ctx, &chars, true));
    Page::new("Planetary Industry", body)
}

/// Extractors whose program has ended, across all characters.
pub fn nav_badge(core: &podstack_core::Core) -> Option<u32> {
    Some(ind::count_expired_extractors(&core.characters(), Utc::now()) as u32)
}

/// Stat tiles and colony cards for `characters`.
pub(crate) fn colonies_view(ctx: &Ctx, characters: &[Arc<Character>], show_character: bool) -> gtk::Box {
    crate::theme::add_css(CSS);
    let now = Utc::now();
    let colonies = ind::colonies(characters, now);
    let counts = ind::extractor_counts(colonies.iter().map(|(_, c)| *c), now);
    let next = colonies.iter().filter_map(|(_, c)| ind::colony_next_expiry(c, now)).min();

    let view = w::vbox(16);
    let expired = w::stat_tile(
        "Extractors expired",
        &fmt::int(counts.expired as i64),
        Some(if counts.expired > 0 { "Restart them in-game" } else { "All running" }),
    );
    if counts.expired > 0 {
        expired.add_css_class("accent");
    }
    let next_tile = w::stat_tile("Next expiry", &next.map(fmt::time_until).unwrap_or_else(|| "—".into()), next.map(fmt::datetime).as_deref());
    if let (Some(end), Some(value)) = (next, next_tile.first_child().and_then(|c| c.next_sibling()).and_then(|c| c.downcast::<gtk::Label>().ok())) {
        let weak = value.downgrade();
        w::ticker(&value, 1, move || {
            if let Some(l) = weak.upgrade() {
                l.set_text(&if end > Utc::now() { fmt::time_until(end) } else { "Expired".into() });
            }
        });
    }
    let idle = if counts.idle > 0 { format!("{} idle", counts.idle) } else { format!("{} in total", counts.total) };
    view.append(&w::tile_row(vec![
        w::stat_tile("Colonies", &fmt::int(colonies.len() as i64), None),
        w::stat_tile("Extractors running", &fmt::int(counts.running as i64), Some(&idle)),
        expired,
        next_tile,
    ]));

    if colonies.is_empty() {
        view.append(&w::empty_state("public", "No planetary colonies", "Colonies set up in-game show up here, with their extractors and stored goods."));
        return view;
    }
    let cards: Vec<gtk::Widget> = colonies.iter().map(|(c, col)| colony_card(ctx, c, col, show_character).upcast()).collect();
    let flow = w::flow(cards, 3);
    flow.set_homogeneous(false);
    view.append(&flow);
    view
}

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

fn colony_card(ctx: &Ctx, character: &Character, colony: &Colony, show_character: bool) -> gtk::Box {
    let now = Utc::now();
    let card = w::vbox(12);
    card.add_css_class("card");
    card.add_css_class("pi-card");
    card.set_valign(gtk::Align::Start);

    // header: planet picture, name, details, overall state
    let head = w::hbox(12);
    match planet_type_id(&colony.planet_type) {
        Some(t) => {
            let img = w::eve_image(&config::type_icon(t, 64), 48);
            img.add_css_class("rounded");
            head.append(&img);
        }
        None => head.append(&w::icon_sized("public", 48)),
    }
    let titles = w::vbox(2);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    titles.append(&w::line(&colony.name(), &["title-medium"]));
    let mut details = vec![capitalize(&colony.planet_type)];
    if let Some(s) = &colony.system_name {
        details.push(s.clone());
    }
    titles.append(&w::line(&details.join(" · "), &["body-medium", "dim"]));
    head.append(&titles);
    let n = ind::extractor_counts([colony], now);
    let state = if n.expired > 0 {
        w::status("Needs restart", Status::Bad)
    } else if n.idle > 0 {
        w::status("Idle extractor", Status::Warn)
    } else if n.running > 0 {
        w::status("Running", Status::Good)
    } else {
        w::status("No extractors", Status::Neutral)
    };
    state.set_valign(gtk::Align::Start);
    head.append(&state);
    card.append(&head);

    // facts
    let facts = w::hbox(8);
    facts.append(&w::status(&format!("CC level {}", colony.upgrade_level), Status::Neutral));
    facts.append(&w::status(&format!("{} structures", colony.num_pins), Status::Neutral));
    if !colony.factories.is_empty() {
        facts.append(&w::status(&format!("{} factories", colony.factories.len()), Status::Neutral));
    }
    card.append(&facts);

    if show_character {
        let who = gtk::Button::new();
        who.add_css_class("pi-character");
        who.add_css_class("text-button");
        who.set_halign(gtk::Align::Start);
        who.set_child(Some(&char_cell(character.id, &character.name())));
        who.set_tooltip_text(Some("Open this character's PI tab"));
        let ctx = ctx.clone();
        let id = character.id;
        who.connect_clicked(move |_| ctx.navigate(Route::Character(id, CharTab::Planets)));
        card.append(&who);
    }

    // extractors, refreshed by a ticker so the countdowns move
    card.append(&w::line("EXTRACTORS", &["pi-section"]));
    if colony.extractors.is_empty() {
        card.append(&w::line("None", &["body-medium", "dim"]));
    }
    for e in &colony.extractors {
        card.append(&extractor_row(e.clone()));
    }

    if !colony.factories.is_empty() {
        card.append(&w::line("PRODUCING", &["pi-section"]));
        let chips = gtk::FlowBox::new();
        chips.set_selection_mode(gtk::SelectionMode::None);
        chips.set_column_spacing(6);
        chips.set_row_spacing(6);
        chips.set_max_children_per_line(4);
        chips.set_halign(gtk::Align::Start);
        for (name, count) in colony.factory_products() {
            let l = if count > 1 { format!("{name} ×{count}") } else { name };
            chips.append(&w::status(&l, Status::Info));
        }
        card.append(&chips);
    }

    card.append(&w::line("STORED", &["pi-section"]));
    if colony.storage.is_empty() {
        card.append(&w::line("Empty", &["body-medium", "dim"]));
    }
    for s in colony.storage.iter().take(STORED_SHOWN) {
        card.append(&stored_row(s.type_id, &s.label(), s.amount));
    }
    if colony.storage.len() > STORED_SHOWN {
        let rest = w::vbox(8);
        for s in colony.storage.iter().skip(STORED_SHOWN) {
            rest.append(&stored_row(s.type_id, &s.label(), s.amount));
        }
        let exp = gtk::Expander::new(Some(&format!("{} more", colony.storage.len() - STORED_SHOWN)));
        exp.set_child(Some(&rest));
        card.append(&exp);
    }

    let foot = w::line(&format!("Updated in-game {}", fmt::relative(colony.last_update)), &["body-small", "dim"]);
    foot.set_tooltip_text(Some(&fmt::datetime(colony.last_update)));
    card.append(&foot);
    card
}

fn stored_row(type_id: i64, name: &str, amount: i64) -> gtk::Box {
    let row = w::hbox(10);
    let img = w::eve_image(&config::type_icon(type_id, 64), 24);
    row.append(&img);
    let n = w::line(name, &["body-medium"]);
    n.set_hexpand(true);
    row.append(&n);
    row.append(&w::line(&format!("×{}", fmt::int(amount)), &["body-medium", "dim", "numeric"]));
    row
}

fn extractor_row(e: Extractor) -> gtk::Box {
    let b = w::vbox(6);
    b.add_css_class("pi-extractor");
    let head = w::hbox(10);
    if let Some(t) = e.product_type_id {
        head.append(&w::eve_image(&config::type_icon(t, 64), 24));
    } else {
        head.append(&w::icon_sized("precision_manufacturing", 20));
    }
    let name = w::line(e.product_name.as_deref().unwrap_or("Not set up"), &["body-medium"]);
    name.set_hexpand(true);
    head.append(&name);
    let right = w::hbox(0);
    head.append(&right);
    b.append(&head);
    let bar = w::progress(0.0);
    b.append(&bar);
    // an extractor without a program has no meaningful rate or heads
    let mut facts = vec![];
    if e.expiry_time.is_some() {
        if let Some(h) = ind::extractor_per_hour(&e) {
            facts.push(format!("~{}/h", fmt::int(h.round() as i64)));
        }
        if e.heads > 0 {
            facts.push(format!("{} heads", e.heads));
        }
    }
    if let Some(t) = e.expiry_time {
        facts.push(format!("ends {}", fmt::datetime(t)));
    }
    if !facts.is_empty() {
        b.append(&w::line(&facts.join(" · "), &["body-small", "dim"]));
    }

    let update = {
        let right = right.downgrade();
        let bar = bar.downgrade();
        move || {
            let (Some(right), Some(bar)) = (right.upgrade(), bar.upgrade()) else { return };
            let now = Utc::now();
            while let Some(c) = right.first_child() {
                right.remove(&c);
            }
            bar.remove_css_class("error");
            match ind::extractor_state(&e, now) {
                ExtractorState::Idle => right.append(&w::status("Idle", Status::Warn)),
                ExtractorState::Expired => {
                    right.append(&w::status("Expired", Status::Bad));
                    bar.add_css_class("error");
                }
                ExtractorState::Running => {
                    // not ellipsized: the countdown matters more than the product name next to it
                    let left = e.expiry_time.unwrap_or(now) - now;
                    right.append(&w::text(&fmt::duration(left, 2), &["body-medium", "numeric"]));
                }
            }
            bar.set_fraction(ind::extractor_progress(&e, now));
        }
    };
    update();
    w::ticker(&b, TICK, update);
    b
}
