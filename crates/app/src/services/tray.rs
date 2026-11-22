//! System tray icon (StatusNotifierItem, shown by waybar's tray module and similar bars).
//!
//! Like the original app's tray, the tooltip lists how long each character's queue has left and the menu has a line
//! per character (skill in training, its time and the queue's), then Show/Hide and Quit. Clicking the icon shows
//! the window. Settings → Window: tray icon on/off, close to tray, start hidden in tray.

use crate::rt;
use futures::StreamExt;
use gtk::{gdk_pixbuf, glib, prelude::*};
use podstack_core::{config, fmt, Core, Event};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Settings key (`core.setting::<WindowSettings>(SETTINGS_KEY)`).
pub const SETTINGS_KEY: &str = "window";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowSettings {
    pub tray_icon: bool,
    /// Closing the window hides it to the tray (when the tray icon is showing).
    pub close_to_tray: bool,
    /// Start with only the tray icon.
    pub start_hidden: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        WindowSettings { tray_icon: true, close_to_tray: false, start_hidden: false }
    }
}

/// Whether a tray host (e.g. waybar) took the icon.
static RUNNING: AtomicBool = AtomicBool::new(false);

const LEVELS: [&str; 6] = ["", "I", "II", "III", "IV", "V"];

#[derive(Clone, Debug)]
enum Msg {
    Toggle,
    Show,
    Open(i64),
    Quit,
}

#[derive(Clone, Default)]
struct Entry {
    id: i64,
    short: String,
    label: String,
}

struct Tray {
    tx: futures::channel::mpsc::UnboundedSender<Msg>,
    icon: Vec<ksni::Icon>,
    visible: bool,
    entries: Vec<Entry>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "podstack".into()
    }
    fn title(&self) -> String {
        config::APP_NAME.into()
    }
    fn icon_name(&self) -> String {
        // only used when the pixmap can't be (hosts prefer the theme icon when it exists)
        if self.icon.is_empty() { "applications-games".into() } else { String::new() }
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icon.clone()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        let description = if self.entries.is_empty() {
            "No characters yet".to_string()
        } else {
            self.entries.iter().map(|e| e.short.as_str()).collect::<Vec<_>>().join("\n")
        };
        ksni::ToolTip { title: config::APP_NAME.into(), description, ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.unbounded_send(Msg::Show);
    }
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let _ = self.tx.unbounded_send(Msg::Toggle);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        let mut items: Vec<MenuItem<Self>> = vec![];
        for e in &self.entries {
            let id = e.id;
            items.push(
                StandardItem { label: e.label.replace('_', "__"), activate: Box::new(move |t: &mut Self| { let _ = t.tx.unbounded_send(Msg::Open(id)); }), ..Default::default() }
                    .into(),
            );
        }
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        items.push(
            StandardItem {
                label: if self.visible { "Hide PodStack".into() } else { "Show PodStack".into() },
                icon_name: if self.visible { "window-close-symbolic".into() } else { "window-new-symbolic".into() },
                activate: Box::new(|t: &mut Self| { let _ = t.tx.unbounded_send(Msg::Toggle); }),
                ..Default::default()
            }
            .into(),
        );
        items.push(
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(|t: &mut Self| { let _ = t.tx.unbounded_send(Msg::Quit); }),
                ..Default::default()
            }
            .into(),
        );
        items
    }
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct State {
    app: gtk::Application,
    window: glib::WeakRef<gtk::ApplicationWindow>,
    core: Arc<Core>,
    handle: Option<ksni::Handle<Tray>>,
    starting: bool,
}

fn short(d: chrono::Duration) -> String {
    fmt::duration(d, 2)
}

fn entries(core: &Core) -> Vec<Entry> {
    let now = chrono::Utc::now();
    let mut chars: Vec<_> = core.characters().into_iter().filter(|c| c.has_basic_info()).collect();
    chars.sort_by_key(|c| std::cmp::Reverse(c.total_sp()));
    chars
        .iter()
        .map(|c| {
            let name = c.name();
            match (c.current_skill(), c.queue_end()) {
                (Some(cur), Some(end)) => {
                    let skill = if cur.skill_name.is_empty() { "Training".to_string() } else { cur.skill_name.clone() };
                    let level = LEVELS.get(cur.finished_level as usize).copied().unwrap_or("");
                    let skill_left = cur.finish_date.map(|f| short(f - now)).unwrap_or_default();
                    Entry {
                        id: c.id,
                        short: format!("{name}: {}", short(end - now)),
                        label: format!("{name} — {skill} {level} · {skill_left} (queue {})", short(end - now)),
                    }
                }
                _ => Entry { id: c.id, short: format!("{name}: idle"), label: format!("{name} — not training") },
            }
        })
        .collect()
}

/// The app icon as ARGB32 for the StatusNotifierItem.
fn icon() -> Vec<ksni::Icon> {
    let bytes = glib::Bytes::from_static(include_bytes!("../../../../assets/icon.png"));
    let stream = gtk::gio::MemoryInputStream::from_bytes(&bytes);
    let Ok(pb) = gdk_pixbuf::Pixbuf::from_stream(&stream, gtk::gio::Cancellable::NONE) else { return vec![] };
    [32, 64]
        .into_iter()
        .filter_map(|size| {
            let pb = pb.scale_simple(size, size, gdk_pixbuf::InterpType::Bilinear)?;
            let pb = if pb.has_alpha() { pb } else { pb.add_alpha(false, 0, 0, 0).ok()? };
            let (w, h, stride) = (pb.width() as usize, pb.height() as usize, pb.rowstride() as usize);
            let px = pb.read_pixel_bytes();
            let mut data = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let i = y * stride + x * 4;
                    data.extend_from_slice(&[px[i + 3], px[i], px[i + 1], px[i + 2]]);
                }
            }
            Some(ksni::Icon { width: w as i32, height: h as i32, data })
        })
        .collect()
}

/// Starts the tray icon if enabled in settings (called once at startup, on the GTK thread).
pub fn start(app: &gtk::Application, window: &gtk::ApplicationWindow, core: Arc<Core>) {
    STATE.with(|s| {
        *s.borrow_mut() = Some(State { app: app.clone(), window: window.downgrade(), core: core.clone(), handle: None, starting: false })
    });
    let settings: WindowSettings = core.setting(SETTINGS_KEY);
    if settings.tray_icon {
        spawn(settings.start_hidden);
    }

    // keep the menu current: on character changes (batched) and every minute for the countdowns
    let pending = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut rx = core.subscribe();
    glib::spawn_future_local(async move {
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

/// Starts or stops the icon to match Settings → Window.
fn apply_settings() {
    let Some((core, running, starting)) = STATE.with(|s| s.borrow().as_ref().map(|s| (s.core.clone(), s.handle.is_some(), s.starting))) else {
        return;
    };
    let want = core.setting::<WindowSettings>(SETTINGS_KEY).tray_icon;
    if want && !running && !starting {
        spawn(false);
    } else if !want && running {
        if let Some(h) = STATE.with(|s| s.borrow_mut().as_mut().and_then(|s| s.handle.take())) {
            RUNNING.store(false, Ordering::Relaxed);
            rt::RUNTIME.spawn(async move { h.shutdown().await });
        }
    }
}

fn spawn(hide_window: bool) {
    let Some(core) = STATE.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut()?;
        s.starting = true;
        Some(s.core.clone())
    }) else {
        return;
    };
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Msg>();
    let tray = Tray { tx, icon: icon(), visible: !hide_window, entries: entries(&core) };
    rt::run(
        async move {
            use ksni::TrayMethods;
            tray.spawn().await
        },
        move |result| {
            STATE.with(|s| {
                if let Some(s) = s.borrow_mut().as_mut() {
                    s.starting = false;
                }
            });
            match result {
                Ok(handle) => {
                    RUNNING.store(true, Ordering::Relaxed);
                    STATE.with(|s| {
                        if let Some(s) = s.borrow_mut().as_mut() {
                            s.handle = Some(handle);
                        }
                    });
                    // only hide once the icon is there to bring the window back
                    if hide_window {
                        if let Some(w) = window() {
                            w.set_visible(false);
                        }
                    }
                    update();
                }
                Err(e) => tracing::info!("tray icon not shown: {e}"),
            }
        },
    );
    glib::spawn_future_local(async move {
        while let Some(msg) = rx.next().await {
            on_msg(msg);
        }
    });
}

fn window() -> Option<gtk::ApplicationWindow> {
    STATE.with(|s| s.borrow().as_ref().and_then(|s| s.window.upgrade()))
}

fn on_msg(msg: Msg) {
    match msg {
        Msg::Show => super::alerts::show_app(None),
        Msg::Open(id) => super::alerts::show_app(Some(id)),
        Msg::Toggle => match window() {
            Some(w) if w.is_visible() => w.set_visible(false),
            _ => super::alerts::show_app(None),
        },
        Msg::Quit => {
            if let Some(app) = STATE.with(|s| s.borrow().as_ref().map(|s| s.app.clone())) {
                if let Some(s) = STATE.with(|s| s.borrow().as_ref().map(|s| s.core.clone())) {
                    s.flush();
                }
                app.quit();
            }
        }
    }
}

/// Refreshes the tooltip and menu.
fn update() {
    let Some((core, handle)) = STATE.with(|s| s.borrow().as_ref().and_then(|s| Some((s.core.clone(), s.handle.clone()?)))) else { return };
    let list = entries(&core);
    let visible = window().map(|w| w.is_visible()).unwrap_or(false);
    rt::RUNTIME.spawn(async move {
        handle
            .update(move |t| {
                t.entries = list;
                t.visible = visible;
            })
            .await;
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
