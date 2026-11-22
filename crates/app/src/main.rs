//! PodStack: an EVE Online character manager for Linux, built with GTK4 and Material You.

mod ctx;
mod fonts;
mod images;
mod login;
mod pages;
mod rt;
mod services;
mod shell;
mod theme;
mod widgets;

use gtk::{gio, glib, prelude::*};
use podstack_core::{config, Core, Event};

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_env("PODSTACK_LOG").unwrap_or_else(|_| "podstack=info,podstack_core=info".into()))
        .init();

    let core = Core::load();
    {
        let _guard = rt::RUNTIME.enter();
        core.start();
    }

    let app = gtk::Application::builder().application_id(config::APP_ID).flags(gio::ApplicationFlags::empty()).build();

    let core_s = core.clone();
    app.connect_startup(move |_| {
        fonts::register();
        theme::apply_settings(&core_s.settings().theme);
        // re-theme when settings change
        let core = core_s.clone();
        let mut rx = core.subscribe();
        glib::spawn_future_local(async move {
            let mut last = core.settings().theme;
            while let Ok(ev) = rx.recv().await {
                if ev == Event::SettingsChanged {
                    let t = core.settings().theme;
                    if t != last {
                        theme::apply_settings(&t);
                        last = t;
                    }
                }
            }
        });
    });

    let core_a = core.clone();
    app.connect_activate(move |app| {
        // a second launch just raises the window
        if let Some(win) = app.active_window().or_else(|| app.windows().into_iter().next()) {
            win.set_visible(true);
            win.present();
            return;
        }
        let win = shell::build(app, core_a.clone());
        win.present();
    });

    let code = app.run();
    core.flush();
    code
}
