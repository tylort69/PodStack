//! PodStack: an EVE Online character manager for Linux and Windows, built with GTK4 and Material You.

// No console window behind the app on Windows; logs go to a file there instead (see `init_logging`).
#![cfg_attr(windows, windows_subsystem = "windows")]

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
use podstack_core::{config, platform, Core, Event};

fn main() -> glib::ExitCode {
    // A restart (after restoring a backup) waits for the old copy to finish exiting before it claims anything.
    if let Some(pid) = std::env::var("PODSTACK_WAIT_PID").ok().and_then(|p| p.parse().ok()) {
        platform::wait_for_exit(pid);
    }
    platform::set_app_id(config::APP_ID);
    // Claimed before logging starts, so a second launch on Windows doesn't empty the running copy's log file.
    let raised = match platform::claim_instance(config::APP_ID) {
        platform::Instance::First { raised } => raised,
        platform::Instance::AlreadyRunning => return glib::ExitCode::SUCCESS,
    };
    init_logging();

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

    // Another launch asked this copy to show itself (Windows; on Linux GTK sends `activate` instead).
    if let Some(raised) = raised {
        let app = app.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(250), move || {
            if raised.try_recv().is_ok() {
                app.activate();
            }
            glib::ControlFlow::Continue
        });
    }

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
        if let Some(path) = std::env::var_os("PODSTACK_SNAPSHOT") {
            snapshot_and_quit(app, &win, path.into());
        }
    });

    let code = app.run();
    core.flush();
    code
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_env("PODSTACK_LOG").unwrap_or_else(|_| "podstack=info,podstack_core=info".into());
    // A Windows app without a console has nowhere to print, so it writes podstack.log in the cache folder.
    #[cfg(windows)]
    if let Ok(file) = std::fs::File::create(podstack_core::store::cache_dir().join("podstack.log")) {
        tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(std::sync::Mutex::new(file)).init();
        return;
    }
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// `PODSTACK_SNAPSHOT=<file.png>`: saves the window as an image once it has settled, then quits. CI uses it to check
/// that a build renders (fonts, icons, theme) on machines nobody can look at, without a desktop screenshot tool.
fn snapshot_and_quit(app: &gtk::Application, win: &gtk::ApplicationWindow, path: std::path::PathBuf) {
    let delay = std::env::var("PODSTACK_SNAPSHOT_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(8);
    let (app, win) = (app.clone(), win.clone());
    glib::timeout_add_seconds_local_once(delay, move || {
        // The window's content, not the window: a toplevel's own paintable draws nothing. A widget paintable only
        // holds an image after the widget's next frame, so ask for one and save it a moment later.
        let target: gtk::Widget = win.child().unwrap_or_else(|| win.clone().upcast());
        let paintable = gtk::WidgetPaintable::new(Some(&target));
        target.queue_draw();
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            let snapshot = gtk::Snapshot::new();
            paintable.snapshot(&snapshot, f64::from(target.width()), f64::from(target.height()));
            match (snapshot.to_node(), win.renderer()) {
                (Some(node), Some(renderer)) => match renderer.render_texture(node, None).save_to_png(&path) {
                    Ok(()) => tracing::info!("snapshot saved to {}", path.display()),
                    Err(e) => tracing::error!("couldn't save the snapshot: {e}"),
                },
                _ => tracing::error!("nothing to snapshot: the window hasn't rendered"),
            }
            app.quit();
        });
    });
}
