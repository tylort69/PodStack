//! One killmail in a dialog: the victim and its value, where and when, the fit and cargo with what dropped and
//! what was destroyed, and the attackers by damage with the user's characters highlighted.

use crate::ctx::Ctx;
use crate::pages::character::summary::parts::sec_label;
use crate::widgets::{self as w, Btn, Status};
use gtk::prelude::*;
use podstack_core::features::killboard::{self as kb, Archive, ArchivedKill, KillItem};
use podstack_core::{config, fmt};
use std::collections::HashSet;
use std::sync::Arc;

pub fn open(ctx: &Ctx, archive: Arc<Archive>, id: i64, mine: &HashSet<i64>) {
    let Some(k) = archive.get(id) else { return };
    let loss = k.mail.victim.character_id.is_some_and(|c| mine.contains(&c));
    let ship = archive.type_name(k.mail.victim.ship_type_id);
    let (win, content, actions) = w::dialog(ctx, &format!("{ship} · {}", archive.victim_name(k)), 820);
    win.set_default_height(760);

    let scroll = w::scrolled(&body(&archive, k, loss, mine));
    scroll.set_min_content_height(480);
    content.append(&scroll);

    let zkill = w::button_icon("open_in_new", "Open in zKillboard", Btn::Tonal);
    {
        let (ctx, url) = (ctx.clone(), kb::zkillboard_url(id));
        zkill.connect_clicked(move |_| w::open_url(&ctx, &url));
    }
    let close = w::button("Close", Btn::Text);
    {
        let win = win.clone();
        close.connect_clicked(move |_| win.close());
    }
    actions.append(&zkill);
    actions.append(&close);
    win.present();
}

/// The dialog's body: header, fit and attackers.
pub(super) fn body(a: &Archive, k: &ArchivedKill, loss: bool, mine: &HashSet<i64>) -> gtk::Box {
    let inner = w::vbox(16);
    inner.append(&header(a, k, loss));
    inner.append(&fit_section(a, k));
    inner.append(&attackers_section(a, k, mine));
    inner
}

fn header(a: &Archive, k: &ArchivedKill, loss: bool) -> gtk::Widget {
    let v = &k.mail.victim;
    let row = w::hbox(16);
    row.append(&w::eve_image(&config::type_render(v.ship_type_id, 256), 128));
    let side = w::vbox(6);
    side.set_hexpand(true);
    let title = w::hbox(8);
    title.append(&w::line(&a.victim_name(k), &["title-large"]));
    title.append(&if loss { w::status("Loss", Status::Bad) } else { w::status("Kill", Status::Good) });
    side.append(&title);
    let org: Vec<String> =
        [v.corporation_id, v.alliance_id, v.faction_id].into_iter().flatten().filter_map(|i| a.name(i).map(str::to_string)).collect();
    if !org.is_empty() {
        side.append(&w::line(&org.join(" · "), &["body-medium", "dim"]));
    }
    let (class, group) = kb::ship_class(v.ship_type_id);
    side.append(&w::line(&format!("{} · {group}", a.type_name(v.ship_type_id)), &["body-large"]));
    let place = w::hbox(8);
    if let Some(s) = k.security {
        place.append(&sec_label(s));
    }
    let region = k.region.as_deref().map(|r| format!(" · {r}")).unwrap_or_default();
    place.append(&w::line(&format!("{}{region} · {}", k.system, k.band().label()), &["body-medium"]));
    side.append(&place);
    side.append(&w::line(&format!("{} EVE time · {}", fmt::eve_time(k.time()), fmt::relative(k.time())), &["body-medium", "dim"]));
    if class != group {
        side.set_tooltip_text(Some(&class));
    }
    row.append(&side);

    let tiles = w::vbox(8);
    let total = match &k.value {
        Some(val) => {
            let t = w::stat_tile("Value", &format!("{} ISK", fmt::isk_short(val.total())), Some(&format!("hull {}", fmt::isk_short(val.hull))));
            t.set_tooltip_text(Some(&format!(
                "{} ISK, at CCP's average prices of {}{}",
                fmt::isk(val.total()),
                fmt::date(val.at),
                if val.unpriced > 0 { format!("\n{} types without a price count as nothing", val.unpriced) } else { String::new() }
            )));
            t
        }
        None => w::stat_tile("Value", "—", Some("not valued yet")),
    };
    total.add_css_class("accent");
    tiles.append(&total);
    if let Some(val) = &k.value {
        let split = w::hbox(8);
        split.append(&w::stat_tile("Destroyed", &fmt::isk_short(val.destroyed), None));
        split.append(&w::stat_tile("Dropped", &fmt::isk_short(val.dropped), None));
        tiles.append(&split);
    }
    row.append(&tiles);
    row.upcast()
}

fn fit_section(a: &Archive, k: &ArchivedKill) -> gtk::Widget {
    let fit = kb::fit(&k.mail);
    if fit.is_empty() {
        return w::card_with("Fit and cargo", None, &[], &w::text("Nothing was fitted or carried.", &["body-medium", "dim"])).upcast();
    }
    let list = w::vbox(12);
    for (slot, items) in fit {
        let group = w::vbox(2);
        group.append(&w::line(slot.label(), &["label-large", "primary-text"]));
        for i in items {
            item_rows(a, i, 0, &group);
        }
        list.append(&group);
    }
    let note = w::hbox(16);
    for (class, label) in [("error-text", "destroyed"), ("success-text", "dropped")] {
        let l = w::line(&format!("● {label}"), &["body-small", class]);
        l.set_ellipsize(gtk::pango::EllipsizeMode::None);
        note.append(&l);
    }
    w::card_with("Fit and cargo", Some("Quantities destroyed and dropped"), &[note.upcast()], &list).upcast()
}

/// An item and, indented below it, what was inside it.
fn item_rows(a: &Archive, i: &KillItem, depth: i32, into: &gtk::Box) {
    let row = w::hbox(10);
    row.add_css_class("kl-item");
    row.set_margin_start(depth * 24);
    row.append(&w::eve_image(&config::type_icon(i.item_type_id, 32), 24));
    let mut name = a.type_name(i.item_type_id);
    if i.is_blueprint_copy() {
        name.push_str(" (copy)");
    }
    let l = w::line(&name, &["body-medium"]);
    l.set_hexpand(true);
    l.set_xalign(0.0);
    row.append(&l);
    if i.destroyed() > 0 {
        row.append(&w::line(&format!("× {}", fmt::int(i.destroyed())), &["numeric", "error-text"]));
    }
    if i.dropped() > 0 {
        row.append(&w::line(&format!("× {}", fmt::int(i.dropped())), &["numeric", "success-text"]));
    }
    into.append(&row);
    for c in &i.items {
        item_rows(a, c, depth + 1, into);
    }
}

fn attackers_section(a: &Archive, k: &ArchivedKill, mine: &HashSet<i64>) -> gtk::Widget {
    let mut list: Vec<&kb::Attacker> = k.mail.attackers.iter().collect();
    list.sort_by_key(|x| (!x.final_blow, std::cmp::Reverse(x.damage_done)));
    let total: i64 = k.mail.victim.damage_taken.max(list.iter().map(|x| x.damage_done).sum::<i64>()).max(1);
    let rows = w::vbox(4);
    for x in &list {
        let row = w::hbox(12);
        row.add_css_class("kl-attacker");
        if x.character_id.is_some_and(|c| mine.contains(&c)) {
            row.add_css_class("kl-mine");
        }
        let portrait = match x.character_id {
            Some(c) => w::avatar(&config::character_portrait(c, 64), 32),
            None => match x.corporation_id {
                Some(c) => w::eve_image(&config::corporation_logo(c, 64), 32).upcast(),
                None => w::icon("smart_toy").upcast(),
            },
        };
        row.append(&portrait);
        if let Some(s) = x.ship_type_id {
            let img = w::eve_image(&config::type_icon(s, 32), 32);
            img.set_tooltip_text(Some(&a.type_name(s)));
            row.append(&img);
        }
        let texts = w::vbox(2);
        texts.set_hexpand(true);
        let top = w::hbox(8);
        top.append(&w::line(&a.attacker_name(x), &["body-medium"]));
        if x.final_blow {
            top.append(&w::status("Final blow", Status::Info));
        }
        if x.character_id.is_some_and(|c| mine.contains(&c)) {
            top.append(&w::status("Yours", Status::Good));
        }
        texts.append(&top);
        let org: Vec<String> = [x.corporation_id, x.alliance_id].into_iter().flatten().filter_map(|i| a.name(i).map(str::to_string)).collect();
        let ship = x.ship_type_id.map(|s| a.type_name(s));
        let weapon = x.weapon_type_id.filter(|wt| Some(*wt) != x.ship_type_id).map(|wt| a.type_name(wt));
        let second: Vec<String> = [ship, weapon].into_iter().flatten().chain(org).collect();
        texts.append(&w::line(&second.join(" · "), &["body-small", "dim"]));
        row.append(&texts);
        let dmg = w::vbox(2);
        dmg.set_valign(gtk::Align::Center);
        let d = w::line(&fmt::int(x.damage_done), &["body-medium", "numeric"]);
        d.set_xalign(1.0);
        dmg.append(&d);
        let p = w::line(&format!("{:.1}%", x.damage_done as f64 / total as f64 * 100.0), &["body-small", "dim", "numeric"]);
        p.set_xalign(1.0);
        dmg.append(&p);
        row.append(&dmg);
        rows.append(&row);
    }
    let players = list.iter().filter(|x| !x.is_npc()).count();
    let subtitle = format!(
        "{} {} · {} damage taken{}",
        list.len(),
        if list.len() == 1 { "attacker" } else { "attackers" },
        fmt::int(k.mail.victim.damage_taken),
        if players < list.len() { format!(" · {} NPC", list.len() - players) } else { String::new() }
    );
    w::card_with("Attackers", Some(&subtitle), &[], &rows).upcast()
}
