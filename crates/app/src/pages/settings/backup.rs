//! Settings → Backup & restore: one password-protected file with everything in the data folder (EVE logins,
//! characters, plans, settings, …), to keep safe or move PodStack to another computer.

use super::{block, settings_card};
use crate::ctx::Ctx;
use crate::rt;
use crate::widgets::{self as w, Btn};
use gtk::{gio, glib, prelude::*};
use podstack_core::features::backup::{self, Backup, BackupError, MIN_PASSWORD_LENGTH};
use podstack_core::{fmt, store};
use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn filters() -> gio::ListStore {
    let ours = gtk::FileFilter::new();
    ours.set_name(Some("PodStack backups"));
    ours.add_suffix(backup::EXTENSION);
    let all = gtk::FileFilter::new();
    all.set_name(Some("All files"));
    all.add_pattern("*");
    let list = gio::ListStore::new::<gtk::FileFilter>();
    list.append(&ours);
    list.append(&all);
    list
}

fn password_entry(placeholder: &str) -> gtk::PasswordEntry {
    let e = gtk::PasswordEntry::new();
    e.set_show_peek_icon(true);
    e.set_property("placeholder-text", placeholder);
    e.set_hexpand(true);
    e
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
}

pub(super) fn card(ctx: &Ctx) -> gtk::Box {
    let card = settings_card(
        "backup",
        "Backup & restore",
        Some("Save your characters (with their EVE logins), skill plans, farms, alerts and settings to one password-protected file."),
        &[],
    );
    let b = block(12);
    b.append(&w::text(
        "The file is encrypted (Argon2id and AES-256-GCM) because it contains your characters' logins: keep the password, \
         it can't be recovered. Images and other caches aren't included; PodStack downloads them again.",
        &["body-medium", "dim"],
    ));
    let message = w::text("", &["body-medium", "success-text"]);
    message.set_visible(false);
    b.append(&message);
    let actions = w::hbox(8);
    actions.set_halign(gtk::Align::End);
    let restore = w::button_icon("settings_backup_restore", "Restore…", Btn::Outlined);
    let create = w::button_icon("backup", "Create backup…", Btn::Filled);
    actions.append(&restore);
    actions.append(&create);
    b.append(&actions);
    card.append(&b);

    {
        let (ctx, message) = (ctx.clone(), message.clone());
        create.connect_clicked(move |_| create_dialog(&ctx, &message));
    }
    {
        let ctx = ctx.clone();
        restore.connect_clicked(move |_| pick_restore(&ctx));
    }
    card
}

fn create_dialog(ctx: &Ctx, message: &gtk::Label) {
    let (win, content, actions) = w::dialog(ctx, "Create backup", 460);
    content.append(&w::text(
        &format!("Choose a password of at least {MIN_PASSWORD_LENGTH} characters. You'll need it to restore."),
        &["body-medium", "dim"],
    ));
    let pw = password_entry("Password");
    let again = password_entry("Repeat password");
    content.append(&pw);
    content.append(&again);
    let error = w::text("", &["body-medium", "error-text"]);
    error.set_visible(false);
    content.append(&error);
    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button("Save backup…", Btn::Filled);
    actions.append(&cancel);
    actions.append(&save);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    let go: Rc<dyn Fn()> = {
        let (ctx, win, pw, again, error, message, save) = (ctx.clone(), win.clone(), pw.clone(), again.clone(), error.clone(), message.clone(), save.clone());
        Rc::new(move || {
            let password = pw.text().to_string();
            let fail = |t: &str| {
                error.set_text(t);
                error.set_visible(true);
            };
            if password.chars().count() < MIN_PASSWORD_LENGTH {
                return fail(&BackupError::WeakPassword.to_string());
            }
            if password != again.text().as_str() {
                return fail("The passwords don't match.");
            }
            error.set_visible(false);
            let dialog = gtk::FileDialog::builder()
                .title("Save backup")
                .modal(true)
                .initial_name(format!("PodStack {}.{}", chrono::Local::now().format("%Y-%m-%d"), backup::EXTENSION))
                .filters(&filters())
                .build();
            let (ctx, win, error, message, save) = (ctx.clone(), win.clone(), error.clone(), message.clone(), save.clone());
            let parent = win.clone();
            dialog.save(Some(&parent), gio::Cancellable::NONE, move |r| {
                let Ok(file) = r else { return };
                let Some(mut path) = file.path() else { return };
                if path.extension().is_none() {
                    path.set_extension(backup::EXTENSION);
                }
                save.set_sensitive(false);
                save.set_label("Encrypting…");
                // pending saves first, so the backup has the latest data
                ctx.core().flush();
                let dest = path.clone();
                rt::run(
                    async move { tokio::task::spawn_blocking(move || backup::create(&store::data_dir(), &dest, &password)).await },
                    move |r| match r {
                        Ok(Ok(summary)) => {
                            let text = format!(
                                "Saved {} ({}) to {}.",
                                plural(summary.characters, "character"),
                                plural(summary.files, "file"),
                                file_name(&path)
                            );
                            message.set_text(&text);
                            message.set_visible(true);
                            ctx.toast("Backup saved");
                            win.close();
                        }
                        Ok(Err(e)) => {
                            save.set_sensitive(true);
                            save.set_label("Save backup…");
                            error.set_text(&format!("Couldn't save the backup: {e}"));
                            error.set_visible(true);
                        }
                        Err(e) => {
                            save.set_sensitive(true);
                            save.set_label("Save backup…");
                            error.set_text(&format!("Couldn't save the backup: {e}"));
                            error.set_visible(true);
                        }
                    },
                );
            });
        })
    };
    {
        let go = go.clone();
        save.connect_clicked(move |_| go());
    }
    {
        let go = go.clone();
        again.connect_activate(move |_| go());
    }
    {
        let again = again.clone();
        pw.connect_activate(move |_| {
            again.grab_focus();
        });
    }
    win.present();
    pw.grab_focus();
}

fn pick_restore(ctx: &Ctx) {
    let dialog = gtk::FileDialog::builder().title("Restore backup").modal(true).filters(&filters()).build();
    let ctx = ctx.clone();
    dialog.open(Some(&ctx.window()), gio::Cancellable::NONE, move |r| {
        if let Some(path) = r.ok().and_then(|f| f.path()) {
            unlock_dialog(&ctx, path);
        }
    });
}

fn unlock_dialog(ctx: &Ctx, path: PathBuf) {
    let (win, content, actions) = w::dialog(ctx, "Restore backup", 480);
    let body = w::vbox(12);
    content.append(&body);
    body.append(&w::text(&format!("Password for {}:", file_name(&path)), &["body-medium", "dim"]));
    let pw = password_entry("Password");
    body.append(&pw);
    let error = w::text("", &["body-medium", "error-text"]);
    error.set_visible(false);
    body.append(&error);
    let cancel = w::button("Cancel", Btn::Text);
    let open = w::button("Open", Btn::Filled);
    actions.append(&cancel);
    actions.append(&open);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    let opened = Rc::new(Cell::new(false));
    let go: Rc<dyn Fn()> = {
        let (ctx, pw, error, open, body, actions, win) = (ctx.clone(), pw.clone(), error.clone(), open.clone(), body.clone(), actions.clone(), win.clone());
        Rc::new(move || {
            if opened.get() {
                return;
            }
            let password = pw.text().to_string();
            let path = path.clone();
            open.set_sensitive(false);
            open.set_label("Checking…");
            error.set_visible(false);
            let (ctx, pw, error, open, body, actions, win, opened) =
                (ctx.clone(), pw.clone(), error.clone(), open.clone(), body.clone(), actions.clone(), win.clone(), opened.clone());
            rt::run(async move { tokio::task::spawn_blocking(move || backup::read(&path, &password)).await }, move |r| {
                let r = r.map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()));
                match r {
                    Ok(b) => {
                        opened.set(true);
                        confirm_view(&ctx, &win, &body, &actions, b);
                    }
                    Err(e) => {
                        open.set_sensitive(true);
                        open.set_label("Open");
                        error.set_text(&e);
                        error.set_visible(true);
                        pw.grab_focus();
                    }
                }
            });
        })
    };
    {
        let go = go.clone();
        open.connect_clicked(move |_| go());
    }
    pw.connect_activate(move |_| go());
    win.present();
    pw.grab_focus();
}

/// After the password checked out: what the backup holds, and the final confirmation.
fn confirm_view(ctx: &Ctx, win: &gtk::Window, body: &gtk::Box, actions: &gtk::Box, b: Backup) {
    while let Some(c) = body.first_child() {
        body.remove(&c);
    }
    while let Some(c) = actions.first_child() {
        actions.remove(&c);
    }
    body.append(&w::text(
        &format!(
            "Backup from {} ({}) with {} and {}.",
            fmt::datetime(b.created),
            b.app,
            plural(b.summary.characters, "character"),
            plural(b.summary.files, "file")
        ),
        &["body-medium"],
    ));
    body.append(&w::banner(
        "warning",
        "Restoring replaces all characters, logins, plans, farms and settings in PodStack with the backup's, then restarts PodStack.",
        w::BannerKind::Notice,
    ));
    let cancel = w::button("Cancel", Btn::Text);
    let go = w::button("Restore and restart", Btn::DangerFilled);
    actions.append(&cancel);
    actions.append(&go);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    let ctx = ctx.clone();
    go.connect_clicked(move |btn| {
        btn.set_sensitive(false);
        let core = ctx.core();
        // write pending data now, so nothing saved later overwrites the restored files
        core.flush();
        match backup::restore(&store::data_dir(), &b) {
            Ok(()) => relaunch(),
            Err(e) => {
                btn.set_sensitive(true);
                ctx.toast(&format!("Couldn't restore: {e}"));
            }
        }
    });
}

/// Starts PodStack again, which waits for this process to exit (`PODSTACK_WAIT_PID`), then exits right away so
/// nothing in memory is saved over the restored files.
fn relaunch() {
    match std::env::current_exe() {
        Ok(exe) => {
            let r = std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .env("PODSTACK_WAIT_PID", std::process::id().to_string())
                .spawn();
            if let Err(e) = r {
                tracing::error!("couldn't restart PodStack: {e}");
            }
        }
        Err(e) => tracing::error!("couldn't restart PodStack: {e}"),
    }
    glib::idle_add_once(|| std::process::exit(0));
}
