//! Material 3 building blocks. Pages compose these instead of styling GTK widgets by hand, so the whole app looks
//! the same. Style classes are documented in `theme/css.rs`.

pub mod table;

pub use table::Table;

use crate::ctx::Ctx;
use gtk::{glib, prelude::*};

pub fn vbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Vertical, spacing)
}

pub fn hbox(spacing: i32) -> gtk::Box {
    gtk::Box::new(gtk::Orientation::Horizontal, spacing)
}

/// A page body: vertical box with the standard page padding.
pub fn page_body() -> gtk::Box {
    let b = vbox(16);
    b.set_margin_start(24);
    b.set_margin_end(24);
    b.set_margin_top(8);
    b.set_margin_bottom(24);
    b
}

/// A Material Icons glyph, e.g. `icon("dashboard")`.
pub fn icon(name: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(name));
    l.add_css_class("icon");
    l
}

pub fn icon_sized(name: &str, px: i32) -> gtk::Label {
    let l = icon(name);
    l.add_css_class(&format!("icon-{px}"));
    l
}

/// A label with style classes, e.g. `text("Skills", &["title-medium"])`. Wraps long text.
pub fn text(t: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(pango::WrapMode::WordChar);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A single-line label that ellipsizes.
pub fn line(t: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.set_xalign(0.0);
    l.set_ellipsize(pango::EllipsizeMode::End);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A label with Pango markup.
pub fn markup(m: &str, classes: &[&str]) -> gtk::Label {
    let l = text("", classes);
    l.set_markup(m);
    l
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Btn {
    Filled,
    Tonal,
    Outlined,
    Text,
    // Part of the M3 button set (documented in CLAUDE.md) though no page uses it yet.
    #[allow(dead_code)]
    Elevated,
    Danger,
    DangerFilled,
}

impl Btn {
    fn classes(self) -> &'static [&'static str] {
        match self {
            Btn::Filled => &["filled"],
            Btn::Tonal => &["tonal"],
            Btn::Outlined => &["outlined"],
            Btn::Text => &["text-button"],
            Btn::Elevated => &["elevated"],
            Btn::Danger => &["danger", "text-button"],
            Btn::DangerFilled => &["danger", "filled"],
        }
    }
}

pub fn button(label: &str, kind: Btn) -> gtk::Button {
    let b = gtk::Button::with_label(label);
    for c in kind.classes() {
        b.add_css_class(c);
    }
    b
}

/// A button with a leading icon.
pub fn button_icon(icon_name: &str, label: &str, kind: Btn) -> gtk::Button {
    let b = gtk::Button::new();
    let row = hbox(8);
    row.set_halign(gtk::Align::Center);
    row.append(&icon_sized(icon_name, 18));
    row.append(&gtk::Label::new(Some(label)));
    b.set_child(Some(&row));
    for c in kind.classes() {
        b.add_css_class(c);
    }
    if kind != Btn::Text {
        b.add_css_class("has-icon");
    }
    b
}

pub fn icon_button(icon_name: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&icon(icon_name)));
    b.add_css_class("icon-button");
    b.set_tooltip_text(Some(tooltip));
    b.set_valign(gtk::Align::Center);
    b
}

/// An elevated card.
pub fn card(child: &impl IsA<gtk::Widget>) -> gtk::Box {
    let c = vbox(0);
    c.add_css_class("card");
    c.append(child);
    c
}

/// A card with a title row (title, optional subtitle, trailing actions) above `body`.
pub fn card_with(title: &str, subtitle: Option<&str>, actions: &[gtk::Widget], body: &impl IsA<gtk::Widget>) -> gtk::Box {
    let c = vbox(8);
    c.add_css_class("card");
    let head = hbox(8);
    head.add_css_class("card-header");
    let titles = vbox(2);
    titles.set_hexpand(true);
    titles.append(&text(title, &["title-medium"]));
    if let Some(s) = subtitle {
        titles.append(&text(s, &["body-medium", "dim"]));
    }
    head.append(&titles);
    for a in actions {
        head.append(a);
    }
    c.append(&head);
    c.append(body);
    c
}

/// Label/value tile for headline numbers.
pub fn stat_tile(label: &str, value: &str, sub: Option<&str>) -> gtk::Box {
    let t = vbox(4);
    t.add_css_class("stat-tile");
    t.append(&line(label, &["stat-label"]));
    let v = line(value, &["stat-value", "numeric"]);
    t.append(&v);
    if let Some(s) = sub {
        t.append(&line(s, &["stat-sub"]));
    }
    t
}

/// A row of stat tiles that wraps on narrow windows.
pub fn tile_row(tiles: Vec<gtk::Box>) -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_column_spacing(12);
    f.set_row_spacing(12);
    f.set_homogeneous(true);
    f.set_max_children_per_line(6);
    f.set_min_children_per_line(1);
    for t in tiles {
        f.append(&t);
    }
    f
}

/// A wrapping grid of cards/widgets.
pub fn flow(children: Vec<gtk::Widget>, max_per_line: u32) -> gtk::FlowBox {
    let f = gtk::FlowBox::new();
    f.set_selection_mode(gtk::SelectionMode::None);
    f.set_column_spacing(16);
    f.set_row_spacing(16);
    f.set_homogeneous(true);
    f.set_max_children_per_line(max_per_line);
    f.set_valign(gtk::Align::Start);
    for c in children {
        f.append(&c);
    }
    f
}

/// Filter chip.
pub fn chip(label: &str, selected: bool) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::with_label(label);
    b.add_css_class("chip");
    b.set_active(selected);
    b
}

#[derive(Clone, Copy)]
pub enum Status {
    Good,
    Warn,
    Bad,
    Info,
    Neutral,
}

/// A small container-colored status pill, e.g. `status("Omega", Status::Good)`.
pub fn status(t: &str, kind: Status) -> gtk::Label {
    let l = gtk::Label::new(Some(t));
    l.add_css_class(match kind {
        Status::Good => "status-good",
        Status::Warn => "status-warn",
        Status::Bad => "status-bad",
        Status::Info => "status-info",
        Status::Neutral => "status-neutral",
    });
    l.set_valign(gtk::Align::Center);
    l.set_halign(gtk::Align::Start);
    l
}

pub fn badge(n: u32) -> gtk::Label {
    let l = gtk::Label::new(Some(&if n > 99 { "99+".to_string() } else { n.to_string() }));
    l.add_css_class("badge");
    l.set_valign(gtk::Align::Center);
    l
}

/// Big icon, title and explanation for empty pages and lists.
pub fn empty_state(icon_name: &str, title: &str, body: &str) -> gtk::Box {
    let b = vbox(8);
    b.add_css_class("empty-state");
    b.set_halign(gtk::Align::Center);
    b.set_valign(gtk::Align::Center);
    b.set_vexpand(true);
    let i = icon_sized(icon_name, 48);
    b.append(&i);
    let t = text(title, &["title-large"]);
    t.set_xalign(0.5);
    t.set_justify(gtk::Justification::Center);
    b.append(&t);
    let s = text(body, &["body-medium", "dim"]);
    s.set_xalign(0.5);
    s.set_justify(gtk::Justification::Center);
    s.set_max_width_chars(60);
    b.append(&s);
    b
}

#[derive(Clone, Copy)]
pub enum BannerKind {
    Notice,
    Info,
    Error,
}

pub fn banner(icon_name: &str, t: &str, kind: BannerKind) -> gtk::Box {
    let b = hbox(12);
    b.add_css_class("banner");
    match kind {
        BannerKind::Info => b.add_css_class("info"),
        BannerKind::Error => b.add_css_class("error"),
        BannerKind::Notice => {}
    }
    b.append(&icon_sized(icon_name, 20));
    let l = text(t, &["body-medium"]);
    l.set_hexpand(true);
    b.append(&l);
    b
}

/// Shown where data needs a scope the character's login lacks.
pub fn scope_notice(scope: &str) -> gtk::Box {
    banner(
        "lock",
        &format!("This character's login doesn't include {scope}. Log in again with \"Add character\" to grant it."),
        BannerKind::Notice,
    )
}

pub fn progress(fraction: f64) -> gtk::ProgressBar {
    let p = gtk::ProgressBar::new();
    p.set_fraction(fraction.clamp(0.0, 1.0));
    p.set_valign(gtk::Align::Center);
    p
}

/// Five skill level pips: `trained` full, `partial` (the next level partly trained), up to `queued` planned.
pub fn skill_pips(trained: u8, partial: bool, queued: u8) -> gtk::Box {
    let b = hbox(3);
    b.set_valign(gtk::Align::Center);
    for lvl in 1..=5u8 {
        let p = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        p.add_css_class("skill-pip");
        if lvl <= trained {
            p.add_css_class("trained");
        } else if partial && lvl == trained + 1 {
            p.add_css_class("partial");
        } else if lvl <= queued {
            p.add_css_class("queued");
        }
        b.append(&p);
    }
    b
}

pub fn search_entry(placeholder: &str) -> gtk::SearchEntry {
    let s = gtk::SearchEntry::new();
    s.set_placeholder_text(Some(placeholder));
    s.set_hexpand(true);
    s
}

pub fn dropdown(items: &[&str], selected: u32) -> gtk::DropDown {
    let d = gtk::DropDown::from_strings(items);
    d.set_selected(selected);
    d.set_valign(gtk::Align::Center);
    d
}

/// Connected toggle buttons (one selected); `on_change` gets the new index.
pub fn segmented(labels: &[&str], selected: usize, on_change: impl Fn(usize) + 'static) -> gtk::Box {
    let b = hbox(0);
    b.add_css_class("segmented");
    b.set_halign(gtk::Align::Start);
    let on_change = std::rc::Rc::new(on_change);
    let mut first: Option<gtk::ToggleButton> = None;
    for (i, l) in labels.iter().enumerate() {
        let t = gtk::ToggleButton::with_label(l);
        if let Some(f) = &first {
            t.set_group(Some(f));
        } else {
            first = Some(t.clone());
        }
        t.set_active(i == selected);
        let cb = on_change.clone();
        t.connect_toggled(move |t| {
            if t.is_active() {
                cb(i);
            }
        });
        b.append(&t);
    }
    b
}

/// A labelled switch row for settings.
pub fn switch_row(title: &str, subtitle: Option<&str>, active: bool, on_change: impl Fn(bool) + 'static) -> gtk::Box {
    let row = hbox(16);
    row.add_css_class("list-item");
    let texts = vbox(2);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    texts.append(&text(title, &["body-large"]));
    if let Some(s) = subtitle {
        texts.append(&text(s, &["body-medium", "dim"]));
    }
    row.append(&texts);
    let sw = gtk::Switch::new();
    sw.set_active(active);
    sw.set_valign(gtk::Align::Center);
    sw.connect_active_notify(move |s| on_change(s.is_active()));
    row.append(&sw);
    row
}

/// A two-line list row: leading widget, headline, supporting text, trailing widget.
pub fn list_row(leading: Option<&gtk::Widget>, headline: &str, supporting: Option<&str>, trailing: Option<&gtk::Widget>) -> gtk::Box {
    let row = hbox(16);
    row.add_css_class("list-item");
    if let Some(l) = leading {
        l.set_valign(gtk::Align::Center);
        row.append(l);
    }
    let texts = vbox(2);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    texts.append(&line(headline, &["headline"]));
    if let Some(s) = supporting {
        texts.append(&line(s, &["supporting"]));
    }
    row.append(&texts);
    if let Some(t) = trailing {
        t.set_valign(gtk::Align::Center);
        row.append(t);
    }
    row
}

/// Key/value rows (e.g. "Security status  5.0").
pub fn key_values(rows: &[(&str, String)]) -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_column_spacing(24);
    g.set_row_spacing(8);
    for (i, (k, v)) in rows.iter().enumerate() {
        let kl = line(k, &["body-medium", "dim"]);
        g.attach(&kl, 0, i as i32, 1, 1);
        let vl = text(v, &["body-medium"]);
        vl.set_selectable(true);
        vl.set_hexpand(true);
        g.attach(&vl, 1, i as i32, 1, 1);
    }
    g
}

/// Calls `f` every `secs` seconds while `widget` is shown (for countdowns).
pub fn ticker(widget: &impl IsA<gtk::Widget>, secs: u32, f: impl Fn() + 'static) {
    let weak = widget.as_ref().downgrade();
    glib::timeout_add_seconds_local(secs, move || match weak.upgrade() {
        Some(w) if w.root().is_some() => {
            if w.is_mapped() {
                f();
            }
            glib::ControlFlow::Continue
        }
        Some(_) => glib::ControlFlow::Continue,
        None => glib::ControlFlow::Break,
    });
}

/// A Material dialog: title, content and action buttons. Returns (window, content box, actions box).
pub fn dialog(ctx: &Ctx, title: &str, width: i32) -> (gtk::Window, gtk::Box, gtk::Box) {
    let win = gtk::Window::new();
    win.set_transient_for(Some(&ctx.window()));
    win.set_modal(true);
    win.set_title(Some(title));
    win.set_default_width(width);
    win.set_resizable(true);
    win.set_decorated(false);
    win.add_css_class("m3-dialog");
    let surface = vbox(16);
    surface.add_css_class("dialog-surface");
    surface.append(&text(title, &["dialog-title"]));
    let content = vbox(12);
    content.set_vexpand(true);
    surface.append(&content);
    let actions = hbox(8);
    actions.add_css_class("dialog-actions");
    actions.set_halign(gtk::Align::End);
    surface.append(&actions);
    win.set_child(Some(&surface));
    let key = gtk::EventControllerKey::new();
    let w = win.clone();
    key.connect_key_pressed(move |_, k, _, _| {
        if k == gtk::gdk::Key::Escape {
            w.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    win.add_controller(key);
    (win, content, actions)
}

/// Asks to confirm; `on_yes` runs when confirmed.
pub fn confirm(ctx: &Ctx, title: &str, body: &str, yes_label: &str, danger: bool, on_yes: impl Fn() + 'static) {
    let (win, content, actions) = dialog(ctx, title, 420);
    content.append(&text(body, &["body-medium", "dim"]));
    let cancel = button("Cancel", Btn::Text);
    let yes = button(yes_label, if danger { Btn::DangerFilled } else { Btn::Filled });
    let w = win.clone();
    cancel.connect_clicked(move |_| w.close());
    let w = win.clone();
    yes.connect_clicked(move |_| {
        on_yes();
        w.close();
    });
    actions.append(&cancel);
    actions.append(&yes);
    win.present();
}

/// A vertical scrolled container that fills its parent.
pub fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_hscrollbar_policy(gtk::PolicyType::Never);
    s.set_vexpand(true);
    s.set_child(Some(child));
    s
}

/// Puts text on the clipboard.
pub fn copy_to_clipboard(ctx: &Ctx, t: &str) {
    ctx.window().clipboard().set_text(t);
    ctx.toast("Copied to clipboard");
}

/// Opens a URL in the default browser.
pub fn open_url(ctx: &Ctx, url: &str) {
    let launcher = gtk::UriLauncher::new(url);
    let url_s = url.to_string();
    launcher.launch(Some(&ctx.window()), gtk::gio::Cancellable::NONE, move |r| {
        if r.is_err() {
            let _ = std::process::Command::new("xdg-open").arg(&url_s).spawn();
        }
    });
}

/// A portrait/logo/icon from the EVE image server.
pub fn eve_image(url: &str, size: i32) -> gtk::Image {
    crate::images::image(url, size)
}

pub fn avatar(url: &str, size: i32) -> gtk::Widget {
    crate::images::avatar(url, size)
}
