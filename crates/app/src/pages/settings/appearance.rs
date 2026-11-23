//! Settings → Appearance: where the Material You colors come from (wallpaper via matugen, an EVE faction, or a
//! custom seed), dark/light mode, scheme style and contrast. Changes are saved with `core.update_settings`; the
//! app re-themes itself on `SettingsChanged`.

use super::{control_row, separator, settings_card, subhead};
use crate::ctx::Ctx;
use crate::theme::{self, presets, scheme};
use crate::widgets::{self as w, Btn};
use crate::rt;
use gtk::{gdk, gio, glib, prelude::*};
use podstack_core::{ThemeMode, ThemeSettings, ThemeSource};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Updates run when the theme changes (selection marks, the image in use).
type Refreshers = Rc<RefCell<Vec<Box<dyn Fn(&ThemeSettings)>>>>;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    /// The desktop's accent color (Windows).
    Accent,
    Wallpaper,
    Faction,
    Custom,
}

fn kind_of(source: &ThemeSource, matugen: bool, accent: bool) -> Kind {
    match source {
        ThemeSource::Auto if accent => Kind::Accent,
        ThemeSource::Auto | ThemeSource::Matugen { .. } if matugen => Kind::Wallpaper,
        ThemeSource::Auto | ThemeSource::Matugen { .. } | ThemeSource::Faction { .. } => Kind::Faction,
        ThemeSource::Custom { .. } => Kind::Custom,
    }
}

fn set_theme(ctx: &Ctx, f: impl FnOnce(&mut ThemeSettings)) {
    ctx.core().update_settings(|s| f(&mut s.theme));
}

/// The faction a theme shows as selected (Auto without matugen falls back to the default faction).
fn selected_faction(t: &ThemeSettings) -> Option<&'static str> {
    match &t.source {
        ThemeSource::Faction { name } => Some(presets::find(name).name),
        ThemeSource::Auto if theme::desktop_accent().is_some() => None,
        ThemeSource::Auto | ThemeSource::Matugen { .. } if !theme::matugen_available() => Some(presets::DEFAULT_FACTION),
        _ => None,
    }
}

fn parse_rgba(hex: &str) -> Option<gdk::RGBA> {
    let h = hex.trim();
    let h = if h.starts_with('#') { h.to_string() } else { format!("#{h}") };
    if h.len() != 7 || !h[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    gdk::RGBA::parse(&h).ok()
}

fn rgba_hex(c: &gdk::RGBA) -> String {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", b(c.red()), b(c.green()), b(c.blue()))
}

/// A round swatch of a preset's seed color (the color itself is what's being shown, so it isn't a scheme role).
fn swatch_circle(seed: &'static str, selected: Rc<Cell<bool>>) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::new();
    area.set_content_width(44);
    area.set_content_height(44);
    area.set_halign(gtk::Align::Center);
    let color = gdk::RGBA::parse(seed).unwrap_or(gdk::RGBA::BLACK);
    area.set_draw_func(move |_, cr, wd, ht| {
        let (cx, cy) = (wd as f64 / 2.0, ht as f64 / 2.0);
        let r = cx.min(cy);
        if selected.get() {
            let ring = theme::rgba("on_secondary_container");
            cr.set_source_rgba(ring.red() as f64, ring.green() as f64, ring.blue() as f64, 1.0);
            cr.arc(cx, cy, r, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
            let gap = theme::rgba("secondary_container");
            cr.set_source_rgba(gap.red() as f64, gap.green() as f64, gap.blue() as f64, 1.0);
            cr.arc(cx, cy, r - 2.5, 0.0, std::f64::consts::TAU);
            let _ = cr.fill();
        }
        cr.set_source_rgba(color.red() as f64, color.green() as f64, color.blue() as f64, 1.0);
        cr.arc(cx, cy, if selected.get() { r - 5.0 } else { r - 2.0 }, 0.0, std::f64::consts::TAU);
        let _ = cr.fill();
        if selected.get() {
            // a check mark in the contrasting tone
            let light = color.red() * 0.299 + color.green() * 0.587 + color.blue() * 0.114 > 0.6;
            let v = if light { 0.0 } else { 1.0 };
            cr.set_source_rgba(v, v, v, 0.9);
            cr.set_line_width(2.5);
            cr.set_line_cap(gtk::cairo::LineCap::Round);
            cr.set_line_join(gtk::cairo::LineJoin::Round);
            cr.move_to(cx - 6.0, cy + 0.5);
            cr.line_to(cx - 1.5, cy + 5.0);
            cr.line_to(cx + 7.0, cy - 4.5);
            let _ = cr.stroke();
        }
    });
    area
}

fn faction_grid(ctx: &Ctx, refresh: Refreshers) -> gtk::FlowBox {
    let grid = gtk::FlowBox::new();
    grid.set_selection_mode(gtk::SelectionMode::None);
    grid.set_homogeneous(true);
    grid.set_max_children_per_line(7);
    grid.set_min_children_per_line(3);
    grid.set_column_spacing(4);
    grid.set_row_spacing(4);
    grid.set_margin_start(16);
    grid.set_margin_end(16);
    for p in presets::FACTIONS {
        let btn = gtk::Button::new();
        btn.add_css_class("settings-swatch");
        btn.set_tooltip_text(Some(p.blurb));
        let col = w::vbox(6);
        let selected = Rc::new(Cell::new(false));
        let circle = swatch_circle(p.seed, selected.clone());
        col.append(&circle);
        let name = w::line(p.name, &["label-medium"]);
        name.set_xalign(0.5);
        col.append(&name);
        btn.set_child(Some(&col));
        {
            let ctx = ctx.clone();
            btn.connect_clicked(move |_| set_theme(&ctx, |t| t.source = ThemeSource::Faction { name: p.name.into() }));
        }
        let b = btn.clone();
        refresh.borrow_mut().push(Box::new(move |t| {
            let on = selected_faction(t) == Some(p.name);
            selected.set(on);
            if on {
                b.add_css_class("selected");
            } else {
                b.remove_css_class("selected");
            }
            circle.queue_draw();
        }));
        grid.append(&btn);
    }
    grid
}

fn pick_image(ctx: &Ctx) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("Images"));
    filter.add_mime_type("image/*");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let dialog = gtk::FileDialog::builder().title("Pick an image for PodStack's colors").modal(true).filters(&filters).build();
    if let Some(dir) = theme::current_wallpaper().and_then(|p| std::path::Path::new(&p).parent().map(|d| d.to_path_buf())) {
        dialog.set_initial_folder(Some(&gio::File::for_path(dir)));
    }
    let ctx = ctx.clone();
    dialog.open(Some(&ctx.window()), gio::Cancellable::NONE, move |r| {
        if let Ok(file) = r {
            if let Some(path) = file.path() {
                let image = path.to_string_lossy().into_owned();
                set_theme(&ctx, |t| t.source = ThemeSource::Matugen { image: Some(image) });
            }
        }
    });
}

/// Windows accent: nothing to choose, so a swatch and where to change it.
fn accent_pane(color: &str) -> gtk::Box {
    let pane = w::hbox(12);
    pane.set_margin_start(24);
    pane.set_margin_end(24);
    let swatch = gtk::DrawingArea::new();
    swatch.set_content_width(28);
    swatch.set_content_height(28);
    swatch.set_valign(gtk::Align::Center);
    if let Some(rgba) = parse_rgba(color) {
        swatch.set_draw_func(move |_, cr, w, h| {
            let r = f64::from(w.min(h)) / 2.0;
            cr.arc(f64::from(w) / 2.0, f64::from(h) / 2.0, r, 0.0, std::f64::consts::TAU);
            cr.set_source_rgba(rgba.red().into(), rgba.green().into(), rgba.blue().into(), 1.0);
            let _ = cr.fill();
        });
    }
    pane.append(&swatch);
    let text = w::text(
        "PodStack follows your Windows accent color. Change it in Windows Settings → Personalization → Colors; PodStack picks it up the next time it starts.",
        &["body-medium"],
    );
    text.set_wrap(true);
    text.set_hexpand(true);
    pane.append(&text);
    pane
}

fn wallpaper_pane(ctx: &Ctx, refresh: Refreshers) -> gtk::Box {
    let pane = w::vbox(0);
    let detected = w::line("Looking for your wallpaper…", &["supporting"]);
    detected.set_ellipsize(pango::EllipsizeMode::Middle);
    detected.set_selectable(true);
    {
        let detected = detected.clone();
        rt::run(async { tokio::task::spawn_blocking(theme::current_wallpaper).await.ok().flatten() }, move |wp| match wp {
            Some(p) => detected.set_text(&p),
            None => detected.set_text("None found (PodStack reads hyprpaper and swww)"),
        });
    }
    let row = w::hbox(16);
    row.add_css_class("list-item");
    row.append(&w::icon("wallpaper"));
    let texts = w::vbox(2);
    texts.set_hexpand(true);
    texts.append(&w::line("Current wallpaper", &["headline"]));
    texts.append(&detected);
    row.append(&texts);
    pane.append(&row);

    let using = w::line("", &["supporting"]);
    using.set_ellipsize(pango::EllipsizeMode::Middle);
    let row = w::hbox(16);
    row.add_css_class("list-item");
    row.append(&w::icon("palette"));
    let texts = w::vbox(2);
    texts.set_hexpand(true);
    texts.append(&w::line("Colors from", &["headline"]));
    texts.append(&using);
    row.append(&texts);
    let follow = w::button("Use wallpaper", Btn::Text);
    let pick = w::button_icon("image", "Pick image…", Btn::Tonal);
    for b in [&follow, &pick] {
        b.set_valign(gtk::Align::Center);
        row.append(b);
    }
    pane.append(&row);
    {
        let ctx = ctx.clone();
        pick.connect_clicked(move |_| pick_image(&ctx));
    }
    {
        let ctx = ctx.clone();
        follow.connect_clicked(move |_| set_theme(&ctx, |t| t.source = ThemeSource::Matugen { image: None }));
    }
    refresh.borrow_mut().push(Box::new(move |t| {
        let image = match &t.source {
            ThemeSource::Matugen { image: Some(i) } => Some(i.clone()),
            _ => None,
        };
        follow.set_visible(image.is_some());
        using.set_text(&match image {
            Some(i) => i,
            None => "Your wallpaper (read again each time PodStack starts)".into(),
        });
    }));
    pane
}

fn custom_pane(ctx: &Ctx) -> gtk::Box {
    let pane = w::vbox(0);
    let current = match &ctx.core().settings().theme.source {
        ThemeSource::Custom { seed } => seed.clone(),
        _ => theme::current().get("primary").to_string(),
    };
    let dialog = gtk::ColorDialog::new();
    dialog.set_with_alpha(false);
    dialog.set_title("Seed color");
    let button = gtk::ColorDialogButton::new(Some(dialog));
    button.set_rgba(&parse_rgba(&current).unwrap_or(gdk::RGBA::BLUE));
    button.set_valign(gtk::Align::Center);
    let entry = gtk::Entry::new();
    entry.set_text(&current);
    entry.set_max_length(7);
    entry.set_width_chars(9);
    entry.set_placeholder_text(Some("#3d7fb8"));
    entry.add_css_class("settings-mono");
    let apply = w::button("Apply", Btn::Tonal);
    let error = w::line("", &["body-small", "error-text"]);
    error.set_visible(false);

    let row = w::hbox(12);
    row.add_css_class("list-item");
    let texts = w::vbox(2);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    texts.append(&w::text("Seed color", &["body-large"]));
    texts.append(&w::text("Every color role is generated from this one.", &["body-medium", "dim"]));
    texts.append(&error);
    row.append(&texts);
    for x in [button.upcast_ref::<gtk::Widget>(), entry.upcast_ref(), apply.upcast_ref()] {
        x.set_valign(gtk::Align::Center);
        row.append(x);
    }
    pane.append(&row);

    let commit: Rc<dyn Fn(&str)> = {
        let (ctx, error) = (ctx.clone(), error.clone());
        Rc::new(move |hex: &str| match parse_rgba(hex) {
            Some(c) => {
                error.set_visible(false);
                let seed = rgba_hex(&c);
                set_theme(&ctx, |t| t.source = ThemeSource::Custom { seed });
            }
            None => {
                error.set_text("Enter a color as #rrggbb");
                error.set_visible(true);
            }
        })
    };
    // the picker sets the entry and applies; typing applies on Enter or Apply
    let syncing = Rc::new(Cell::new(false));
    {
        let (entry, commit, syncing) = (entry.clone(), commit.clone(), syncing.clone());
        button.connect_rgba_notify(move |b| {
            if syncing.get() {
                return;
            }
            let hex = rgba_hex(&b.rgba());
            entry.set_text(&hex);
            commit(&hex);
        });
    }
    let from_entry = {
        let (entry, button, commit, syncing) = (entry.clone(), button.clone(), commit.clone(), syncing.clone());
        move || {
            let text = entry.text().to_string();
            if let Some(c) = parse_rgba(&text) {
                syncing.set(true);
                button.set_rgba(&c);
                syncing.set(false);
            }
            commit(&text);
        }
    };
    {
        let f = from_entry.clone();
        entry.connect_activate(move |_| f());
    }
    apply.connect_clicked(move |_| from_entry());
    pane
}

pub(super) fn card(ctx: &Ctx) -> gtk::Box {
    let core = ctx.core().clone();
    let t = core.settings().theme;
    let matugen = theme::matugen_available();
    let accent = theme::desktop_accent();
    let card = settings_card(
        "palette",
        "Appearance",
        Some("Material You colors for the whole app, from your wallpaper, an EVE faction or a color of your own."),
        &[],
    );
    let refresh: Refreshers = Rc::new(RefCell::new(vec![]));

    // source
    let mut kinds: Vec<(Kind, &str)> = vec![(Kind::Faction, "EVE faction"), (Kind::Custom, "Custom")];
    if matugen {
        kinds.insert(0, (Kind::Wallpaper, "Wallpaper (matugen)"));
    }
    if accent.is_some() {
        kinds.insert(0, (Kind::Accent, "Windows accent"));
    }
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_vhomogeneous(false);
    stack.set_hhomogeneous(false);
    stack.set_interpolate_size(true);
    if matugen {
        stack.add_named(&wallpaper_pane(ctx, refresh.clone()), Some("wallpaper"));
    }
    if let Some(color) = &accent {
        stack.add_named(&accent_pane(color), Some("accent"));
    }
    let faction = w::vbox(8);
    faction.append(&faction_grid(ctx, refresh.clone()));
    if !matugen && cfg!(target_os = "linux") {
        let hint = w::text("Install matugen (pacman -S matugen) to color PodStack from your wallpaper.", &["body-small", "dim"]);
        hint.set_margin_start(24);
        hint.set_margin_top(4);
        faction.append(&hint);
    }
    stack.add_named(&faction, Some("faction"));
    stack.add_named(&custom_pane(ctx), Some("custom"));
    let page_of = |k: Kind| match k {
        Kind::Accent => "accent",
        Kind::Wallpaper => "wallpaper",
        Kind::Faction => "faction",
        Kind::Custom => "custom",
    };
    let has_accent = accent.is_some();
    let current = kind_of(&t.source, matugen, has_accent);
    stack.set_visible_child_name(page_of(current));

    let labels: Vec<&str> = kinds.iter().map(|k| k.1).collect();
    let selected = kinds.iter().position(|k| k.0 == current).unwrap_or(0);
    let segmented = {
        let (ctx, stack) = (ctx.clone(), stack.clone());
        let kinds: Vec<Kind> = kinds.iter().map(|k| k.0).collect();
        w::segmented(&labels, selected, move |i| {
            let k = kinds[i];
            stack.set_visible_child_name(page_of(k));
            let now = ctx.core().settings().theme;
            if kind_of(&now.source, matugen, has_accent) == k {
                return;
            }
            // switching the source applies its current choice right away
            let source = match k {
                Kind::Accent => ThemeSource::Auto,
                Kind::Wallpaper => ThemeSource::Matugen { image: None },
                Kind::Faction => ThemeSource::Faction { name: presets::DEFAULT_FACTION.into() },
                Kind::Custom => ThemeSource::Custom { seed: theme::current().get("primary").to_string() },
            };
            set_theme(&ctx, |t| t.source = source);
        })
    };
    let seg_row = w::hbox(0);
    seg_row.set_margin_start(24);
    seg_row.set_margin_end(24);
    seg_row.set_margin_top(4);
    seg_row.set_margin_bottom(8);
    seg_row.append(&segmented);
    card.append(&subhead("Color source"));
    card.append(&seg_row);
    card.append(&stack);

    card.append(&separator());

    // mode
    let mode_idx = match t.mode {
        ThemeMode::Dark => 0,
        ThemeMode::Light => 1,
        ThemeMode::System => 2,
    };
    let mode = {
        let ctx = ctx.clone();
        w::segmented(&["Dark", "Light", "System"], mode_idx, move |i| {
            let m = [ThemeMode::Dark, ThemeMode::Light, ThemeMode::System][i];
            set_theme(&ctx, |t| t.mode = m);
        })
    };
    card.append(&control_row("Mode", Some("System follows your desktop's light/dark preference."), &mode));

    // variant
    let names: Vec<&str> = scheme::VARIANTS.iter().map(|v| v.1).collect();
    let idx = scheme::VARIANTS.iter().position(|v| v.0 == t.variant).unwrap_or(0) as u32;
    let variant = w::dropdown(&names, idx);
    {
        let ctx = ctx.clone();
        variant.connect_selected_notify(move |d| {
            if let Some(v) = scheme::VARIANTS.get(d.selected() as usize) {
                let v = v.0.to_string();
                set_theme(&ctx, |t| t.variant = v);
            }
        });
    }
    card.append(&control_row("Style", Some("How colorful the scheme is around its source color."), &variant));

    // contrast, applied once the slider rests (matugen runs on every change)
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, -1.0, 1.0, 0.1);
    scale.set_value(t.contrast);
    scale.set_width_request(240);
    scale.set_draw_value(false);
    scale.add_mark(0.0, gtk::PositionType::Bottom, None);
    let value = w::line("", &["label-large", "dim", "numeric"]);
    value.set_width_chars(8);
    let describe = |v: f64| match v {
        v if v.abs() < 0.05 => "Standard".to_string(),
        v if v < 0.0 => format!("Less {:.1}", v.abs()),
        v => format!("More {v:.1}"),
    };
    value.set_text(&describe(t.contrast));
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    {
        let (ctx, value) = (ctx.clone(), value.clone());
        scale.connect_value_changed(move |s| {
            let v = (s.value() * 10.0).round() / 10.0;
            value.set_text(&describe(v));
            if let Some(id) = pending.borrow_mut().take() {
                id.remove();
            }
            let ctx = ctx.clone();
            let pending2 = pending.clone();
            let id = glib::timeout_add_local_once(std::time::Duration::from_millis(350), move || {
                pending2.borrow_mut().take();
                set_theme(&ctx, |t| t.contrast = v);
            });
            *pending.borrow_mut() = Some(id);
        });
    }
    let contrast = w::hbox(8);
    contrast.append(&scale);
    contrast.append(&value);
    card.append(&control_row("Contrast", Some("More contrast makes text and outlines stand out further."), &contrast));

    // keep the selection marks current as the theme changes
    let apply_refresh = {
        let refresh = refresh.clone();
        move |t: &ThemeSettings| {
            for f in refresh.borrow().iter() {
                f(t);
            }
        }
    };
    apply_refresh(&t);
    {
        let core = core.clone();
        let last = RefCell::new(t);
        ctx.on_event(&card, move |e| {
            if *e == podstack_core::Event::SettingsChanged {
                let t = core.settings().theme;
                if *last.borrow() != t {
                    apply_refresh(&t);
                    *last.borrow_mut() = t;
                }
            }
        });
    }
    card
}
