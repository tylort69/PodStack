//! Character tab: summary. Pilot dossier, attributes, jump clones, skill queue, jump fatigue, active implants, the
//! cerebral accelerator panel and loyalty points (with what they're worth in each corporation's LP store).
//!
//! Also holds [`parts`], small pieces the other character tabs of this area share.

use crate::ctx::{Ctx, Page};
use crate::rt;
use crate::widgets::{self as w, Status};
use gtk::prelude::*;
use parts::{card, collapsible, columns, dim_text, fit, kv_grid, sec_label, type_cell};
use podstack_core::features::character_extras as extras;
use podstack_core::sde::Attribute;
use podstack_core::training::roman;
use podstack_core::universe::Place;
use podstack_core::{config, fmt, Character};
use std::cell::RefCell;
use std::collections::HashSet;

/// Highest value an attribute can reach with a full remap and +5 implants.
const MAX_ATTRIBUTE: f64 = 32.0;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    parts::add_css();
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Summary", body) };

    let mut left: Vec<gtk::Widget> = vec![dossier(&c).upcast(), attributes(&c).upcast(), jump_clones(&c).upcast()];
    let mut right: Vec<gtk::Widget> = vec![skill_queue(&c).upcast()];
    if let Some(f) = fatigue(&c) {
        right.push(f.upcast());
    }
    right.push(implants(&c).upcast());
    if let Some(p) = crate::pages::character::plans::panels::accelerator_panel(ctx, id) {
        right.push(p);
    }
    if c.loyalty.is_some() {
        right.push(loyalty_panel(ctx, &c).upcast());
    } else {
        left.push(card("Loyalty Points", "stars", None, &[], &parts::pending(&c, "loyalty_points", "esi-characters.read_loyalty.v1", "loyalty points")).upcast());
    }
    body.append(&columns(left, right));
    Page::new("Summary", body)
}

fn not_loaded() -> gtk::Label {
    dim_text("Not loaded yet")
}

fn describe_place(p: &Place) -> String {
    let kind = match p.kind.as_str() {
        "station" => "Station",
        "structure" => "Structure",
        "system" => "System",
        _ => "Unknown",
    };
    format!("{} ({kind})", p.name)
}

fn dossier(c: &Character) -> gtk::Box {
    let info = c.info.as_ref();
    let mut rows: Vec<(&str, gtk::Widget)> = vec![("Character ID", w::line(&c.id.to_string(), &["body-medium", "numeric"]).upcast())];
    rows.push((
        "Date of birth",
        match info.and_then(|i| i.birthday) {
            Some(b) => w::line(&fmt::datetime(b), &["body-medium"]).upcast(),
            None => not_loaded().upcast(),
        },
    ));
    rows.push((
        "Security status",
        match info.and_then(|i| i.security_status) {
            Some(s) => w::line(&format!("{s:.2}"), &["body-medium", "numeric", if s < 0.0 { "error-text" } else { "primary-text" }]).upcast(),
            None => not_loaded().upcast(),
        },
    ));
    rows.push((
        "Wallet",
        match c.wallet {
            Some(v) => w::line(&fmt::isk(v), &["body-medium", "numeric"]).upcast(),
            None => not_loaded().upcast(),
        },
    ));
    rows.push((
        "Corporation",
        match info.and_then(|i| i.corporation.as_ref()) {
            Some(corp) if !corp.ticker.is_empty() => w::line(&format!("{} [{}]", corp.name, corp.ticker), &["body-medium"]).upcast(),
            Some(corp) => w::line(&corp.name, &["body-medium"]).upcast(),
            None => not_loaded().upcast(),
        },
    ));
    rows.push((
        "Alliance",
        match c.alliance_name() {
            Some(a) => w::line(a, &["body-medium"]).upcast(),
            None => dim_text("None").upcast(),
        },
    ));

    let mut place_rows: Vec<(&str, gtk::Widget)> = vec![];
    match &c.location {
        Some(loc) => {
            let sys = w::hbox(8);
            if let Some(s) = loc.system.security {
                sys.append(&sec_label(s));
            }
            sys.append(&w::line(&loc.system.name, &["body-medium"]));
            if let Some(r) = &loc.system.region_name {
                sys.append(&w::line(r, &["body-medium", "dim"]));
            }
            place_rows.push(("Solar system", sys.upcast()));
            place_rows.push((
                "Location",
                match &loc.docked {
                    Some(d) => w::line(&describe_place(d), &["body-medium"]).upcast(),
                    None => w::line("In space", &["body-medium"]).upcast(),
                },
            ));
        }
        None => {
            place_rows.push(("Solar system", not_loaded().upcast()));
            place_rows.push(("Location", not_loaded().upcast()));
        }
    }
    place_rows.push((
        "Active ship",
        match &c.ship {
            Some(s) => w::line(&format!("{} ({})", s.ship_name, s.type_name), &["body-medium"]).upcast(),
            None => not_loaded().upcast(),
        },
    ));
    place_rows.push((
        "Home station",
        match c.clones.as_ref().map(|cl| cl.home.as_ref()) {
            Some(Some(h)) => w::line(&describe_place(h), &["body-medium"]).upcast(),
            Some(None) => dim_text("Unknown").upcast(),
            None => not_loaded().upcast(),
        },
    ));

    let b = w::vbox(12);
    b.append(&kv_grid(rows));
    b.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    b.append(&kv_grid(place_rows));
    card("Pilot Dossier", "badge", None, &[], &b)
}

fn attributes(c: &Character) -> gtk::Box {
    let Some(a) = &c.attributes else {
        return card("Attributes", "tune", None, &[], &parts::pending(c, "attributes", "esi-skills.read_skills.v1", "attributes"));
    };
    let b = w::vbox(12);
    let g = gtk::Grid::new();
    g.set_column_spacing(16);
    g.set_row_spacing(10);
    for (i, attr) in [Attribute::Intelligence, Attribute::Memory, Attribute::Perception, Attribute::Willpower, Attribute::Charisma]
        .into_iter()
        .enumerate()
    {
        let v = a.get(attr);
        g.attach(&w::line(attr.name(), &["body-medium"]), 0, i as i32, 1, 1);
        let bar = w::progress(v as f64 / MAX_ATTRIBUTE);
        bar.set_hexpand(true);
        g.attach(&bar, 1, i as i32, 1, 1);
        let l = fit(&v.to_string(), &["title-small", "numeric"]);
        l.set_xalign(1.0);
        g.attach(&l, 2, i as i32, 1, 1);
    }
    b.append(&g);
    b.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let remap: gtk::Widget = match c.next_remap_date() {
        Some(d) => w::line(&format!("{} ({})", fmt::datetime(d), fmt::relative(d)), &["body-medium"]).upcast(),
        None => w::status("Available", Status::Good).upcast(),
    };
    b.append(&kv_grid(vec![
        ("Unallocated SP", w::line(&fmt::int(c.unallocated_sp()), &["body-medium", "numeric"]).upcast()),
        ("Bonus remaps", w::line(&a.bonus_remaps.unwrap_or(0).to_string(), &["body-medium", "numeric"]).upcast()),
        ("Next yearly remap", remap),
    ]));
    card("Attributes", "tune", None, &[], &b)
}

fn clone_jump_row(c: &Character) -> gtk::Box {
    let row = w::hbox(12);
    let l = w::line("Clone jump available", &["body-medium", "dim"]);
    l.set_hexpand(true);
    row.append(&l);
    match extras::clone_jump_wait(c, chrono::Utc::now()) {
        None => row.append(&w::status("Now", Status::Good)),
        Some(d) => {
            let v = fit(&fmt::time_until(d), &["body-medium", "numeric"]);
            v.set_tooltip_text(Some(&fmt::datetime(d)));
            row.append(&v);
        }
    }
    row
}

fn jump_clones(c: &Character) -> gtk::Box {
    let Some(clones) = &c.clones else {
        return card("Jump Clones", "people_outline", None, &[], &parts::pending(c, "clones", "esi-clones.read_clones.v1", "jump clones"));
    };
    let b = w::vbox(12);
    b.append(&clone_jump_row(c));
    if clones.jump_clones.is_empty() {
        b.append(&dim_text("No jump clones"));
    }
    for jc in &clones.jump_clones {
        let item = w::vbox(6);
        item.add_css_class("cx-subtle");
        let head = w::hbox(8);
        head.append(&w::line(jc.name.as_deref().filter(|n| !n.is_empty()).unwrap_or("Unnamed Clone"), &["title-small"]));
        if let Some(s) = jc.place.security {
            head.append(&sec_label(s));
        }
        let place = w::line(&jc.place.name, &["body-medium", "dim"]);
        place.set_hexpand(true);
        head.append(&place);
        item.append(&head);
        if jc.implants.is_empty() {
            item.append(&w::line("No implants", &["body-small", "dim"]));
        } else {
            let icons = w::hbox(6);
            for imp in &jc.implants {
                let img = w::eve_image(&config::type_icon(imp.type_id, 64), 28);
                img.set_tooltip_text(Some(&imp.name));
                icons.append(&img);
            }
            item.append(&icons);
        }
        b.append(&item);
    }
    let sub = format!("{} / {}", clones.jump_clones.len(), c.max_jump_clones());
    card("Jump Clones", "people_outline", Some(&sub), &[], &b)
}

fn skill_queue(c: &Character) -> gtk::Box {
    let now = chrono::Utc::now();
    let current = c.current_skill();
    let pending: Vec<_> = c.queue().iter().filter(|q| q.finish_date.map(|f| f >= now).unwrap_or(false)).collect();
    // a paused queue has no dates
    let paused: Vec<_> = if current.is_none() { c.queue().iter().filter(|q| q.finish_date.is_none()).collect() } else { vec![] };

    let b = w::vbox(4);
    if c.skill_queue.is_none() {
        b.append(&parts::pending(c, "skill_queue", "esi-skills.read_skillqueue.v1", "the skill queue"));
    } else if pending.is_empty() && paused.is_empty() {
        b.append(&dim_text("No skills in queue"));
    }
    for q in &pending {
        let is_current = current.map(|cur| cur.queue_position == q.queue_position).unwrap_or(false);
        let item = w::vbox(6);
        item.add_css_class("cx-queue-item");
        if is_current {
            item.add_css_class("current");
        }
        let row = w::hbox(8);
        let name = w::line(&format!("{} {}", q.skill_name, roman(q.finished_level)), &["body-medium"]);
        name.set_hexpand(true);
        row.append(&name);
        let time = if is_current {
            q.finish_date.map(fmt::time_until).unwrap_or_default()
        } else {
            extras::queue_entry_length(q, now).map(|d| fmt::duration(d, 3)).unwrap_or_default()
        };
        let tl = fit(&time, &["body-medium", "numeric", "dim"]);
        if let Some(f) = q.finish_date {
            tl.set_tooltip_text(Some(&format!("Finishes {}", fmt::datetime(f))));
        }
        row.append(&tl);
        item.append(&row);
        if is_current {
            let bar = w::progress(extras::training_progress(q, now));
            item.append(&bar);
            let q = (*q).clone();
            let (tl2, bar2) = (tl.clone(), bar.clone());
            w::ticker(&tl, 1, move || {
                let now = chrono::Utc::now();
                if let Some(f) = q.finish_date {
                    tl2.set_text(&fmt::time_until(f));
                }
                bar2.set_fraction(extras::training_progress(&q, now));
            });
        }
        b.append(&item);
    }
    for q in &paused {
        let row = w::hbox(8);
        row.add_css_class("cx-queue-item");
        let name = w::line(&format!("{} {}", q.skill_name, roman(q.finished_level)), &["body-medium"]);
        name.set_hexpand(true);
        row.append(&name);
        row.append(&w::line("Paused", &["body-medium", "dim"]));
        b.append(&row);
    }

    let action: Vec<gtk::Widget> = match (current, c.queue_end()) {
        (Some(_), Some(end)) => {
            let l = fit(&fmt::time_until(end), &["title-small", "numeric", "primary-text"]);
            l.set_tooltip_text(Some(&format!("Queue ends {}", fmt::datetime(end))));
            vec![l.upcast()]
        }
        _ => vec![w::status("Not training", Status::Warn).upcast()],
    };
    card("Skill Queue", "schedule", None, &action, &b)
}

fn fatigue(c: &Character) -> Option<gtk::Box> {
    let f = extras::fatigue_info(c, chrono::Utc::now())?;
    let g = gtk::Grid::new();
    g.set_column_spacing(24);
    g.set_row_spacing(10);
    let rows: [(&str, String, Option<chrono::DateTime<chrono::Utc>>, &str); 3] = [
        ("Last jump", format!("{} ago", fmt::time_since(f.last_jump)), Some(f.last_jump), ""),
        ("Red timer", f.red_timer.map(fmt::time_until).unwrap_or_else(|| "None".into()), f.red_timer, "error-text"),
        ("Blue timer", f.blue_timer.map(fmt::time_until).unwrap_or_else(|| "None".into()), f.blue_timer, "primary-text"),
    ];
    for (i, (label, value, date, class)) in rows.into_iter().enumerate() {
        g.attach(&fit(label, &["body-medium", "dim"]), 0, i as i32, 1, 1);
        let mut classes = vec!["body-medium", "numeric"];
        if date.is_some() && !class.is_empty() {
            classes.push(class);
        }
        let v = fit(&value, &classes);
        v.set_hexpand(true);
        g.attach(&v, 1, i as i32, 1, 1);
        if let Some(d) = date {
            let dl = fit(&fmt::datetime(d), &["body-small", "dim"]);
            dl.set_xalign(1.0);
            g.attach(&dl, 2, i as i32, 1, 1);
        }
    }
    Some(card("Jump Fatigue", "flash_on", None, &[], &g))
}

fn implants(c: &Character) -> gtk::Box {
    let Some(list) = &c.implants else {
        return card("Active Implants", "memory", None, &[], &parts::pending(c, "implants", "esi-clones.read_implants.v1", "implants"));
    };
    let b = w::vbox(8);
    if list.is_empty() {
        b.append(&dim_text("No active implants"));
    }
    for imp in list {
        let sub = imp.slot.map(|s| format!("Slot {s}"));
        b.append(&type_cell(&w::eve_image(&config::type_icon(imp.type_id, 64), 28).upcast(), &imp.name, sub.as_deref()));
    }
    card("Active Implants", "memory", Some(&format!("{} / 10", list.len())), &[], &b)
}

// ---------- loyalty points ----------

thread_local! {
    /// Corporations whose LP store is being priced, so a page rebuild doesn't start the same load again.
    static LP_LOADING: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
}

fn loyalty_panel(ctx: &Ctx, c: &Character) -> gtk::Box {
    let holder = w::vbox(0);
    fill_loyalty(ctx, &holder, c.id);
    let points = c.loyalty.as_ref().map(|l| l.points.clone()).unwrap_or_default();
    for lp in points.iter().filter(|lp| extras::cached_best_offers(lp.corporation_id).is_none()) {
        let corp = lp.corporation_id;
        if LP_LOADING.with(|l| !l.borrow_mut().insert(corp)) {
            continue;
        }
        let core = ctx.core().clone();
        let (ctx, holder, id) = (ctx.clone(), holder.clone(), c.id);
        rt::run(async move { extras::best_offers(&core, corp).await }, move |_| {
            LP_LOADING.with(|l| l.borrow_mut().remove(&corp));
            if holder.root().is_some() {
                fill_loyalty(&ctx, &holder, id);
            }
        });
    }
    holder
}

fn fill_loyalty(ctx: &Ctx, holder: &gtk::Box, id: i64) {
    while let Some(ch) = holder.first_child() {
        holder.remove(&ch);
    }
    let Some(c) = ctx.core().character(id) else { return };
    let points = c.loyalty.as_ref().map(|l| l.points.clone()).unwrap_or_default();
    let offers: Vec<Option<Vec<extras::LpOffer>>> = points.iter().map(|lp| extras::cached_best_offers(lp.corporation_id)).collect();
    let loading = offers.iter().any(|o| o.is_none());
    let total: f64 = points.iter().zip(&offers).filter_map(|(lp, o)| o.as_ref().and_then(|o| extras::lp_value(lp.loyalty_points, o))).sum();

    let b = w::vbox(4);
    if points.is_empty() {
        b.append(&dim_text("No loyalty points"));
    }
    for (lp, offers) in points.iter().zip(offers) {
        let head = w::hbox(12);
        head.append(&w::avatar(&config::corporation_logo(lp.corporation_id, 64), 28));
        let name = w::line(&lp.corporation_name, &["body-medium"]);
        name.set_hexpand(true);
        head.append(&name);
        head.append(&fit(&format!("{} LP", fmt::int(lp.loyalty_points)), &["body-medium", "numeric"]));
        let worth = match &offers {
            None => "…".to_string(),
            Some(o) => match extras::lp_value(lp.loyalty_points, o) {
                Some(v) => format!("~{} ISK", fmt::isk_short(v)),
                None => "No priced offers".into(),
            },
        };
        let wl = fit(&worth, &["body-medium", "numeric", "dim"]);
        wl.set_width_chars(14);
        wl.set_xalign(1.0);
        head.append(&wl);

        let body: gtk::Widget = match &offers {
            None => dim_text("Pricing offers…").upcast(),
            Some(o) if o.is_empty() => dim_text("No offer from this store has a reliable Jita price.").upcast(),
            Some(o) => offers_table(o),
        };
        b.append(&collapsible(ctx, &format!("summary.lp.{}", lp.corporation_id), false, 36, &head.upcast(), &body));
    }
    if !points.is_empty() {
        let note = w::text("Best offer's item at Jita 4-4 sell, minus its ISK and item costs, before taxes and fees.", &["body-small", "dim"]);
        note.set_margin_top(8);
        b.append(&note);
    }
    let sub = if points.is_empty() {
        None
    } else if loading {
        Some("Pricing…".to_string())
    } else {
        Some(format!("~{} ISK at Jita", fmt::isk_short(total)))
    };
    holder.append(&card("Loyalty Points", "stars", sub.as_deref(), &[], &b));
}

fn offers_table(offers: &[extras::LpOffer]) -> gtk::Widget {
    let rows: Vec<extras::LpOffer> = offers.iter().take(5).cloned().collect();
    w::Table::new(rows)
        .widget_sorted(
            "Best offers",
            |o| {
                let title = if o.quantity > 1 { format!("{} × {}", fmt::int(o.quantity), o.name) } else { o.name.clone() };
                let needs = (!o.required_items.is_empty()).then(|| {
                    format!(
                        "needs {}",
                        o.required_items.iter().map(|i| format!("{} × {}", fmt::int(i.quantity), i.name)).collect::<Vec<_>>().join(", ")
                    )
                });
                type_cell(&w::eve_image(&config::type_icon(o.type_id, 64), 28).upcast(), &title, needs.as_deref())
            },
            |a, b| a.name.cmp(&b.name),
        )
        .expand()
        .num("LP", |o| fmt::int(o.lp_cost), |o| o.lp_cost as f64)
        .num("+ ISK", |o| if o.isk_cost > 0.0 { fmt::isk_short(o.isk_cost) } else { "—".into() }, |o| o.isk_cost)
        .num("ISK / LP", |o| fmt::decimal(o.isk_per_lp, 0), |o| o.isk_per_lp)
        .natural_height()
        .build()
}

/// Pieces the character tabs of this area share.
pub(crate) mod parts {
    use crate::ctx::Ctx;
    use crate::widgets as w;
    use gtk::prelude::*;
    use podstack_core::features::character_extras::{self as extras, StandingTier};
    use podstack_core::Character;

    const CSS: &str = "
.cx-collapse-head { border-radius: 12px; padding: 8px 8px; }
.cx-collapse-head:hover { background-color: alpha(@on_surface, 0.08); }
.cx-subtle { background-color: @surface_container; border-radius: 12px; padding: 10px 12px; }
.cx-queue-item { padding: 8px 12px; border-radius: 12px; }
.cx-queue-item.current { background-color: @secondary_container; color: @on_secondary_container; }
.cx-mono { font-family: monospace; font-size: 12px; background-color: @surface_container; border-radius: 8px; padding: 10px 12px; }
.cx-unread-dot { min-width: 8px; min-height: 8px; border-radius: 999px; background-color: @primary; }
.cx-standing { font-weight: 500; }
.cx-scope-row { padding: 6px 4px; }
.cx-medal { padding: 12px 4px; }
.stat-tile.cx-warn-tile { background-color: @error_container; }
.stat-tile.cx-warn-tile label { color: @on_error_container; }
.cx-dot-spacer { min-width: 8px; }
.cx-unread { font-weight: 600; }
";

    pub fn add_css() {
        crate::theme::add_css(CSS);
    }

    /// A card with an icon in its title row.
    pub fn card(title: &str, icon: &str, subtitle: Option<&str>, actions: &[gtk::Widget], body: &impl IsA<gtk::Widget>) -> gtk::Box {
        let c = w::card_with(title, subtitle, actions, body);
        if let Some(head) = c.first_child().and_then(|h| h.downcast::<gtk::Box>().ok()) {
            let i = w::icon_sized(icon, 20);
            i.add_css_class("primary-text");
            i.set_valign(gtk::Align::Start);
            i.set_margin_top(1);
            head.prepend(&i);
            head.set_spacing(12);
        }
        c
    }

    /// Two columns that become one on narrow windows.
    pub fn columns(left: Vec<gtk::Widget>, right: Vec<gtk::Widget>) -> gtk::Widget {
        let col = |items: Vec<gtk::Widget>| {
            let b = w::vbox(16);
            b.set_valign(gtk::Align::Start);
            for i in items {
                b.append(&i);
            }
            b.upcast::<gtk::Widget>()
        };
        let f = w::flow(vec![col(left), col(right)], 2);
        f.set_min_children_per_line(1);
        f.upcast()
    }

    /// A one-line label that keeps its full width (numbers, times, short keys).
    pub fn fit(t: &str, classes: &[&str]) -> gtk::Label {
        let l = w::line(t, classes);
        l.set_ellipsize(pango::EllipsizeMode::None);
        l
    }

    pub fn dim_text(t: &str) -> gtk::Label {
        w::text(t, &["body-medium", "dim"])
    }

    /// Label/widget rows.
    pub fn kv_grid(rows: Vec<(&str, gtk::Widget)>) -> gtk::Grid {
        let g = gtk::Grid::new();
        g.set_column_spacing(24);
        g.set_row_spacing(8);
        for (i, (k, v)) in rows.into_iter().enumerate() {
            let kl = fit(k, &["body-medium", "dim"]);
            kl.set_valign(gtk::Align::Center);
            g.attach(&kl, 0, i as i32, 1, 1);
            v.set_hexpand(true);
            v.set_halign(gtk::Align::Start);
            // values keep their full width rather than being cut off
            if let Some(l) = v.downcast_ref::<gtk::Label>() {
                l.set_ellipsize(pango::EllipsizeMode::None);
            }
            g.attach(&v, 1, i as i32, 1, 1);
        }
        g
    }

    /// Where data hasn't loaded: the scope notice when the login lacks it, else a "loads on the next refresh" note.
    pub fn pending(c: &Character, key: &str, scope: &str, what: &str) -> gtk::Widget {
        match c.refreshes.get(key).and_then(|r| r.error.as_deref()) {
            Some("scope") => w::scope_notice(scope).upcast(),
            Some(_) => {
                let msg = c.refreshes.get(key).and_then(|r| r.message.clone()).unwrap_or_default();
                w::banner("error_outline", &format!("Loading {what} failed: {msg}. PodStack tries again shortly."), w::BannerKind::Error).upcast()
            }
            None => dim_text(&format!("Not loaded yet: {what} load on the next refresh.")).upcast(),
        }
    }

    /// A whole tab whose data hasn't loaded.
    pub fn pending_page(c: &Character, key: &str, scope: &str, what: &str, icon: &str) -> gtk::Box {
        let b = w::vbox(16);
        match c.refreshes.get(key).and_then(|r| r.error.as_deref()) {
            Some("scope") | Some(_) => b.append(&pending(c, key, scope, what)),
            None => b.append(&w::empty_state(icon, "Not loaded yet", &format!("This character's {what} load on the next refresh."))),
        }
        b
    }

    /// An image (portrait, logo, icon) with a title and optional dim second line.
    pub fn type_cell(image: &gtk::Widget, title: &str, sub: Option<&str>) -> gtk::Widget {
        let row = w::hbox(12);
        image.set_valign(gtk::Align::Center);
        row.append(image);
        let texts = w::vbox(2);
        texts.set_valign(gtk::Align::Center);
        texts.set_hexpand(true);
        texts.append(&w::line(title, &["body-medium"]));
        if let Some(s) = sub {
            texts.append(&w::line(s, &["body-small", "dim"]));
        }
        row.append(&texts);
        row.upcast()
    }

    /// Security status in its game colors (high, low, null sec).
    pub fn sec_label(sec: f64) -> gtk::Label {
        let class = if sec >= 0.5 {
            "success-text"
        } else if sec > 0.0 {
            "warning-text"
        } else {
            "error-text"
        };
        fit(&format!("{sec:.1}"), &["body-medium", "numeric", class])
    }

    /// A standing, colored by how good it is.
    pub fn standing_label(v: f64) -> gtk::Label {
        let class = match extras::standing_tier(v) {
            StandingTier::Excellent => "primary-text",
            StandingTier::Good => "success-text",
            StandingTier::Neutral => "dim",
            StandingTier::Bad => "warning-text",
            StandingTier::Terrible => "error-text",
        };
        let l = fit(&extras::standing_text(v), &["body-medium", "numeric", "cx-standing", class]);
        l.set_xalign(1.0);
        l
    }

    /// A header row that shows/hides `body` (indented by `indent` px) when clicked. Open state is remembered under `key`.
    pub fn collapsible(ctx: &Ctx, key: &str, default_open: bool, indent: i32, header: &gtk::Widget, body: &gtk::Widget) -> gtk::Box {
        let open = ctx.recall(key).map(|v| v == "1").unwrap_or(default_open);
        let outer = w::vbox(0);
        let head = w::hbox(8);
        head.add_css_class("cx-collapse-head");
        let chevron = w::icon_sized(if open { "expand_more" } else { "chevron_right" }, 20);
        chevron.add_css_class("dim");
        chevron.set_valign(gtk::Align::Center);
        head.append(&chevron);
        header.set_hexpand(true);
        head.append(header);
        head.set_cursor_from_name(Some("pointer"));
        let rev = gtk::Revealer::new();
        rev.set_transition_type(gtk::RevealerTransitionType::SlideDown);
        rev.set_transition_duration(150);
        rev.set_reveal_child(open);
        body.set_margin_start(indent);
        body.set_margin_top(4);
        body.set_margin_bottom(8);
        rev.set_child(Some(body));
        let click = gtk::GestureClick::new();
        {
            let (ctx, key, rev) = (ctx.clone(), key.to_string(), rev.clone());
            click.connect_released(move |_, _, _, _| {
                let now_open = !rev.reveals_child();
                rev.set_reveal_child(now_open);
                chevron.set_text(if now_open { "expand_more" } else { "chevron_right" });
                ctx.remember(&key, if now_open { "1" } else { "0" });
            });
        }
        head.add_controller(click);
        outer.append(&head);
        outer.append(&rev);
        outer
    }

    /// Replaces a box's children.
    pub fn clear(b: &gtk::Box) {
        while let Some(ch) = b.first_child() {
            b.remove(&ch);
        }
    }
}
