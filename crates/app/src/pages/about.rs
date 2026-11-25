//! About PodStack: what it is, where it came from, its license and credits, how to support it, and Tranquility's
//! server status (public ESI `status`). [`server_status_widget`] is a compact status line other places can embed.

use crate::ctx::{Ctx, Page};
use crate::rt;
use crate::widgets::{self as w, Btn};
use chrono::{DateTime, Duration, Timelike, Utc};
use gtk::prelude::*;
use podstack_core::{config, esi::EsiError, fmt};
use serde::Deserialize;
use std::rc::Rc;

const REPOSITORY: &str = "https://github.com/tylort69/PodStack";
const ORIGINAL_PROJECT: &str = "https://github.com/PrometheusSatyen/Cerebral";
const LICENSE_URL: &str = "https://www.gnu.org/licenses/agpl-3.0.html";
/// In-game characters that receive ISK donations, with what each did for PodStack.
const DONATION_CHARACTERS: &[(&str, &str)] = &[
    ("1234nin4321", "Made the original PodStack for Windows"),
    ("Alaheem Jihad", "Made the Linux build and turned PodStack into the full-featured app it is now"),
];
/// Daily downtime, EVE time (UTC).
const DOWNTIME_HOUR: u32 = 11;
/// ESI caches the status for 30 seconds; once a minute is plenty.
const POLL_SECS: u32 = 60;

const CSS: &str = r#"
.about-hero { background-color: @primary_container; color: @on_primary_container; border-radius: 28px; padding: 28px 32px; }
.about-hero .dim { color: alpha(@on_primary_container, 0.8); }
.about-hero button.text-button { color: @on_primary_container; }
.about-hero button.text-button:hover { background-color: alpha(@on_primary_container, 0.08); }
.about-logo { border-radius: 24px; }
.about-highlight .icon { color: @primary; }
.about-dot { min-width: 10px; min-height: 10px; border-radius: 999px; background-color: @outline; }
.about-dot.good { background-color: @success; }
.about-dot.warn { background-color: @warning; }
.about-dot.bad { background-color: @error; }
.about-donate { background-color: @surface_container_high; border-radius: 16px; padding: 12px 16px; }
.about-status-compact { padding: 6px 16px; }
"#;

const HIGHLIGHTS: &[(&str, &str, &str)] = &[
    ("hub", "Fleet-wide command", "Every capsuleer you fly on one screen: skill points, wallets, clones, queues and contracts, refreshed straight from New Eden."),
    ("event_note", "Precision skill planning", "Stack plans by priority, merge them, sort for the fastest train and let the remap optimiser squeeze every last SP per hour out of your attributes."),
    ("rocket_launch", "Fit-to-fly in one paste", "Drop in an EFT, Pyfa, XML or DNA fit and get a plan that puts you in the hull first and unlocks the rest of the fit fastest."),
    ("monitor_heart", "Zero idle queues", "Queue Health and desktop alerts watch every account for paused training, short queues and ready injectors."),
];

const HERITAGE: &[&str] = &[
    "Rewritten as a native app in Rust with GTK 4 and Material You, for Linux (built on Arch and Hyprland) and Windows: colors from your wallpaper or accent color, desktop notifications and a tray icon.",
    "ESI's new versioning: requests carry a compatibility date and tenant, so CCP's API changes don't silently break your data.",
    "A modern EVE SSO login with PKCE, and your own EVE application if you like, caught automatically on a loopback callback.",
    "Skill planning, fit planning, skillbook pricing, implant profiles, SP farming and queue health on top of the original's character overview.",
];

const CREDITS: &[(&str, &str)] = &[
    ("Cerebral", "the original EVE Online character manager by PrometheusSatyen and contributors, which PodStack is a fork of"),
    ("CCP Games", "EVE Online, the EVE Swagger Interface, the static data export and the image server"),
    ("GTK and gtk-rs", "the toolkit and its Rust bindings"),
    ("Material Design 3", "the design system, Material Icons and material-color-utilities (Apache 2.0)"),
    ("matugen", "Material You colors from your wallpaper"),
    ("Inter", "the interface font by Rasmus Andersson (SIL Open Font License)"),
];

#[derive(Clone, Debug, Deserialize)]
struct ServerStatus {
    players: i64,
    #[serde(default)]
    server_version: Option<String>,
    #[serde(default)]
    start_time: Option<DateTime<Utc>>,
    #[serde(default)]
    vip: Option<bool>,
}

#[derive(Clone, Debug)]
enum State {
    Loading,
    Up(ServerStatus),
    /// ESI answered with an error: the server is down (daily downtime) or ESI is having trouble.
    Down,
    Unreachable,
}

fn until_downtime(now: DateTime<Utc>) -> Duration {
    let today = now.date_naive().and_hms_opt(DOWNTIME_HOUR, 0, 0).expect("valid time").and_utc();
    let next = if today > now { today } else { today + Duration::days(1) };
    next - now
}

fn fetch(ctx: &Ctx, done: impl FnOnce(State) + 'static) {
    let core = ctx.core().clone();
    rt::run(async move { core.esi.get("status").json::<ServerStatus>().await }, move |r| {
        done(match r {
            Ok(s) => State::Up(s),
            Err(EsiError::Http { .. }) => State::Down,
            Err(_) => State::Unreachable,
        })
    });
}

fn dot(state: &State) -> &'static str {
    match state {
        State::Up(s) if s.vip == Some(true) => "warn",
        State::Up(_) => "good",
        State::Down | State::Unreachable => "bad",
        State::Loading => "",
    }
}

fn label(state: &State) -> &'static str {
    match state {
        State::Loading => "Checking…",
        State::Up(s) if s.vip == Some(true) => "VIP only",
        State::Up(_) => "Online",
        State::Down => "Offline",
        State::Unreachable => "Unreachable",
    }
}

fn set_dot(d: &gtk::Box, state: &State) {
    d.set_css_classes(&["about-dot"]);
    let c = dot(state);
    if !c.is_empty() {
        d.add_css_class(c);
    }
}

/// Polls the status while `widget` is shown and calls `show` with each result.
fn poll(ctx: &Ctx, widget: &impl IsA<gtk::Widget>, show: impl Fn(&State) + 'static) {
    let show = Rc::new(show);
    let run = {
        let ctx = ctx.clone();
        let show = show.clone();
        move || {
            let show = show.clone();
            fetch(&ctx, move |s| show(&s));
        }
    };
    run();
    w::ticker(widget, POLL_SECS, run);
}

/// A compact Tranquility status line ("● Tranquility  Online · 23,456 pilots") for the navigation drawer.
pub fn server_status_widget(ctx: &Ctx) -> gtk::Widget {
    crate::theme::add_css(CSS);
    let row = w::hbox(8);
    row.add_css_class("about-status-compact");
    let d = w::hbox(0);
    d.add_css_class("about-dot");
    d.set_valign(gtk::Align::Center);
    row.append(&d);
    row.append(&w::line("Tranquility", &["label-large"]));
    let text = w::line(label(&State::Loading), &["label-medium", "dim", "numeric"]);
    text.set_hexpand(true);
    row.append(&text);
    let row2 = row.clone();
    poll(ctx, &row, move |s| {
        set_dot(&d, s);
        let t = match s {
            State::Up(st) => format!("{} · {}", label(s), fmt::int(st.players)),
            State::Down => format!("Offline · DT is {DOWNTIME_HOUR}:00 EVE"),
            _ => label(s).to_string(),
        };
        text.set_text(&t);
        let tip = match s {
            State::Up(st) => Some(format!("Tranquility {}", st.server_version.as_deref().unwrap_or(""))),
            State::Down => Some("Tranquility is down (daily downtime starts at 11:00 EVE time)".to_string()),
            State::Unreachable => Some("Couldn't reach EVE's API".to_string()),
            State::Loading => None,
        };
        row2.set_tooltip_text(tip.as_deref());
    });
    row.upcast()
}

fn status_card(ctx: &Ctx) -> gtk::Box {
    let state_dot = w::hbox(0);
    state_dot.add_css_class("about-dot");
    state_dot.set_valign(gtk::Align::Center);
    let state_label = w::line(label(&State::Loading), &["title-medium"]);
    let head = w::hbox(10);
    head.append(&state_dot);
    head.append(&state_label);

    let players = w::stat_tile("Pilots online", "—", None);
    let version = w::stat_tile("Server version", "—", None);
    let up = w::stat_tile("Up since", "—", None);
    let clock = w::stat_tile("EVE time", "—", None);
    let value_of = |tile: &gtk::Box| tile.first_child().and_then(|c| c.next_sibling()).and_downcast::<gtk::Label>().expect("stat value");
    let (pv, vv, uv, cv) = (value_of(&players), value_of(&version), value_of(&up), value_of(&clock));
    let tiles = w::tile_row(vec![players, version, up, clock]);
    tiles.set_max_children_per_line(4);

    let note = w::text("", &["body-medium", "dim"]);
    let body = w::vbox(16);
    body.append(&head);
    body.append(&tiles);
    body.append(&note);
    let card = w::card_with("Tranquility", Some("EVE Online's server, from ESI's public status."), &[], &body);

    let tick_clock = {
        let (cv, note) = (cv.clone(), note.clone());
        let core = ctx.core().clone();
        move || {
            let now = Utc::now();
            cv.set_text(&format!("{:02}:{:02}", now.hour(), now.minute()));
            let mut text = format!("Daily downtime is at {DOWNTIME_HOUR}:00 EVE time, in {}.", fmt::duration(until_downtime(now), 2));
            // the original app's "ESI asked us to slow down" banner
            if let Some(wait) = core.esi.status().paused_for {
                text.push_str(&format!(
                    " EVE's API asked PodStack to slow down; updates continue by themselves in {}.",
                    fmt::duration(Duration::seconds(wait.as_secs().max(1) as i64), 2)
                ));
            }
            note.set_text(&text);
        }
    };
    tick_clock();
    w::ticker(&card, 15, tick_clock);

    poll(ctx, &card, move |s| {
        set_dot(&state_dot, s);
        state_label.set_text(match s {
            State::Down => "Offline: downtime or ESI trouble",
            State::Unreachable => "Unreachable: check your connection",
            _ => label(s),
        });
        match s {
            State::Up(st) => {
                pv.set_text(&fmt::int(st.players));
                vv.set_text(st.server_version.as_deref().unwrap_or("—"));
                uv.set_text(&st.start_time.map(|t| format!("{} ago", fmt::duration(Utc::now() - t, 2))).unwrap_or_else(|| "—".into()));
            }
            _ => {
                for l in [&pv, &vv, &uv] {
                    l.set_text("—");
                }
            }
        }
    });
    card
}

fn link(ctx: &Ctx, icon: &str, text: &str, url: &'static str) -> gtk::Button {
    let b = w::button_icon(icon, text, Btn::Text);
    let ctx = ctx.clone();
    b.connect_clicked(move |_| w::open_url(&ctx, url));
    b
}

fn logo() -> gtk::Image {
    let pic = w::app_logo(96);
    pic.set_valign(gtk::Align::Start);
    pic.add_css_class("about-logo");
    pic
}

fn hero(ctx: &Ctx) -> gtk::Box {
    let hero = w::hbox(28);
    hero.add_css_class("about-hero");
    hero.append(&logo());
    let col = w::vbox(6);
    col.set_hexpand(true);
    col.append(&w::text(config::APP_NAME, &["display-small"]));
    col.append(&w::text(&format!("Version {} · native {} edition", config::VERSION, if cfg!(windows) { "Windows" } else { "Linux" }), &["title-medium", "dim"]));
    let tagline = w::text(
        "The capsuleer command suite for pilots who run more than one clone. PodStack plugs straight into the EVE \
         Swagger Interface for real-time awareness across your whole roster: skill queues, SP, ISK, contracts and \
         mail, so you spend less time alt-tabbing and more time undocking.",
        &["body-large"],
    );
    tagline.set_margin_top(8);
    tagline.set_max_width_chars(90);
    col.append(&tagline);
    let links = w::hbox(4);
    links.set_margin_top(8);
    links.append(&link(ctx, "code", "Source code", REPOSITORY));
    links.append(&link(ctx, "new_releases", "Releases", "https://github.com/tylort69/PodStack/releases"));
    links.append(&link(ctx, "bug_report", "Report a bug", "https://github.com/tylort69/PodStack/issues"));
    links.append(&link(ctx, "history_edu", "Original project", ORIGINAL_PROJECT));
    col.append(&links);
    hero.append(&col);
    hero
}

fn heritage_card(ctx: &Ctx) -> gtk::Box {
    let body = w::vbox(10);
    body.append(&w::text(
        "PodStack is a fork of Cerebral, the open-source EVE Online character manager, whose last release was in early \
         2019. Since then EVE's login and APIs have moved on, so PodStack brought it back into service:",
        &["body-medium"],
    ));
    for item in HERITAGE {
        let row = w::hbox(10);
        let i = w::icon_sized("check", 18);
        i.add_css_class("primary-text");
        i.set_valign(gtk::Align::Start);
        row.append(&i);
        let t = w::text(item, &["body-medium"]);
        t.set_hexpand(true);
        row.append(&t);
        body.append(&row);
    }
    let original = link(ctx, "open_in_new", "Cerebral on GitHub", ORIGINAL_PROJECT);
    original.set_halign(gtk::Align::Start);
    body.append(&original);
    w::card_with("Heritage", None, &[], &body)
}

fn license_card(ctx: &Ctx) -> gtk::Box {
    let body = w::vbox(10);
    body.append(&w::text(
        "Like Cerebral, PodStack is free software: you can redistribute and change it under the GNU Affero General \
         Public License, version 3 (AGPL-3.0-only). It comes with no warranty. Fly safe, and thanks to everyone who \
         built the original.",
        &["body-medium"],
    ));
    let b = link(ctx, "gavel", "Read the license", LICENSE_URL);
    b.set_halign(gtk::Align::Start);
    body.append(&b);
    body.append(&w::text(
        "EVE Online and the EVE logo are the registered trademarks of CCP hf. All rights are reserved worldwide. \
         PodStack is a third-party tool and is not affiliated with or endorsed by CCP hf.",
        &["body-small", "dim"],
    ));
    w::card_with("License", Some("GNU AGPL v3"), &[], &body)
}

fn credits_card() -> gtk::Box {
    let body = w::vbox(8);
    for (name, what) in CREDITS {
        body.append(&w::markup(&format!("<b>{}</b>: {}", fmt::escape(name), fmt::escape(what)), &["body-medium"]));
    }
    w::card_with("Credits", None, &[], &body)
}

fn donate_card(ctx: &Ctx) -> gtk::Box {
    let body = w::vbox(12);
    body.append(&w::text(
        "PodStack is free and always will be. If it's saved you a few hours of spreadsheet wrangling (or an idle \
         queue), an ISK donation keeps its developers in ships and the updates coming.",
        &["body-medium"],
    ));
    body.append(&w::line("Send ISK in game to", &["label-medium", "dim"]));
    for (name, what) in DONATION_CHARACTERS {
        let boxed = w::hbox(16);
        boxed.add_css_class("about-donate");
        let names = w::vbox(2);
        names.set_hexpand(true);
        names.append(&w::line(name, &["title-large"]));
        let role = w::text(what, &["body-small", "dim"]);
        names.append(&role);
        boxed.append(&names);
        let copy = w::button_icon("content_copy", "Copy name", Btn::Tonal);
        copy.set_valign(gtk::Align::Center);
        {
            let ctx = ctx.clone();
            copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, name));
        }
        boxed.append(&copy);
        body.append(&boxed);
    }
    body.append(&w::text(
        "In EVE: search for a character, right-click their name and choose Give Money. Donations are gifts: they \
         don't unlock anything, and every PodStack feature stays free.",
        &["body-small", "dim"],
    ));
    w::card_with("Support PodStack", None, &[], &body)
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.set_spacing(20);
    body.append(&hero(ctx));

    let highlights: Vec<gtk::Widget> = HIGHLIGHTS
        .iter()
        .map(|(icon, title, text)| {
            let b = w::vbox(8);
            b.add_css_class("about-highlight");
            let i = w::icon_sized(icon, 36);
            i.set_halign(gtk::Align::Start);
            b.append(&i);
            b.append(&w::text(title, &["title-medium"]));
            b.append(&w::text(text, &["body-medium", "dim"]));
            b.set_vexpand(true);
            w::card(&b).upcast()
        })
        .collect();
    body.append(&w::flow(highlights, 4));

    body.append(&status_card(ctx));

    let columns = w::flow(vec![heritage_card(ctx).upcast(), donate_card(ctx).upcast()], 2);
    body.append(&columns);
    let columns = w::flow(vec![license_card(ctx).upcast(), credits_card().upcast()], 2);
    body.append(&columns);
    Page::new("About", body).static_page()
}
