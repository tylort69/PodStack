//! The "Add character" dialog: EVE SSO in the browser, then either a loopback callback (own EVE application with
//! an http://127.0.0.1:<port>/… callback) or pasting the address the browser ended on.

use crate::ctx::{CharTab, Ctx, Route};
use crate::rt;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::{config, sso};
use std::rc::Rc;

pub fn show(ctx: &Ctx) {
    let core = ctx.core().clone();
    let (client_id, callback) = core.sso_client();
    let pkce = sso::Pkce::new();
    let url = sso::authorize_url(&client_id, &callback, &config::scope_names(), &pkce);
    let loopback = sso::loopback_port(&callback);

    let (win, content, actions) = w::dialog(ctx, "Add character", 520);
    content.append(&w::text(
        "PodStack opens EVE's login page in your browser. Log in, choose the character and authorize it. \
         Log in again for a character you already added to renew its login or grant new permissions.",
        &["body-medium", "dim"],
    ));

    let open = w::button_icon("open_in_new", "Open EVE login", Btn::Filled);
    open.set_halign(gtk::Align::Start);
    let copy = w::button_icon("content_copy", "Copy login link", Btn::Text);
    let row = w::hbox(8);
    row.append(&open);
    row.append(&copy);
    content.append(&row);

    let error = w::text("", &["body-medium", "error-text"]);
    error.set_visible(false);
    let status = w::text("", &["body-medium", "dim"]);
    status.set_visible(false);

    let finish: Rc<dyn Fn(String)> = {
        let ctx = ctx.clone();
        let win = win.clone();
        let error = error.clone();
        let status = status.clone();
        let state = pkce.state.clone();
        Rc::new(move |callback_url: String| {
            let code = match sso::code_from_callback(&callback_url, Some(&state)) {
                Ok(c) => c,
                Err(e) => {
                    error.set_text(&e);
                    error.set_visible(true);
                    return;
                }
            };
            error.set_visible(false);
            status.set_text("Adding character…");
            status.set_visible(true);
            let core = ctx.core().clone();
            let (client_id, pkce) = (client_id.clone(), pkce.clone());
            let ctx2 = ctx.clone();
            let win = win.clone();
            let error = error.clone();
            let status = status.clone();
            rt::run(async move { core.login(&client_id, &code, &pkce).await.map_err(|e| e.to_string()) }, move |r| match r {
                Ok((id, name)) => {
                    ctx2.toast(&format!("Added {name}. Loading data from ESI…"));
                    win.close();
                    ctx2.navigate(Route::Character(id, CharTab::Summary));
                }
                Err(e) => {
                    status.set_visible(false);
                    error.set_text(&format!("Login failed: {e}"));
                    error.set_visible(true);
                }
            });
        })
    };

    {
        let ctx = ctx.clone();
        let url = url.clone();
        open.connect_clicked(move |_| w::open_url(&ctx, &url));
    }
    {
        let ctx = ctx.clone();
        let url = url.clone();
        copy.connect_clicked(move |_| w::copy_to_clipboard(&ctx, &url));
    }

    let cancel = w::button("Cancel", Btn::Text);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    actions.append(&cancel);

    if let Some(port) = loopback {
        status.set_text("Waiting for the browser to finish logging in…");
        status.set_visible(true);
        let finish = finish.clone();
        let win_weak = win.downgrade();
        let error = error.clone();
        rt::run(
            async move { tokio::time::timeout(std::time::Duration::from_secs(600), sso::wait_for_loopback(port)).await },
            move |r| {
                if win_weak.upgrade().map(|w| w.is_visible()).unwrap_or(false) {
                    match r {
                        Ok(Ok(u)) => finish(u),
                        Ok(Err(e)) => {
                            error.set_text(&format!("Couldn't listen on port {port}: {e}"));
                            error.set_visible(true);
                        }
                        Err(_) => {
                            error.set_text("Timed out waiting for the browser.");
                            error.set_visible(true);
                        }
                    }
                }
            },
        );
    } else {
        content.append(&w::text(
            "After you authorize, the browser can't open the page it's sent to (https://localhost/callback…). That's \
             expected: copy the whole address from the address bar and paste it here.",
            &["body-small", "dim"],
        ));
        let entry = gtk::Entry::new();
        entry.set_placeholder_text(Some("https://localhost/callback?code=…"));
        content.append(&entry);
        let add = w::button("Add character", Btn::Filled);
        add.set_sensitive(false);
        {
            let add = add.clone();
            entry.connect_changed(move |e| add.set_sensitive(e.text().contains("code=")));
        }
        {
            let finish = finish.clone();
            let entry2 = entry.clone();
            add.connect_clicked(move |_| finish(entry2.text().to_string()));
        }
        {
            let finish = finish.clone();
            entry.connect_activate(move |e| finish(e.text().to_string()));
        }
        actions.append(&add);
    }
    content.append(&status);
    content.append(&error);
    win.present();
}
