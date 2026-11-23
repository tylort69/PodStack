//! The tray icon on Windows (the notification area), with the same tooltip and menu as on Linux: a line per character
//! with its training and queue, then Show/Hide and Quit. Clicking the icon shows the window; right-click opens the
//! menu. Settings → General: tray icon on/off, close to tray, start hidden in tray.
//!
//! The icon lives on the GTK thread: Windows delivers its clicks as window messages, which GTK's main loop already
//! dispatches, and `tray-icon` hands them over through channels that a short timer drains.

pub use super::window_settings::{WindowSettings, SETTINGS_KEY};
use super::tray_menu::{entries, Entry};
use gtk::{gdk_pixbuf, glib, prelude::*};
use podstack_core::{config, Core, Event};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Windows cuts tray tooltips at 127 characters.
const TOOLTIP_MAX: usize = 127;

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct State {
    app: gtk::Application,
    window: glib::WeakRef<gtk::ApplicationWindow>,
    core: Arc<Core>,
    tray: Option<TrayIcon>,
}

fn window() -> Option<gtk::ApplicationWindow> {
    STATE.with(|s| s.borrow().as_ref().and_then(|s| s.window.upgrade()))
}

/// The app icon as RGBA for the notification area.
fn icon() -> Option<tray_icon::Icon> {
    let bytes = glib::Bytes::from_static(include_bytes!("../../../../assets/icon.png"));
    let stream = gtk::gio::MemoryInputStream::from_bytes(&bytes);
    let pb = gdk_pixbuf::Pixbuf::from_stream(&stream, gtk::gio::Cancellable::NONE).ok()?;
    let pb = pb.scale_simple(32, 32, gdk_pixbuf::InterpType::Bilinear)?;
    let pb = if pb.has_alpha() { pb } else { pb.add_alpha(false, 0, 0, 0).ok()? };
    let (w, h, stride) = (pb.width() as usize, pb.height() as usize, pb.rowstride() as usize);
    let px = pb.read_pixel_bytes();
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        rgba.extend_from_slice(&px[y * stride..y * stride + w * 4]);
    }
    tray_icon::Icon::from_rgba(rgba, w as u32, h as u32).ok()
}

fn tooltip(list: &[Entry]) -> String {
    let mut text = if list.is_empty() {
        format!("{}\nNo characters yet", config::APP_NAME)
    } else {
        std::iter::once(config::APP_NAME.to_string()).chain(list.iter().map(|e| e.short.clone())).collect::<Vec<_>>().join("\n")
    };
    if text.chars().count() > TOOLTIP_MAX {
        text = text.chars().take(TOOLTIP_MAX - 1).collect::<String>() + "…";
    }
    text
}

fn menu(list: &[Entry], visible: bool) -> Menu {
    let menu = Menu::new();
    for e in list {
        // "&" marks a keyboard shortcut in Windows menus.
        let _ = menu.append(&MenuItem::with_id(format!("open:{}", e.id), e.label.replace('&', "&&"), true, None));
    }
    if !list.is_empty() {
        let _ = menu.append(&PredefinedMenuItem::separator());
    }
    let _ = menu.append(&MenuItem::with_id("toggle", if visible { "Hide PodStack" } else { "Show PodStack" }, true, None));
    let _ = menu.append(&MenuItem::with_id("quit", "Quit", true, None));
    menu
}

/// Starts the tray icon if enabled in settings (called once at startup, on the GTK thread).
pub fn start(app: &gtk::Application, window: &gtk::ApplicationWindow, core: Arc<Core>) {
    STATE.with(|s| *s.borrow_mut() = Some(State { app: app.clone(), window: window.downgrade(), core: core.clone(), tray: None }));
    let settings: WindowSettings = core.setting(SETTINGS_KEY);
    if settings.tray_icon {
        show_icon(settings.start_hidden);
    }

    glib::timeout_add_local(std::time::Duration::from_millis(100), || {
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                super::alerts::show_app(None);
            }
        }
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            on_menu(&event.id.0);
        }
        glib::ControlFlow::Continue
    });

    // keep the menu current: on character changes (batched) and every minute for the countdowns
    let mut rx = core.subscribe();
    glib::spawn_future_local(async move {
        let pending = std::rc::Rc::new(std::cell::Cell::new(false));
        loop {
            match rx.recv().await {
                Ok(Event::CharacterUpdated(_) | Event::CharactersChanged) => {
                    if !pending.replace(true) {
                        let pending = pending.clone();
                        glib::timeout_add_seconds_local_once(3, move || {
                            pending.set(false);
                            update();
                        });
                    }
                }
                Ok(Event::SettingsChanged) => apply_settings(),
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(_) => break,
            }
        }
    });
    glib::timeout_add_seconds_local(60, || {
        update();
        glib::ControlFlow::Continue
    });
    window.connect_visible_notify(|_| update());
}

fn show_icon(hide_window: bool) {
    let Some(core) = STATE.with(|s| s.borrow().as_ref().map(|s| s.core.clone())) else { return };
    let list = entries(&core);
    let mut builder = TrayIconBuilder::new()
        .with_tooltip(tooltip(&list))
        .with_menu(Box::new(menu(&list, !hide_window)))
        .with_menu_on_left_click(false);
    if let Some(icon) = icon() {
        builder = builder.with_icon(icon);
    }
    match builder.build() {
        Ok(tray) => {
            RUNNING.store(true, Ordering::Relaxed);
            STATE.with(|s| {
                if let Some(s) = s.borrow_mut().as_mut() {
                    s.tray = Some(tray);
                }
            });
            // only hide once the icon is there to bring the window back
            if hide_window {
                if let Some(w) = window() {
                    w.set_visible(false);
                }
            }
        }
        Err(e) => tracing::info!("tray icon not shown: {e}"),
    }
}

/// Starts or stops the icon to match Settings → General.
fn apply_settings() {
    let Some((core, running)) = STATE.with(|s| s.borrow().as_ref().map(|s| (s.core.clone(), s.tray.is_some()))) else { return };
    let want = core.setting::<WindowSettings>(SETTINGS_KEY).tray_icon;
    if want && !running {
        show_icon(false);
    } else if !want && running {
        // dropping the icon removes it from the notification area
        STATE.with(|s| s.borrow_mut().as_mut().map(|s| s.tray.take()));
        RUNNING.store(false, Ordering::Relaxed);
    }
}

fn on_menu(id: &str) {
    match id {
        "toggle" => match window() {
            Some(w) if w.is_visible() => w.set_visible(false),
            _ => super::alerts::show_app(None),
        },
        "quit" => {
            if let Some((app, core)) = STATE.with(|s| s.borrow().as_ref().map(|s| (s.app.clone(), s.core.clone()))) {
                core.flush();
                app.quit();
            }
        }
        other => {
            if let Some(id) = other.strip_prefix("open:").and_then(|i| i.parse().ok()) {
                super::alerts::show_app(Some(id));
            }
        }
    }
}

/// Refreshes the tooltip and menu.
fn update() {
    STATE.with(|s| {
        let s = s.borrow();
        let Some(s) = s.as_ref() else { return };
        let Some(tray) = s.tray.as_ref() else { return };
        let list = entries(&s.core);
        let visible = s.window.upgrade().map(|w| w.is_visible()).unwrap_or(false);
        let _ = tray.set_tooltip(Some(tooltip(&list)));
        tray.set_menu(Some(Box::new(menu(&list, visible))));
    });
}

/// Whether the tray icon is showing.
pub fn is_running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// Whether closing the window should hide it to the tray instead of quitting.
pub fn close_to_tray(core: &Core) -> bool {
    is_running() && core.setting::<WindowSettings>(SETTINGS_KEY).close_to_tray
}
