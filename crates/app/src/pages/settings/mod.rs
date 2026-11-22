//! Settings: appearance (Material You theme source, mode, style, contrast), EVE login application, desktop alerts,
//! window and tray behaviour, password-protected backup and restore, and the data folders.
//!
//! The page is static (it holds text input): each control saves straight away through `Core`, and the parts that
//! show changing state (recent alerts, tray status) update themselves.

mod alerts;
mod appearance;
mod backup;

use crate::ctx::{Ctx, Page};
use crate::services::tray::{self, WindowSettings};
use crate::widgets::{self as w, Btn};
use gtk::{gio, glib, prelude::*};
use podstack_core::{config, sso, store, SsoSettings};

const CSS: &str = r#"
.settings-card { padding: 8px 0 16px 0; color: @on_surface; }
.settings-card .settings-head { padding: 16px 24px 8px 24px; }
.icon.settings-icon {
  min-width: 40px; min-height: 40px; background-color: @secondary_container; color: @on_secondary_container; border-radius: 999px;
}
.settings-card .list-item { padding: 10px 24px; }
.settings-card .settings-block { padding: 8px 24px; }
.settings-card separator.settings-sep { margin: 8px 24px; background-color: @outline_variant; }
.settings-subhead { color: @primary; font-size: 14px; font-weight: 500; padding: 12px 24px 4px 24px; }
button.settings-swatch {
  border-radius: 16px; padding: 10px 6px 8px 6px; min-width: 104px; min-height: 0; color: @on_surface;
  font-weight: 400; background-color: transparent;
}
button.settings-swatch:hover { background-color: alpha(@on_surface, 0.08); }
button.settings-swatch.selected { background-color: @secondary_container; color: @on_secondary_container; }
.settings-mono { font-family: monospace; }
.settings-alert-row { padding: 6px 24px; }
"#;

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.set_spacing(20);
    body.append(&appearance::card(ctx));
    body.append(&login_card(ctx));
    body.append(&alerts::card(ctx));
    body.append(&window_card(ctx));
    body.append(&backup::card(ctx));
    body.append(&data_card(ctx));
    Page::new("Settings", body).static_page()
}

/// A settings card: icon, title and explanation in the header, then `rows`.
pub(super) fn settings_card(icon: &str, title: &str, subtitle: Option<&str>, actions: &[gtk::Widget]) -> gtk::Box {
    let card = w::vbox(0);
    card.add_css_class("card");
    card.add_css_class("settings-card");
    let head = w::hbox(16);
    head.add_css_class("settings-head");
    let i = w::icon_sized(icon, 20);
    i.add_css_class("settings-icon");
    i.set_valign(gtk::Align::Start);
    head.append(&i);
    let titles = w::vbox(2);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    titles.append(&w::text(title, &["title-large"]));
    if let Some(s) = subtitle {
        titles.append(&w::text(s, &["body-medium", "dim"]));
    }
    head.append(&titles);
    for a in actions {
        a.set_valign(gtk::Align::Center);
        head.append(a);
    }
    card.append(&head);
    card
}

pub(super) fn separator() -> gtk::Separator {
    let s = gtk::Separator::new(gtk::Orientation::Horizontal);
    s.add_css_class("settings-sep");
    s
}

pub(super) fn subhead(t: &str) -> gtk::Label {
    w::line(t, &["settings-subhead"])
}

/// A padded block inside a settings card.
pub(super) fn block(spacing: i32) -> gtk::Box {
    let b = w::vbox(spacing);
    b.add_css_class("settings-block");
    b
}

/// A list row whose trailing widget is a control (dropdown, segmented buttons, …).
pub(super) fn control_row(title: &str, subtitle: Option<&str>, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = w::hbox(16);
    row.add_css_class("list-item");
    let texts = w::vbox(2);
    texts.set_hexpand(true);
    texts.set_valign(gtk::Align::Center);
    texts.append(&w::text(title, &["body-large"]));
    if let Some(s) = subtitle {
        texts.append(&w::text(s, &["body-medium", "dim"]));
    }
    row.append(&texts);
    control.as_ref().set_valign(gtk::Align::Center);
    row.append(control);
    row
}

fn login_card(ctx: &Ctx) -> gtk::Box {
    let core = ctx.core().clone();
    let card = settings_card(
        "vpn_key",
        "EVE login",
        Some("Which EVE application PodStack logs characters in with."),
        &[],
    );
    let b = block(12);
    b.append(&w::text(
        "By default PodStack uses its own EVE application: after logging in, the browser lands on a page that \
         doesn't load, and you paste its address back into PodStack. You can register your own application at \
         developers.eveonline.com instead (tick every scope PodStack uses) and enter its client ID and callback \
         here. With a callback such as http://127.0.0.1:8765/callback, PodStack listens on that port and catches \
         the login automatically. Characters already added keep working: each remembers the application that \
         logged it in.",
        &["body-medium", "dim"],
    ));

    let sso_now = core.settings().sso;
    let client = gtk::Entry::new();
    client.set_placeholder_text(Some("Client ID"));
    client.set_text(sso_now.client_id.as_deref().unwrap_or(""));
    client.set_hexpand(true);
    let callback = gtk::Entry::new();
    callback.set_placeholder_text(Some("Callback URL, e.g. http://127.0.0.1:8765/callback"));
    callback.set_text(sso_now.callback.as_deref().unwrap_or(""));
    callback.set_hexpand(true);
    let fields = w::hbox(12);
    fields.append(&client);
    fields.append(&callback);
    b.append(&fields);

    let status = w::text("", &["body-medium"]);
    let show_status = {
        let (status, core) = (status.clone(), core.clone());
        move || {
            let (id, cb) = core.sso_client();
            if id == config::DEFAULT_CLIENT_ID {
                status.set_text("Using PodStack's application: you'll paste the address the browser lands on.");
                status.set_css_classes(&["body-medium", "dim"]);
            } else {
                let how = match sso::loopback_port(&cb) {
                    Some(port) => format!("logins are caught automatically on port {port}"),
                    None => "you'll paste the address the browser lands on".into(),
                };
                status.set_text(&format!("Using your application ({id}): {how}."));
                status.set_css_classes(&["body-medium", "success-text"]);
            }
        }
    };
    show_status();
    b.append(&status);

    let actions = w::hbox(8);
    let portal = w::button_icon("open_in_new", "developers.eveonline.com", Btn::Text);
    let scopes = w::button_icon("content_copy", "Copy scope list", Btn::Text);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    let reset = w::button("Use PodStack's", Btn::Outlined);
    let save = w::button("Save", Btn::Filled);
    for x in [portal.upcast_ref::<gtk::Widget>(), scopes.upcast_ref(), spacer.upcast_ref(), reset.upcast_ref(), save.upcast_ref()] {
        actions.append(x);
    }
    b.append(&actions);
    card.append(&b);

    {
        let ctx = ctx.clone();
        portal.connect_clicked(move |_| w::open_url(&ctx, "https://developers.eveonline.com/applications"));
    }
    {
        let ctx = ctx.clone();
        scopes.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &config::scope_names().join(" ")));
    }
    {
        let (ctx, client, callback, show_status) = (ctx.clone(), client.clone(), callback.clone(), show_status.clone());
        reset.connect_clicked(move |_| {
            client.set_text("");
            callback.set_text("");
            ctx.core().update_settings(|s| s.sso = SsoSettings::default());
            show_status();
            ctx.toast("Logins use PodStack's application");
        });
    }
    {
        let ctx = ctx.clone();
        save.connect_clicked(move |_| {
            let id = client.text().trim().to_string();
            let cb = callback.text().trim().to_string();
            let sso = match (id.is_empty(), cb.is_empty()) {
                (true, true) => SsoSettings::default(),
                (false, false) => {
                    let rest = cb.strip_prefix("http://").or_else(|| cb.strip_prefix("https://")).unwrap_or("");
                    let ok = !rest.is_empty() && !rest.starts_with('/') && !cb.contains(char::is_whitespace);
                    if !ok {
                        ctx.toast("The callback must be a web address, e.g. http://127.0.0.1:8765/callback");
                        return;
                    }
                    SsoSettings { client_id: Some(id), callback: Some(cb) }
                }
                _ => {
                    ctx.toast("Enter both the client ID and the callback (or neither)");
                    return;
                }
            };
            ctx.core().update_settings(|s| s.sso = sso);
            show_status();
            ctx.toast("EVE login settings saved");
        });
    }
    card
}

fn window_card(ctx: &Ctx) -> gtk::Box {
    let core = ctx.core().clone();
    let card = settings_card("web_asset", "Window", Some("What closing the window does, and the tray icon."), &[]);
    let s: WindowSettings = core.setting(tray::SETTINGS_KEY);

    let save = {
        let core = core.clone();
        move |f: &dyn Fn(&mut WindowSettings)| {
            let mut s: WindowSettings = core.setting(tray::SETTINGS_KEY);
            f(&mut s);
            core.set_setting(tray::SETTINGS_KEY, &s);
        }
    };
    let save = std::rc::Rc::new(save);
    let dependent = w::vbox(0);
    dependent.set_sensitive(s.tray_icon);
    let tray_status = w::text("", &["body-small", "dim"]);
    tray_status.set_margin_start(24);
    tray_status.set_margin_end(24);
    let show_tray_status = {
        let (tray_status, core) = (tray_status.clone(), core.clone());
        move || {
            let on = core.setting::<WindowSettings>(tray::SETTINGS_KEY).tray_icon;
            let (text, class) = match (on, tray::is_running()) {
                (true, true) => ("The tray icon is showing.", "success-text"),
                (true, false) => ("No tray found: closing quits PodStack. Waybar needs its \"tray\" module.", "warning-text"),
                (false, _) => ("The tray icon is off.", "dim"),
            };
            tray_status.set_text(text);
            tray_status.set_css_classes(&["body-small", class]);
        }
    };
    show_tray_status();

    {
        let (save, dependent, show) = (save.clone(), dependent.clone(), show_tray_status.clone());
        card.append(&w::switch_row(
            "Tray icon",
            Some("Shows PodStack in your bar's system tray, with each character's training and a way back to the window."),
            s.tray_icon,
            move |on| {
                save(&|s| s.tray_icon = on);
                dependent.set_sensitive(on);
                // the tray takes a moment to (dis)appear
                let show = show.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(1200), show);
            },
        ));
    }
    card.append(&tray_status);
    {
        let save = save.clone();
        dependent.append(&w::switch_row(
            "Close to tray",
            Some("Closing the window keeps PodStack running in the tray, refreshing and alerting. Quit from the tray menu."),
            s.close_to_tray,
            move |on| save(&|s| s.close_to_tray = on),
        ));
    }
    {
        let save = save.clone();
        dependent.append(&w::switch_row(
            "Start hidden in tray",
            Some("Start with only the tray icon; click it to show the window."),
            s.start_hidden,
            move |on| save(&|s| s.start_hidden = on),
        ));
    }
    card.append(&dependent);
    card
}

fn dir_size(dir: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

fn human_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

fn open_folder(ctx: &Ctx, dir: &std::path::Path) {
    let file = gio::File::for_path(dir);
    w::open_url(ctx, &file.uri());
}

fn data_card(ctx: &Ctx) -> gtk::Box {
    let card = settings_card("folder", "Data", Some("Where PodStack keeps its files."), &[]);
    let data = store::data_dir();
    let cache = store::cache_dir();

    let open_data = w::button("Open", Btn::Tonal);
    {
        let (ctx, data) = (ctx.clone(), data.clone());
        open_data.connect_clicked(move |_| open_folder(&ctx, &data));
    }
    let lead = w::icon("folder_open").upcast::<gtk::Widget>();
    card.append(&w::list_row(Some(&lead), "Data folder", Some(&format!("{} · logins, characters, plans, settings", data.display())), Some(open_data.upcast_ref())));

    let images = cache.join("images");
    let size_label = format!("{} · images {}", cache.display(), human_size(dir_size(&images)));
    let clear = w::button("Clear image cache", Btn::Outlined);
    let open_cache = w::button("Open", Btn::Text);
    {
        let (ctx, cache) = (ctx.clone(), cache.clone());
        open_cache.connect_clicked(move |_| open_folder(&ctx, &cache));
    }
    let trailing = w::hbox(8);
    trailing.append(&open_cache);
    trailing.append(&clear);
    let lead = w::icon("cached").upcast::<gtk::Widget>();
    let row = w::list_row(Some(&lead), "Cache", Some(&size_label), Some(trailing.upcast_ref()));
    card.append(&row);
    {
        let ctx = ctx.clone();
        clear.connect_clicked(move |b| {
            let images = images.clone();
            let ctx2 = ctx.clone();
            let b = b.clone();
            w::confirm(
                &ctx,
                "Clear image cache?",
                "Portraits, logos and item icons are downloaded again as they're shown. Names and other data PodStack is using stay cached.",
                "Clear",
                false,
                move || {
                    let freed = dir_size(&images);
                    let _ = std::fs::remove_dir_all(&images);
                    b.set_sensitive(false);
                    ctx2.toast(&format!("Cleared {}", human_size(freed)));
                },
            );
        });
    }
    card
}
