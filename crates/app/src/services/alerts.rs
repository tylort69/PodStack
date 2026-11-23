//! Alerts (training stopped, injector ready, new mail, …) on the desktop, via freedesktop notifications which mako,
//! dunst, swaync and other notification daemons show, and on Discord. The rules and which alert goes where live in
//! `podstack_core::features::alerts` (Discord delivery in `features::discord`); this checks them every minute on
//! Tokio (also while the window is hidden in the tray) and sends what's new.
//!
//! Clicking a notification raises the window and, when the shell registers the `app.open-character` action
//! (an `i64` parameter), opens that character.

use crate::rt;
use futures::StreamExt;
use gtk::{gio, glib, prelude::*};
use podstack_core::features::alerts::{self, Notice};
use podstack_core::features::discord;
use podstack_core::{config, store, Core};
use std::sync::{Arc, OnceLock};

static CLICKS: OnceLock<futures::channel::mpsc::UnboundedSender<Option<i64>>> = OnceLock::new();

/// Starts watching for alert conditions (called once at startup, on the GTK thread).
pub fn start(core: Arc<Core>) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Option<i64>>();
    if CLICKS.set(tx).is_err() {
        return;
    }
    // notification clicks arrive on worker threads; act on them here
    glib::spawn_future_local(async move {
        while let Some(character) = rx.next().await {
            show_app(character);
        }
    });
    rt::RUNTIME.spawn(async move {
        // give the first refresh a moment, so startup doesn't compare against half-loaded data
        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(alerts::CHECK_INTERVAL_SECS));
        loop {
            tick.tick().await;
            let core = core.clone();
            let check = core.clone();
            let delivery = tokio::task::spawn_blocking(move || alerts::check(&check, chrono::Utc::now())).await.unwrap_or_default();
            if !delivery.discord.is_empty() {
                let core = core.clone();
                let list = delivery.discord.clone();
                tokio::spawn(async move {
                    if let Err(e) = discord::deliver(&core.esi.http, &list).await {
                        tracing::warn!("alerts: Discord: {e}");
                    }
                    core.emit(podstack_core::Event::Doc(alerts::STATE_DOC.into()));
                });
            }
            match delivery.desktop {
                Some(Notice::Separate(list)) => {
                    for c in list {
                        notify(&c.title, &c.body, Some(c.character_id));
                    }
                }
                Some(Notice::Summary { title, body }) => notify(&title, &body, None),
                None => {}
            }
        }
    });
}

/// Raises the window (and opens a character when the shell supports it).
pub fn show_app(character: Option<i64>) {
    let Some(app) = gio::Application::default().and_downcast::<gtk::Application>() else { return };
    if let Some(win) = app.windows().into_iter().next() {
        win.set_visible(true);
        win.present();
    }
    if let Some(id) = character {
        if app.lookup_action("open-character").is_some() {
            app.activate_action("open-character", Some(&id.to_variant()));
        }
    }
}

/// The app icon as a file, for notification daemons that want a path.
fn icon_path() -> Option<String> {
    static PATH: OnceLock<Option<String>> = OnceLock::new();
    PATH.get_or_init(|| {
        let p = store::cache_dir().join("podstack-icon.png");
        if !p.is_file() {
            store::write_bytes(&p, include_bytes!("../../../../assets/icon.png"), false).ok()?;
        }
        Some(p.to_string_lossy().into_owned())
    })
    .clone()
}

/// Shows a notification (on a worker thread: talking to the daemon blocks, and so does waiting for a click).
pub fn notify(title: &str, body: &str, character: Option<i64>) {
    let (title, body) = (title.to_string(), body.to_string());
    std::thread::spawn(move || {
        let mut n = notify_rust::Notification::new();
        n.appname(config::APP_NAME).summary(&title).body(&body).action("default", "Open");
        if let Some(icon) = icon_path() {
            n.icon(&icon);
        }
        match n.show() {
            Ok(handle) => handle.wait_for_action(|action| {
                if action == "default" {
                    if let Some(tx) = CLICKS.get() {
                        let _ = tx.unbounded_send(character);
                    }
                }
            }),
            Err(e) => tracing::warn!("alerts: couldn't show notification: {e}"),
        }
    });
}

/// Settings → Alerts → Send test.
pub fn test() {
    notify("PodStack alerts are working", "You'll be notified like this when something needs your attention.", None);
}
