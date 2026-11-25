//! Shared by the Doctrines tabs.

/// Page styles (prefixed `doc-`, scheme roles only).
pub const CSS: &str = "";

// ---------- Pilots and Ships tabs ----------

use crate::ctx::Ctx;
use crate::pages::skill_browser::shared as browser;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::doctrines::eve_plan_text;
use podstack_core::features::fitting::{total_time, PlannedLevel};
use podstack_core::{config, fmt, Character};
use std::rc::Rc;
use std::sync::Arc;

/// Styles of the Pilots and Ships tabs (registered by their `build`).
pub const ROSTER_CSS: &str = "
.doc-toolbar { padding: 4px 0; }
.doc-pilot-card { padding: 20px; }
.doc-pilot-head { padding-bottom: 4px; }
.doc-section-label { color: @on_surface_variant; font-size: 12px; font-weight: 500; letter-spacing: 0.5px; margin-top: 4px; }
.doc-doctrine-path { color: @on_surface_variant; font-size: 12px; }
.doc-fit-chip { background-color: @surface_container_high; border-radius: 8px; padding: 4px 10px 4px 4px; }
.doc-fit-chip.ready { background-color: @secondary_container; color: @on_secondary_container; }
.doc-fit-chip image { border-radius: 6px; }
.doc-train-row { padding: 6px 0; }
.doc-matrix { padding: 4px 0 8px 0; }
.doc-matrix-head { padding: 0 6px 8px 6px; }
.doc-matrix-name { padding: 6px 12px 6px 0; }
.doc-matrix-cell { min-width: 88px; padding: 4px 6px; }
.doc-matrix-total { border-top: 1px solid @outline_variant; padding-top: 8px; margin-top: 4px; }
.doc-section-title { margin-top: 8px; }
button.doc-class-tile { background-color: @surface_container; color: @on_surface; border-radius: 20px; padding: 14px 16px; min-width: 220px; font-weight: 400; }
button.doc-class-tile:hover { background-color: mix(@surface_container, @on_surface, 0.08); }
button.doc-class-tile:active { background-color: mix(@surface_container, @on_surface, 0.12); }
.doc-class-tile .doc-class-count { font-size: 32px; font-weight: 400; color: @primary; }
button.doc-class-tile.nobody .doc-class-count { color: @on_surface_variant; }
.doc-class-tile .doc-class-render { border-radius: 14px; background-color: @surface_container_highest; }
.doc-hull-render { border-radius: 16px; background-color: @surface_container_highest; }
.doc-pilot-chip { background-color: @secondary_container; color: @on_secondary_container; border-radius: 999px; padding: 2px 12px 2px 2px; }
";

/// A round portrait and the character's name, in a pill.
pub fn pilot_chip(c: &Character) -> gtk::Widget {
    let b = w::hbox(8);
    b.add_css_class("doc-pilot-chip");
    b.append(&w::avatar(&config::character_portrait(c.id, 64), 24));
    let n = w::line(&c.name(), &["label-large"]);
    n.set_valign(gtk::Align::Center);
    // character names are short: show them whole
    n.set_ellipsize(gtk::pango::EllipsizeMode::None);
    b.append(&n);
    b.upcast()
}

/// A ship icon with a name next to it ("Ishtar · Sentry"), for fits and hulls.
pub fn fit_chip(ship_id: i64, label: &str, ready: bool, tooltip: Option<&str>) -> gtk::Widget {
    let b = w::hbox(8);
    b.add_css_class("doc-fit-chip");
    if ready {
        b.add_css_class("ready");
    }
    b.set_overflow(gtk::Overflow::Hidden);
    b.append(&w::eve_image(&config::type_icon(ship_id, 32), 24));
    let n = w::line(label, &["label-large"]);
    n.set_valign(gtk::Align::Center);
    n.set_max_width_chars(36);
    n.set_width_chars(label.chars().count().min(36) as i32);
    b.append(&n);
    b.set_tooltip_text(tooltip);
    b.upcast()
}

/// "Copy skill plan" (EVE's format) and "Add to skill plan" (opens [`plan_dialog`]) buttons. `levels` is worked
/// out on click: pages show many of these rows, and most are never clicked.
pub fn plan_buttons(ctx: &Ctx, c: &Arc<Character>, title: &str, levels: impl Fn(&Character) -> Vec<PlannedLevel> + 'static) -> gtk::Box {
    let levels = Rc::new(levels);
    let row = w::hbox(4);
    row.set_valign(gtk::Align::Center);
    let copy = w::icon_button("content_copy", "Copy skill plan (EVE's format, for the Skills window's plan import)");
    let (ctx2, c2, l2) = (ctx.clone(), c.clone(), levels.clone());
    copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx2, &eve_plan_text(&l2(&c2))));
    row.append(&copy);
    let add = w::icon_button("playlist_add", "Add to skill plan\u{2026}");
    let (ctx2, c2, title) = (ctx.clone(), c.clone(), title.to_string());
    add.connect_clicked(move |_| plan_dialog(&ctx2, &c2, &title, &levels(&c2)));
    row.append(&add);
    row
}

/// The levels a character has to train for something, to copy for EVE or add to one of its PodStack plans.
pub fn plan_dialog(ctx: &Ctx, c: &Character, title: &str, levels: &[PlannedLevel]) {
    plan_dialog_noted(ctx, c, title, levels, None, &[]);
}

/// [`plan_dialog`] with a line about the plan under its total, and a note under each level.
pub fn plan_dialog_noted(ctx: &Ctx, c: &Character, title: &str, levels: &[PlannedLevel], about: Option<&str>, notes: &[String]) {
    let (win, content, actions) = w::dialog(ctx, &format!("{title} \u{00b7} {}", c.name()), 520);
    let head = w::hbox(12);
    head.append(&w::avatar(&config::character_portrait(c.id, 64), 40));
    let n = levels.len();
    head.append(&w::text(
        &format!("{n} skill level{} \u{00b7} {}", if n == 1 { "" } else { "s" }, fmt::duration(total_time(levels), 3)),
        &["body-large"],
    ));
    content.append(&head);
    if let Some(a) = about {
        content.append(&w::text(a, &["body-small", "dim"]));
    }
    let list = gtk::ScrolledWindow::new();
    list.set_hscrollbar_policy(gtk::PolicyType::Never);
    list.set_propagate_natural_height(true);
    list.set_max_content_height(420);
    list.set_child(Some(&browser::levels_list_noted(levels, notes)));
    content.append(&list);
    content.append(&browser::add_to_plan_row(ctx, c.id, title, Some(title.to_string()), levels));

    let copy = w::button_icon("content_copy", "Copy skill plan", Btn::Text);
    copy.set_tooltip_text(Some("EVE's skill plan format: paste it into the Skills window's plan import"));
    let (ctx2, text) = (ctx.clone(), eve_plan_text(levels));
    copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx2, &text));
    actions.append(&copy);
    let close = w::button("Close", Btn::Text);
    let w2 = win.clone();
    close.connect_clicked(move |_| w2.close());
    actions.append(&close);
    win.present();
}

/// "3 more", folding away the rest of a list.
pub fn more_expander(n: usize, rest: &impl IsA<gtk::Widget>) -> gtk::Expander {
    let exp = gtk::Expander::new(None);
    exp.set_label_widget(Some(&w::line(&format!("{n} more"), &["label-large", "dim"])));
    exp.set_margin_top(4);
    exp.set_child(Some(rest));
    exp
}

/// A muted note for characters whose skills haven't loaded yet.
pub fn unloaded_note(n: usize) -> Option<gtk::Widget> {
    (n > 0).then(|| {
        w::banner(
            "hourglass_empty",
            &if n == 1 {
                "1 character's skills haven't loaded yet: it's left out until they have.".to_string()
            } else {
                format!("{n} characters' skills haven't loaded yet: they're left out until they have.")
            },
            w::BannerKind::Info,
        )
        .upcast()
    })
}
