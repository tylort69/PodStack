//! Settings → Alerts: desktop notifications on/off, each alert type, the low-queue threshold, a test button and
//! the recent alerts.

use super::{settings_card, subhead};
use crate::ctx::{CharTab, Ctx, Route};
use crate::services;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::alerts::{self, AlertSettings, ALERT_TYPES, SETTINGS_KEY};
use podstack_core::{fmt, Event};
use std::rc::Rc;

fn update(ctx: &Ctx, f: impl FnOnce(&mut AlertSettings)) {
    let core = ctx.core();
    let mut s: AlertSettings = core.setting(SETTINGS_KEY);
    f(&mut s);
    core.set_setting(SETTINGS_KEY, &s);
}

fn fill_log(ctx: &Ctx, list: &gtk::Box, clear: &gtk::Button) {
    while let Some(c) = list.first_child() {
        list.remove(&c);
    }
    let log = alerts::load_state().log;
    clear.set_visible(!log.is_empty());
    if log.is_empty() {
        let none = w::text("None yet.", &["body-medium", "dim"]);
        none.set_margin_start(24);
        list.append(&none);
        return;
    }
    for entry in log.iter().take(10) {
        let row = gtk::Button::new();
        row.add_css_class("flat");
        row.add_css_class("settings-alert-row");
        let line = w::hbox(12);
        let texts = w::vbox(0);
        texts.set_hexpand(true);
        texts.append(&w::line(&entry.title, &["body-medium"]));
        texts.append(&w::line(&entry.body, &["body-small", "dim"]));
        line.append(&texts);
        line.append(&w::line(&fmt::relative(entry.time), &["body-small", "dim", "numeric"]));
        row.set_child(Some(&line));
        match entry.character_id.filter(|id| ctx.core().character(*id).is_some()) {
            Some(id) => {
                let ctx = ctx.clone();
                row.connect_clicked(move |_| ctx.navigate(Route::Character(id, CharTab::Summary)));
            }
            None => row.set_can_target(false),
        }
        list.append(&row);
    }
}

pub(super) fn card(ctx: &Ctx) -> gtk::Box {
    let s: AlertSettings = ctx.core().setting(SETTINGS_KEY);
    let test = w::button_icon("notifications_active", "Send test", Btn::Tonal);
    test.connect_clicked(|_| services::alerts::test());
    let card = settings_card(
        "notifications",
        "Alerts",
        Some("Desktop notifications through your notification daemon (mako, dunst, swaync, …)."),
        &[test.upcast()],
    );

    let types = w::vbox(0);
    types.set_sensitive(s.enabled);
    {
        let (ctx, types) = (ctx.clone(), types.clone());
        card.append(&w::switch_row(
            "Desktop notifications",
            Some("PodStack checks every minute, also while hidden in the tray, and alerts once per event. Clicking an alert shows PodStack."),
            s.enabled,
            move |on| {
                update(&ctx, |s| s.enabled = on);
                types.set_sensitive(on);
            },
        ));
    }

    types.append(&subhead("Alert me when"));
    let threshold = gtk::SpinButton::with_range(1.0, 240.0, 1.0);
    threshold.set_value(s.queue_low_hours as f64);
    threshold.set_sensitive(s.is_on("queue_low"));
    threshold.set_valign(gtk::Align::Center);
    let threshold = Rc::new(threshold);
    for t in ALERT_TYPES {
        let ctx = ctx.clone();
        let threshold = threshold.clone();
        types.append(&w::switch_row(t.label, Some(t.description), s.is_on(t.id), move |on| {
            update(&ctx, |s| s.set(t.id, on));
            if t.id == "queue_low" {
                threshold.set_sensitive(on);
            }
        }));
    }
    {
        let ctx = ctx.clone();
        threshold.connect_value_changed(move |b| {
            let h = b.value().round().clamp(1.0, 240.0) as u32;
            update(&ctx, |s| s.queue_low_hours = h);
        });
    }
    let hours = w::hbox(8);
    hours.append(&*threshold);
    hours.append(&w::line("hours", &["body-medium", "dim"]));
    types.append(&super::control_row("Running low below", Some("When \"Skill queue running low\" alerts."), &hours));
    card.append(&types);

    // recent alerts
    let head = w::hbox(8);
    let title = subhead("Recent alerts");
    title.set_hexpand(true);
    head.append(&title);
    let clear = w::button("Clear", Btn::Text);
    clear.set_margin_end(16);
    clear.set_valign(gtk::Align::End);
    head.append(&clear);
    card.append(&head);
    let list = w::vbox(0);
    card.append(&list);
    fill_log(ctx, &list, &clear);
    {
        let ctx = ctx.clone();
        clear.connect_clicked(move |_| alerts::clear_log(ctx.core()));
    }
    {
        let (ctx2, list2, clear2) = (ctx.clone(), list.clone(), clear.clone());
        ctx.on_event(&list, move |e| {
            if matches!(e, Event::Doc(d) if d == alerts::STATE_DOC) {
                fill_log(&ctx2, &list2, &clear2);
            }
        });
    }
    card
}
