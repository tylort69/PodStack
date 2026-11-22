//! Character tab: ESI. Which scopes the login grants, the token's health, when each kind of data was last and will
//! next be refreshed (and why it failed), logging in again, and removing the character.

use super::summary::parts::{self, card, columns, kv_grid};
use crate::ctx::{Ctx, Page};
use crate::rt;
use crate::widgets::{self as w, Btn, Status};
use gtk::prelude::*;
use podstack_core::character::RefreshState;
use podstack_core::{config, domains, fmt};

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("ESI", body) };
    let Some(auth) = ctx.core().auth.get(id) else {
        body.append(&w::empty_state("vpn_key_off", "Not logged in", "This character has no EVE login."));
        return Page::new("ESI", body);
    };
    let name = c.name();

    // scopes
    let missing = auth.missing_scopes();
    let scopes = w::vbox(0);
    if !missing.is_empty() {
        let n = missing.len();
        let legacy = if auth.uses_legacy_client() { " (this character was added with the older Cerebral login)" } else { "" };
        let b = w::banner(
            "lock",
            &format!(
                "{n} permission{} missing, so some data can't load{legacy}. Use Log in again and log in as {name} on the EVE \
                 page that opens.",
                if n == 1 { " is" } else { "s are" }
            ),
            w::BannerKind::Notice,
        );
        b.set_margin_bottom(8);
        scopes.append(&b);
    }
    for s in config::SCOPES {
        let granted = auth.has_scope(s.name);
        let row = w::hbox(12);
        row.add_css_class("cx-scope-row");
        let texts = w::vbox(2);
        texts.set_hexpand(true);
        texts.append(&w::line(s.description, &["body-medium"]));
        texts.append(&w::line(s.name, &["body-small", "dim"]));
        row.append(&texts);
        row.append(&if granted { w::status("Granted", Status::Good) } else { w::status("Missing", Status::Bad) });
        scopes.append(&row);
    }
    let login = w::button_icon(if missing.is_empty() { "login" } else { "add_moderator" }, "Log in again", if missing.is_empty() { Btn::Outlined } else { Btn::Filled });
    login.set_tooltip_text(Some(&format!("Log in with EVE again as {name} to renew the login or grant missing permissions")));
    {
        let ctx = ctx.clone();
        login.connect_clicked(move |_| crate::login::show(&ctx));
    }
    let granted = config::SCOPES.len() - missing.len();
    let scope_card = card("Scopes Granted", "verified_user", Some(&format!("{granted} / {}", config::SCOPES.len())), &[login.upcast()], &scopes);

    // token health
    let now = chrono::Utc::now();
    let app: gtk::Widget = if auth.uses_legacy_client() {
        let s = w::status("Cerebral (older login)", Status::Warn);
        s.set_tooltip_text(Some("Added before PodStack had its own EVE application. Log in again to move it over."));
        s.upcast()
    } else if auth.client_id == config::DEFAULT_CLIENT_ID {
        w::status("PodStack", Status::Good).upcast()
    } else {
        let s = w::status("Your own application", Status::Info);
        s.set_tooltip_text(Some(&format!("Client id {}", auth.client_id)));
        s.upcast()
    };
    let token: gtk::Widget = if auth.needs_reauth() {
        w::status("Dead — log in again", Status::Bad).upcast()
    } else {
        w::status("OK", Status::Good).upcast()
    };
    let expiry = if auth.access_token_expiry > now {
        format!("in {}", fmt::duration(auth.access_token_expiry - now, 2))
    } else {
        format!("expired {} (renewed when needed)", fmt::relative(auth.access_token_expiry))
    };
    let mut rows: Vec<(&str, gtk::Widget)> = vec![
        ("EVE application", app),
        ("Token status", token),
        ("Access token", w::line("••••••••", &["body-medium", "dim"]).upcast()),
        ("Token expiry", w::line(&expiry, &["body-medium"]).upcast()),
        ("Refresh token", w::line("••••••••", &["body-medium", "dim"]).upcast()),
    ];
    if let Some(r) = &auth.last_refresh {
        let row = w::hbox(8);
        row.append(&if r.success { w::status("Success", Status::Good) } else { w::status("Failure", Status::Bad) });
        row.append(&w::line(&fmt::relative(r.date), &["body-medium", "dim"]));
        if let Some(e) = &r.error {
            row.set_tooltip_text(Some(e));
        }
        rows.push(("Last token refresh", row.upcast()));
        if let Some(e) = r.error.as_ref().filter(|_| !r.success) {
            rows.push(("Error", w::text(e, &["body-small", "error-text"]).upcast()));
        }
    }
    rows.push(("Added", w::line(&fmt::datetime(auth.added), &["body-medium"]).upcast()));
    let health = card("API Health", "monitor_heart", None, &[], &kv_grid(rows));

    // data refresh
    let refresh_rows: Vec<(&'static str, Option<RefreshState>)> =
        domains::all().into_iter().map(|d| (d.label, c.refreshes.get(d.key).cloned())).collect();
    let table = w::Table::new(refresh_rows)
        .text("Data", |(label, _)| label.to_string())
        .expand()
        .widget_sorted(
            "Last",
            |(_, r)| {
                let (t, class) = last_text(r.as_ref());
                let l = w::line(&t, &["body-medium", class]);
                l.set_xalign(1.0);
                l.set_tooltip_text(r.as_ref().and_then(|r| r.message.as_deref()));
                l.upcast()
            },
            |a, b| a.1.as_ref().and_then(|r| r.last).cmp(&b.1.as_ref().and_then(|r| r.last)),
        )
        .width(140)
        .widget_sorted(
            "Next",
            |(_, r)| {
                let l = w::line(&next_text(r.as_ref()), &["body-medium", "dim", "numeric"]);
                l.set_xalign(1.0);
                l.upcast()
            },
            |a, b| a.1.as_ref().and_then(|r| r.next).cmp(&b.1.as_ref().and_then(|r| r.next)),
        )
        .width(140)
        .natural_height()
        .build();
    let sub = match (c.last_updated(), c.next_auto_refresh()) {
        (Some(l), Some(n)) if n > now => format!("Updated {} · next in {}", fmt::relative(l), fmt::duration(n - now, 2)),
        (Some(l), _) => format!("Updated {}", fmt::relative(l)),
        _ => "Not refreshed yet".to_string(),
    };
    let data = card("Data Refresh", "sync", Some(&sub), &[], &table);

    // remove
    let remove_body = w::vbox(12);
    remove_body.append(&w::text(
        "Signs this character out of PodStack with EVE and deletes its data and skill plans from this computer. You can add \
         it again later with Add character.",
        &["body-medium", "dim"],
    ));
    let remove = w::button_icon("person_remove", "Remove character", Btn::DangerFilled);
    remove.set_halign(gtk::Align::Start);
    {
        let ctx = ctx.clone();
        let name = name.clone();
        remove.connect_clicked(move |b| {
            let core = ctx.core().clone();
            let (ctx2, name2, button) = (ctx.clone(), name.clone(), b.clone());
            w::confirm(
                &ctx,
                "Remove character",
                &format!(
                    "Remove {name} from PodStack?\n\nThis signs the character out of PodStack with EVE and deletes its data and \
                     skill plans from this computer."
                ),
                "Remove",
                true,
                move || {
                    button.set_sensitive(false);
                    let core = core.clone();
                    let (ctx, name) = (ctx2.clone(), name2.clone());
                    rt::run(async move { core.remove_character(id).await }, move |_| ctx.toast(&format!("{name} was removed")));
                },
            );
        });
    }
    remove_body.append(&remove);
    let remove_card = card("Remove Character", "person_remove", None, &[], &remove_body);

    body.append(&columns(vec![scope_card.upcast()], vec![health.upcast(), data.upcast(), remove_card.upcast()]));
    Page::new("ESI", body)
}

/// When data was last refreshed, or why it failed (with a style class).
fn last_text(r: Option<&RefreshState>) -> (String, &'static str) {
    let Some(r) = r else { return ("Not yet".into(), "dim") };
    match r.error.as_deref() {
        None => match r.last {
            Some(l) if chrono::Utc::now() - l < chrono::Duration::seconds(5) => ("Just now".into(), "dim"),
            Some(l) => (format!("{} ago", fmt::time_since(l)), "dim"),
            None => ("Not yet".into(), "dim"),
        },
        Some("scope") => ("No scope".into(), "warning-text"),
        Some("token") => ("Token invalid".into(), "error-text"),
        Some("client") => ("Client invalid".into(), "error-text"),
        Some(_) => ("Error".into(), "error-text"),
    }
}

fn next_text(r: Option<&RefreshState>) -> String {
    let Some(r) = r else { return "Due".into() };
    match r.next {
        Some(n) if n > chrono::Utc::now() => fmt::time_until(n),
        Some(_) => "Due".into(),
        None => "Never".into(),
    }
}
