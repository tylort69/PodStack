//! Settings → Alerts: where alerts go (desktop notifications, Discord, both or neither), setting up Discord
//! (a channel webhook, or a bot posting in a channel or by direct message), which alert types go to each, which
//! characters alert where, the low-queue threshold, and the recent alerts with where each was sent.

use super::settings_card;
use crate::ctx::{CharTab, Ctx, Route};
use crate::rt;
use crate::services;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::alerts::{self, AlertSettings, Channel, ALERT_TYPES, SETTINGS_KEY};
use podstack_core::features::discord::{self, DiscordConfig, Method};
use podstack_core::{fmt, Event};
use std::cell::RefCell;
use std::rc::Rc;

fn update(ctx: &Ctx, f: impl FnOnce(&mut AlertSettings)) {
    let core = ctx.core();
    let mut s: AlertSettings = core.setting(SETTINGS_KEY);
    f(&mut s);
    core.set_setting(SETTINGS_KEY, &s);
}

/// The Alerts tab's cards.
pub(super) fn cards(ctx: &Ctx) -> Vec<gtk::Widget> {
    let s: AlertSettings = ctx.core().setting(SETTINGS_KEY);
    // the per-type and per-character columns follow their channel's master switch
    let columns: Rc<RefCell<Vec<(Channel, gtk::Widget)>>> = Rc::default();
    let set_column = {
        let columns = columns.clone();
        move |ch: Channel, on: bool| {
            for (c, wdg) in columns.borrow().iter() {
                if *c == ch {
                    wdg.set_sensitive(on);
                }
            }
        }
    };
    let set_column = Rc::new(set_column);
    vec![
        channels_card(ctx, &s, set_column.clone()).upcast(),
        discord_card(ctx).upcast(),
        types_card(ctx, &s, &columns).upcast(),
        characters_card(ctx, &s, &columns).upcast(),
        recent_card(ctx).upcast(),
    ]
}

// ---------- channels ----------

fn channels_card(ctx: &Ctx, s: &AlertSettings, set_column: Rc<dyn Fn(Channel, bool)>) -> gtk::Box {
    let card = settings_card(
        "notifications",
        "Where alerts go",
        Some("PodStack checks every minute, also while hidden in the tray, and alerts once per event. Send each alert to the desktop, to Discord, to both or to neither: pick per alert type and per character below."),
        &[],
    );
    let test = w::button_icon("notifications_active", "Test", Btn::Text);
    test.connect_clicked(|_| services::alerts::test());
    {
        let (ctx, set_column) = (ctx.clone(), set_column.clone());
        let row = w::switch_row(
            "Desktop notifications",
            Some(if cfg!(windows) {
                "Windows notifications. Clicking one shows PodStack."
            } else {
                "Through your notification daemon (mako, dunst, swaync, …). Clicking one shows PodStack."
            }),
            s.enabled,
            move |on| {
                update(&ctx, |s| s.enabled = on);
                set_column(Channel::Desktop, on);
            },
        );
        row.insert_child_after(&test, row.first_child().as_ref());
        test.set_valign(gtk::Align::Center);
        card.append(&row);
    }
    let cfg = discord::load();
    let sub = if cfg.is_complete() {
        format!("To {} (set up below).", cfg.method.label().to_lowercase())
    } else {
        "Set up a webhook or a bot below first.".to_string()
    };
    {
        let ctx = ctx.clone();
        card.append(&w::switch_row("Discord", Some(&sub), s.discord, move |on| {
            update(&ctx, |s| s.discord = on);
            set_column(Channel::Discord, on);
        }));
    }
    card
}

// ---------- Discord setup ----------

fn secret_entry(text: &str, placeholder: &str) -> gtk::PasswordEntry {
    let e = gtk::PasswordEntry::new();
    e.set_show_peek_icon(true);
    e.set_placeholder_text(Some(placeholder));
    e.set_text(text);
    e.set_hexpand(true);
    e
}

fn plain_entry(text: &str, placeholder: &str) -> gtk::Entry {
    let e = gtk::Entry::new();
    e.set_placeholder_text(Some(placeholder));
    e.set_text(text);
    e.set_hexpand(true);
    e
}

fn help(text: &str) -> gtk::Label {
    let l = w::text(text, &["body-small", "dim"]);
    l.set_xalign(0.0);
    l.set_wrap(true);
    l
}

fn show_result(label: &gtk::Label, cfg: &DiscordConfig) {
    match &cfg.last {
        Some(r) => {
            label.set_text(&format!("{} · {}", r.message, fmt::relative(r.time)));
            label.set_css_classes(&["body-small", if r.ok { "success-text" } else { "error-text" }]);
        }
        None => {
            label.set_text("Nothing sent to Discord yet.");
            label.set_css_classes(&["body-small", "dim"]);
        }
    }
}

fn discord_card(ctx: &Ctx) -> gtk::Box {
    let cfg = discord::load();
    let card = settings_card(
        "forum",
        "Discord",
        Some("Send alerts to a Discord channel or to your DMs. The webhook URL and bot token stay on this computer, readable only by you."),
        &[],
    );
    let b = super::block(12);

    let method = Rc::new(RefCell::new(cfg.method));
    let labels: Vec<&str> = Method::ALL.iter().map(|m| m.label()).collect();
    let webhook = secret_entry(&cfg.webhook_url, "https://discord.com/api/webhooks/…");
    let token = secret_entry(&cfg.bot_token, "Bot token");
    let channel = plain_entry(&cfg.channel_id, "Channel ID, e.g. 112233445566778899");
    let user = plain_entry(&cfg.user_id, "Your user ID, e.g. 112233445566778899");
    let mention = plain_entry(&cfg.mention, "Optional: @here, <@your user id> or <@&role id>");
    let portraits = gtk::Switch::new();
    portraits.set_active(cfg.portraits);
    portraits.set_valign(gtk::Align::Center);

    let webhook_box = w::vbox(6);
    webhook_box.append(&webhook);
    webhook_box.append(&help(
        "In Discord: Server Settings (or the channel's Edit Channel) → Integrations → Webhooks → New Webhook → pick the channel → Copy Webhook URL. Anyone with the URL can post in that channel, so keep it private.",
    ));
    let bot_box = w::vbox(6);
    bot_box.append(&token);
    bot_box.append(&help(
        "Create an application at discord.com/developers → Bot → Reset Token and copy it. To post in a channel, invite the bot (OAuth2 → URL Generator → bot, with Send Messages and Embed Links). For direct messages, the bot must share a server with you, and your privacy settings must allow DMs from that server's members. Turn on Developer Mode (Settings → Advanced) to copy channel and user IDs.",
    ));
    let channel_box = w::vbox(6);
    channel_box.append(&channel);
    let user_box = w::vbox(6);
    user_box.append(&user);

    let show_method = {
        let (webhook_box, bot_box, channel_box, user_box) = (webhook_box.clone(), bot_box.clone(), channel_box.clone(), user_box.clone());
        move |m: Method| {
            webhook_box.set_visible(m == Method::Webhook);
            bot_box.set_visible(m != Method::Webhook);
            channel_box.set_visible(m == Method::BotChannel);
            user_box.set_visible(m == Method::BotDm);
        }
    };
    show_method(cfg.method);
    {
        let (method, show_method) = (method.clone(), show_method.clone());
        let seg = w::segmented(&labels, Method::ALL.iter().position(|m| *m == cfg.method).unwrap_or(0), move |i| {
            *method.borrow_mut() = Method::ALL[i];
            show_method(Method::ALL[i]);
        });
        seg.set_halign(gtk::Align::Start);
        b.append(&seg);
    }
    for x in [&webhook_box, &bot_box, &channel_box, &user_box] {
        b.append(x);
    }
    b.append(&mention);
    b.append(&help("A mention is put in front of every alert message, so Discord pings you or a role."));
    let row = w::hbox(12);
    let pl = w::text("Show the character's portrait with each alert", &["body-medium"]);
    pl.set_hexpand(true);
    pl.set_xalign(0.0);
    row.append(&pl);
    row.append(&portraits);
    b.append(&row);

    let result = w::text("", &["body-small"]);
    result.set_xalign(0.0);
    result.set_wrap(true);
    show_result(&result, &cfg);

    let read = {
        let (method, webhook, token, channel, user, mention, portraits) =
            (method.clone(), webhook.clone(), token.clone(), channel.clone(), user.clone(), mention.clone(), portraits.clone());
        move || DiscordConfig {
            method: *method.borrow(),
            webhook_url: webhook.text().trim().to_string(),
            bot_token: token.text().trim().to_string(),
            channel_id: channel.text().trim().to_string(),
            user_id: user.text().trim().to_string(),
            mention: mention.text().trim().to_string(),
            portraits: portraits.is_active(),
            last: discord::load().last,
        }
    };
    let read = Rc::new(read);

    let actions = w::hbox(8);
    actions.set_halign(gtk::Align::End);
    let test = w::button_icon("send", "Send test", Btn::Tonal);
    let save = w::button("Save", Btn::Filled);
    actions.append(&test);
    actions.append(&save);
    b.append(&actions);
    b.append(&result);
    card.append(&b);

    {
        let (ctx, read) = (ctx.clone(), read.clone());
        save.connect_clicked(move |_| {
            let c = read();
            discord::save(&c);
            ctx.toast(if c.is_complete() { "Discord settings saved" } else { "Saved, but Discord isn't fully set up yet" });
        });
    }
    {
        let (ctx, read, result) = (ctx.clone(), read.clone(), result.clone());
        test.connect_clicked(move |btn| {
            let c = read();
            discord::save(&c);
            btn.set_sensitive(false);
            result.set_text("Sending…");
            result.set_css_classes(&["body-small", "dim"]);
            let core = ctx.core().clone();
            let (btn, result, ctx) = (btn.clone(), result.clone(), ctx.clone());
            rt::run(
                async move {
                    let r = discord::test(&core.esi.http, &c).await;
                    let mut latest = discord::load();
                    latest.last = Some(discord::LastResult {
                        time: chrono::Utc::now(),
                        ok: r.is_ok(),
                        message: match &r {
                            Ok(()) => "Test message sent".into(),
                            Err(e) => e.clone(),
                        },
                    });
                    discord::save(&latest);
                    (r, latest)
                },
                move |(r, latest)| {
                    btn.set_sensitive(true);
                    show_result(&result, &latest);
                    if r.is_ok() {
                        ctx.toast("Test sent to Discord");
                    }
                },
            );
        });
    }
    // deliveries from the alerts service update the last result
    {
        let result2 = result.clone();
        ctx.on_event(&result, move |e| {
            if matches!(e, Event::Doc(d) if d == alerts::STATE_DOC) {
                show_result(&result2, &discord::load());
            }
        });
    }
    card
}

// ---------- per type and per character ----------

/// A grid with a Desktop and a Discord column of switches.
fn channel_grid() -> gtk::Grid {
    let g = gtk::Grid::new();
    g.set_column_spacing(24);
    g.set_row_spacing(14);
    g.add_css_class("settings-block");
    for (i, ch) in Channel::ALL.iter().enumerate() {
        let l = w::line(ch.label(), &["label-large", "dim"]);
        l.set_halign(gtk::Align::Center);
        g.attach(&l, 1 + i as i32, 0, 1, 1);
    }
    g
}

fn texts(title: &str, sub: Option<&str>) -> gtk::Box {
    let t = w::vbox(2);
    t.set_hexpand(true);
    t.set_valign(gtk::Align::Center);
    t.append(&w::text(title, &["body-large"]));
    if let Some(s) = sub {
        t.append(&w::text(s, &["body-small", "dim"]));
    }
    t
}

fn types_card(ctx: &Ctx, s: &AlertSettings, columns: &Rc<RefCell<Vec<(Channel, gtk::Widget)>>>) -> gtk::Box {
    let card = settings_card("tune", "Alert types", Some("Which alerts go to the desktop and which to Discord."), &[]);
    let g = channel_grid();
    let switches: Rc<RefCell<Vec<(Channel, gtk::Switch)>>> = Rc::default();
    let threshold = gtk::SpinButton::with_range(1.0, 240.0, 1.0);
    threshold.set_value(s.queue_low_hours as f64);
    threshold.set_valign(gtk::Align::Center);
    for (row, t) in ALERT_TYPES.iter().enumerate() {
        let r = row as i32 + 1;
        g.attach(&texts(t.label, Some(t.description)), 0, r, 1, 1);
        for (i, ch) in Channel::ALL.into_iter().enumerate() {
            let sw = gtk::Switch::new();
            sw.set_active(s.type_on(ch, t.id));
            sw.set_valign(gtk::Align::Center);
            sw.set_halign(gtk::Align::Center);
            sw.set_sensitive(s.channel_on(ch));
            sw.set_tooltip_text(Some(&format!("{} to {}", t.label, ch.label())));
            let ctx = ctx.clone();
            sw.connect_active_notify(move |sw| update(&ctx, |s| s.set_type(ch, t.id, sw.is_active())));
            g.attach(&sw, 1 + i as i32, r, 1, 1);
            columns.borrow_mut().push((ch, sw.clone().upcast()));
            switches.borrow_mut().push((ch, sw));
        }
    }
    card.append(&g);

    // all on / all off per channel
    let bulk = w::hbox(8);
    bulk.add_css_class("settings-block");
    for ch in Channel::ALL {
        for (on, label) in [(true, "All"), (false, "None")] {
            let b = w::button(&format!("{label} to {}", ch.label()), Btn::Text);
            let switches = switches.clone();
            b.connect_clicked(move |_| {
                for (c, sw) in switches.borrow().iter() {
                    if *c == ch {
                        sw.set_active(on);
                    }
                }
            });
            bulk.append(&b);
        }
    }
    card.append(&bulk);

    {
        let ctx = ctx.clone();
        threshold.connect_value_changed(move |b| {
            let h = b.value().round().clamp(1.0, 240.0) as u32;
            update(&ctx, |s| s.queue_low_hours = h);
        });
    }
    let hours = w::hbox(8);
    hours.append(&threshold);
    hours.append(&w::line("hours", &["body-medium", "dim"]));
    card.append(&super::control_row("Skill queue running low below", Some("When \"Skill queue running low\" alerts."), &hours));
    card
}

fn characters_card(ctx: &Ctx, s: &AlertSettings, columns: &Rc<RefCell<Vec<(Channel, gtk::Widget)>>>) -> gtk::Box {
    let card = settings_card(
        "group",
        "Characters",
        Some("Mute a character on a channel, e.g. keep an SP farm's alerts on the desktop only."),
        &[],
    );
    let chars = ctx.core().characters();
    if chars.is_empty() {
        let b = super::block(0);
        b.append(&w::text("Add a character to choose where its alerts go.", &["body-medium", "dim"]));
        card.append(&b);
        return card;
    }
    let g = channel_grid();
    for (row, c) in chars.iter().enumerate() {
        let r = row as i32 + 1;
        let who = w::hbox(12);
        who.set_hexpand(true);
        who.append(&w::avatar(&podstack_core::config::character_portrait(c.id, 64), 28));
        let n = w::line(&c.name(), &["body-large"]);
        n.set_valign(gtk::Align::Center);
        who.append(&n);
        g.attach(&who, 0, r, 1, 1);
        for (i, ch) in Channel::ALL.into_iter().enumerate() {
            let sw = gtk::Switch::new();
            sw.set_active(s.character_on(ch, c.id));
            sw.set_valign(gtk::Align::Center);
            sw.set_halign(gtk::Align::Center);
            sw.set_sensitive(s.channel_on(ch));
            let (ctx, id) = (ctx.clone(), c.id);
            sw.connect_active_notify(move |sw| update(&ctx, |s| s.set_character(ch, id, sw.is_active())));
            g.attach(&sw, 1 + i as i32, r, 1, 1);
            columns.borrow_mut().push((ch, sw.upcast()));
        }
    }
    card.append(&g);
    card
}

// ---------- recent ----------

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
        for ch in &entry.sent_to {
            let chip = w::status(ch, w::Status::Neutral);
            chip.set_valign(gtk::Align::Center);
            line.append(&chip);
        }
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

fn recent_card(ctx: &Ctx) -> gtk::Box {
    let clear = w::button("Clear", Btn::Text);
    let card = settings_card("history", "Recent alerts", Some("The last alerts, and where each was sent."), &[clear.clone().upcast()]);
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
