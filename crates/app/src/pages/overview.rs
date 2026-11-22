//! Overview: fleet totals and the roster of every character, sorted by skill points. A row opens the character.
//!
//! Also holds [`fleet`], the pieces the fleet pages (Overview, SP Farming, Queue Health) share.

use crate::ctx::{CharTab, Ctx, Page};
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::{fmt, training::roman, Character};

use fleet::{Updaters, CSS};

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let core = ctx.core().clone();
    let mut chars = core.characters();
    chars.sort_by_key(|c| std::cmp::Reverse(c.total_sp()));

    let add = w::button_icon("person_add", "Add character", Btn::Filled);
    {
        let ctx = ctx.clone();
        add.connect_clicked(move |_| crate::login::show(&ctx));
    }

    let body = w::page_body();
    if chars.is_empty() {
        let empty = w::empty_state("group", "No characters yet", "Add your first capsuleer: PodStack opens EVE's login page in your browser.");
        let b = w::button_icon("person_add", "Add character", Btn::Filled);
        b.set_halign(gtk::Align::Center);
        b.set_margin_top(8);
        let ctx2 = ctx.clone();
        b.connect_clicked(move |_| crate::login::show(&ctx2));
        empty.append(&b);
        body.append(&empty);
        return Page::new("Overview", body).action(add);
    }

    let total_sp: i64 = chars.iter().map(|c| c.total_sp()).sum();
    let total_isk: f64 = chars.iter().filter_map(|c| c.wallet).sum();
    let omegas = chars.iter().filter(|c| c.is_omega() == Some(true)).count();
    let idle = chars.iter().filter(|c| !c.is_training()).count();
    let n = chars.len();

    let sp_tile = fleet::tile("psychology", "Skill points", &fmt::sp_short(total_sp), Some(&fmt::sp(total_sp)), false);
    body.append(&w::tile_row(vec![
        fleet::tile("group", "Capsuleers", &n.to_string(), Some(&format!("{omegas} Omega")), false),
        sp_tile.clone(),
        fleet::tile("account_balance_wallet", "Net worth", &format!("{} ISK", fmt::isk_short(total_isk)), Some("Combined wallet balance"), false),
        fleet::tile(
            "schedule",
            "Training",
            &format!("{}/{n}", n - idle),
            Some(&if idle > 0 { format!("{idle} idle queue{}", if idle == 1 { "" } else { "s" }) } else { "All queues active".into() }),
            idle > 0,
        ),
    ]));

    let updaters = Updaters::default();
    {
        // the SP total counts up while characters train
        let core = core.clone();
        let value = fleet::tile_value(&sp_tile);
        let sub = fleet::tile_sub(&sp_tile);
        updaters.add(move || {
            let sp: i64 = core.characters().iter().map(|c| c.total_sp()).sum();
            value.set_text(&fmt::sp_short(sp));
            if let Some(s) = &sub {
                s.set_text(&fmt::sp(sp));
            }
        });
    }

    let list = fleet::list();
    list.append(&fleet::head_row(&[("", 56), ("Capsuleer", -180), ("Wallet", 120), ("Skill points", 120), ("Training", 260)]));
    for c in &chars {
        list.append(&roster_row(ctx, c, &updaters));
    }
    fleet::activate_rows(ctx, &list, 1, chars.iter().map(|c| c.id).collect(), CharTab::Summary);

    let card = fleet::list_card("Roster", Some("Sorted by skill points. Select a character to open it."), &[], &list);
    body.append(&card);
    updaters.tick(&body, 1);

    Page::new("Overview", body).action(add)
}

fn roster_row(ctx: &Ctx, c: &Character, updaters: &Updaters) -> gtk::Widget {
    let core = ctx.core().clone();
    let id = c.id;
    let row = w::hbox(16);
    row.add_css_class("fleet-row");
    row.append(&fleet::portrait(&core, c, 48));
    row.append(&fleet::name_block(c, true));

    // wallet
    let wallet = fleet::figure(
        &c.wallet.map(|v| format!("{} ISK", fmt::isk_short(v))).unwrap_or_else(|| "—".into()),
        &c.wallet.map(|v| fmt::decimal(v, 2)).unwrap_or_default(),
        120,
    );
    row.append(&wallet);

    // skill points, counting up
    let sp = fleet::figure(&fmt::sp_short(c.total_sp()), &fmt::int(c.total_sp()), 120);
    let (sp_big, sp_small) = fleet::figure_labels(&sp);
    if c.is_training() {
        let core = core.clone();
        updaters.add(move || {
            if let Some(c) = core.character(id) {
                let t = c.total_sp();
                sp_big.set_text(&fmt::sp_short(t));
                sp_small.set_text(&fmt::int(t));
            }
        });
    }
    row.append(&sp);

    // training
    let training = w::vbox(4);
    training.set_size_request(260, -1);
    training.set_valign(gtk::Align::Center);
    match c.current_skill() {
        Some(q) => {
            let top = w::hbox(8);
            let name = w::line(&format!("{} {}", q.skill_name, roman(q.finished_level)), &["body-medium"]);
            name.set_hexpand(true);
            top.append(&name);
            let left = w::line("", &["body-small", "dim", "numeric"]);
            top.append(&left);
            training.append(&top);
            let bar = w::progress(0.0);
            training.append(&bar);
            let ends = w::line("", &["body-small", "dim"]);
            training.append(&ends);
            let update = {
                let core = core.clone();
                move || {
                    let Some(c) = core.character(id) else { return };
                    let Some(q) = c.current_skill() else {
                        left.set_text("");
                        bar.set_fraction(1.0);
                        return;
                    };
                    let now = chrono::Utc::now();
                    if let Some(f) = q.finish_date {
                        left.set_text(&fmt::duration(f - now, 2));
                    }
                    bar.set_fraction(fleet::training_progress(q, now));
                    if let Some(e) = c.queue_end() {
                        ends.set_text(&format!("Queue ends in {}", fmt::duration(e - now, 3)));
                    }
                }
            };
            update();
            updaters.add(update);
        }
        None => training.append(&fleet::not_training()),
    }
    row.append(&training);
    row.upcast()
}

/// What the fleet pages share: styles, tiles, portraits with a login status dot, list cards and countdowns.
pub(crate) mod fleet {
    use crate::ctx::{CharTab, Ctx, Route};
    use crate::widgets::{self as w, Status};
    use gtk::prelude::*;
    use podstack_core::domains::skills::QueueEntry;
    use podstack_core::{Character, Core};
    use std::cell::RefCell;
    use std::rc::Rc;

    pub const CSS: &str = r#"
.stat-tile .fleet-tile-icon { color: @primary; margin-bottom: 4px; }
.stat-tile.fleet-warn { background-color: @warning_container; }
.stat-tile.fleet-warn .stat-label, .stat-tile.fleet-warn .stat-value, .stat-tile.fleet-warn .stat-sub,
.stat-tile.fleet-warn .fleet-tile-icon { color: @on_warning_container; }
.stat-tile.fleet-bad { background-color: @error_container; }
.stat-tile.fleet-bad .stat-label, .stat-tile.fleet-bad .stat-value, .stat-tile.fleet-bad .stat-sub,
.stat-tile.fleet-bad .fleet-tile-icon { color: @on_error_container; }
.stat-tile.fleet-good { background-color: @success_container; }
.stat-tile.fleet-good .stat-label, .stat-tile.fleet-good .stat-value, .stat-tile.fleet-good .stat-sub,
.stat-tile.fleet-good .fleet-tile-icon { color: @on_success_container; }

.fleet-card-head { padding: 16px 16px 8px 16px; }
list.fleet-list { background-color: transparent; }
list.fleet-list > row { border-top: 1px solid alpha(@outline_variant, 0.5); }
list.fleet-list > row.fleet-head-row, list.fleet-list > row.fleet-head-row:hover { background-color: @surface_container; border-top: none; }
list.fleet-list > row:last-child { border-radius: 0 0 16px 16px; }
.fleet-row { padding: 12px 16px; }
.fleet-head { padding: 10px 16px; }
.fleet-head label { color: @on_surface_variant; font-size: 12px; font-weight: 500; letter-spacing: 0.5px; }
.fleet-figure { font-size: 16px; font-weight: 500; }

.fleet-dot { min-width: 12px; min-height: 12px; border-radius: 999px; background-color: @success;
  border: 2px solid @surface_container_low; }
.fleet-dot.legacy { background-color: @warning; }
.fleet-dot.error { background-color: @error; }

.fleet-omega { background-color: @tertiary_container; color: @on_tertiary_container; border-radius: 8px; padding: 2px 8px;
  font-size: 12px; font-weight: 500; }
.fleet-alpha { background-color: @surface_container_highest; color: @on_surface_variant; border-radius: 8px; padding: 2px 8px;
  font-size: 12px; font-weight: 500; }

.fleet-result { border-radius: 16px; padding: 16px 20px; }
.fleet-result.good { background-color: @success_container; color: @on_success_container; }
.fleet-result.bad { background-color: @error_container; color: @on_error_container; }
.fleet-result label { color: inherit; }
.fleet-result .fleet-result-value { font-size: 28px; font-weight: 400; }

.fleet-grid { padding: 4px 16px 12px 16px; }
.fleet-grid .fleet-grid-head { color: @on_surface_variant; font-size: 12px; font-weight: 500; letter-spacing: 0.5px;
  padding-bottom: 4px; }
.fleet-grid .fleet-total { font-weight: 600; }
.fleet-grid entry, .fleet-grid spinbutton { min-height: 36px; }
.fleet-grid dropdown > button { min-height: 36px; }
.fleet-rule { background-color: @outline_variant; min-height: 1px; }

.fleet-idle-card { border: 1px solid @error; }
.fleet-account-head { padding: 16px 16px 12px 16px; }
"#;

    /// Callbacks run together every few seconds while the page is shown (countdowns, SP counting up).
    #[derive(Clone, Default)]
    pub struct Updaters(Rc<RefCell<Vec<Updater>>>);

    type Updater = Box<dyn Fn()>;

    impl Updaters {
        pub fn add(&self, f: impl Fn() + 'static) {
            self.0.borrow_mut().push(Box::new(f));
        }

        pub fn tick(&self, widget: &impl IsA<gtk::Widget>, secs: u32) {
            if self.0.borrow().is_empty() {
                return;
            }
            let list = self.0.clone();
            w::ticker(widget, secs, move || {
                for f in list.borrow().iter() {
                    f();
                }
            });
        }
    }

    /// A stat tile with an icon; `warn` tints it.
    pub fn tile(icon: &str, label: &str, value: &str, sub: Option<&str>, warn: bool) -> gtk::Box {
        let t = w::stat_tile(label, value, sub);
        let i = w::icon_sized(icon, 20);
        i.set_halign(gtk::Align::Start);
        i.add_css_class("fleet-tile-icon");
        t.prepend(&i);
        if warn {
            t.add_css_class("fleet-warn");
        }
        t
    }

    fn tile_label(t: &gtk::Box, class: &str) -> Option<gtk::Label> {
        let mut child = t.first_child();
        while let Some(c) = child {
            if c.has_css_class(class) {
                return c.downcast::<gtk::Label>().ok();
            }
            child = c.next_sibling();
        }
        None
    }

    pub fn tile_value(t: &gtk::Box) -> gtk::Label {
        tile_label(t, "stat-value").unwrap_or_else(|| gtk::Label::new(None))
    }

    pub fn tile_sub(t: &gtk::Box) -> Option<gtk::Label> {
        tile_label(t, "stat-sub")
    }

    /// "Ω Omega" / "α Alpha" (nothing when unknown).
    pub fn clone_badge(c: &Character) -> Option<gtk::Label> {
        let (t, class, tip) = match c.is_omega()? {
            true => ("Ω Omega", "fleet-omega", "Omega clone state"),
            false => ("α Alpha", "fleet-alpha", "Alpha clone state"),
        };
        let l = gtk::Label::new(Some(t));
        l.add_css_class(class);
        l.set_tooltip_text(Some(tip));
        l.set_valign(gtk::Align::Center);
        Some(l)
    }

    /// The character's portrait with a dot for its login: healthy, old (legacy) application, or failing.
    pub fn portrait(core: &Core, c: &Character, size: i32) -> gtk::Widget {
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&w::avatar(&c.portrait_url(128), size)));
        overlay.set_valign(gtk::Align::Center);
        if let Some(a) = core.auth.get(c.id) {
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.add_css_class("fleet-dot");
            dot.set_halign(gtk::Align::End);
            dot.set_valign(gtk::Align::End);
            let failing = a.last_refresh.as_ref().map(|r| !r.success).unwrap_or(false);
            let tip = if failing {
                dot.add_css_class("error");
                "Token refresh failing: re-authorize this character"
            } else if a.uses_legacy_client() {
                dot.add_css_class("legacy");
                "Token healthy (old PodStack/Cerebral login)"
            } else {
                "Token healthy"
            };
            dot.set_tooltip_text(Some(tip));
            overlay.add_overlay(&dot);
        }
        overlay.upcast()
    }

    /// Name with clone badge, and corporation (and alliance) below. Takes the spare width.
    pub fn name_block(c: &Character, alliance: bool) -> gtk::Box {
        let b = w::vbox(4);
        b.set_hexpand(true);
        b.set_size_request(180, -1);
        b.set_valign(gtk::Align::Center);
        let top = w::hbox(8);
        top.append(&w::line(&c.name(), &["title-small"]));
        if let Some(badge) = clone_badge(c) {
            top.append(&badge);
        }
        b.append(&top);
        if let Some(i) = &c.info {
            let corp = w::hbox(6);
            if i.corporation_id > 0 {
                corp.append(&w::eve_image(&podstack_core::config::corporation_logo(i.corporation_id, 32), 16));
            }
            let mut t = c.corporation_name().unwrap_or("").to_string();
            if alliance {
                if let Some(a) = c.alliance_name() {
                    t.push_str(&format!("  /  {a}"));
                }
            }
            let l = w::line(&t, &["body-small", "dim"]);
            l.set_tooltip_text(Some(&t));
            corp.append(&l);
            b.append(&corp);
        }
        b
    }

    /// A big number with a small caption below, `width` wide and right-aligned.
    pub fn figure(big: &str, small: &str, width: i32) -> gtk::Box {
        let b = w::vbox(2);
        b.set_size_request(width, -1);
        b.set_valign(gtk::Align::Center);
        let l = w::line(big, &["fleet-figure", "numeric"]);
        l.set_xalign(1.0);
        b.append(&l);
        let s = w::line(small, &["body-small", "dim", "numeric"]);
        s.set_xalign(1.0);
        b.append(&s);
        b
    }

    pub fn figure_labels(b: &gtk::Box) -> (gtk::Label, gtk::Label) {
        let big = b.first_child().and_then(|c| c.downcast::<gtk::Label>().ok()).unwrap_or_else(|| gtk::Label::new(None));
        let small = big.next_sibling().and_then(|c| c.downcast::<gtk::Label>().ok()).unwrap_or_else(|| gtk::Label::new(None));
        (big, small)
    }

    pub fn list() -> gtk::ListBox {
        let l = gtk::ListBox::new();
        l.add_css_class("fleet-list");
        l.set_selection_mode(gtk::SelectionMode::None);
        l
    }

    /// Column headings: (title, width; negative = at least that wide, sharing the spare width). Numbers are right-aligned, so are their titles.
    pub fn head_row(cols: &[(&str, i32)]) -> gtk::ListBoxRow {
        let b = w::hbox(16);
        b.add_css_class("fleet-head");
        for (t, width) in cols {
            let l = w::line(t, &[]);
            if *width < 0 {
                l.set_hexpand(true);
                l.set_size_request(-*width, -1);
            } else {
                l.set_size_request(*width, -1);
            }
            if matches!(*t, "Wallet" | "Skill points") {
                l.set_xalign(1.0);
            }
            b.append(&l);
        }
        let row = gtk::ListBoxRow::new();
        row.add_css_class("fleet-head-row");
        row.set_activatable(false);
        row.set_child(Some(&b));
        row
    }

    /// Clicking a row opens that character on `tab`; `skip` rows (headings) come before the first character.
    pub fn activate_rows(ctx: &Ctx, list: &gtk::ListBox, skip: i32, ids: Vec<i64>, tab: CharTab) {
        list.set_activate_on_single_click(true);
        let ctx = ctx.clone();
        list.connect_row_activated(move |_, row| {
            let i = row.index() - skip;
            if i >= 0 {
                if let Some(id) = ids.get(i as usize) {
                    ctx.navigate(Route::Character(*id, tab));
                }
            }
        });
    }

    /// A card holding a full-width list: title, subtitle and actions padded, the list edge to edge.
    pub fn list_card(title: &str, subtitle: Option<&str>, actions: &[gtk::Widget], body: &impl IsA<gtk::Widget>) -> gtk::Box {
        let c = w::vbox(0);
        c.add_css_class("card");
        c.add_css_class("flush");
        let head = w::hbox(8);
        head.add_css_class("fleet-card-head");
        let titles = w::vbox(2);
        titles.set_hexpand(true);
        titles.set_size_request(220, -1);
        titles.append(&w::text(title, &["title-medium"]));
        if let Some(s) = subtitle {
            titles.append(&w::text(s, &["body-medium", "dim"]));
        }
        head.append(&titles);
        for a in actions {
            a.set_valign(gtk::Align::Center);
            head.append(a);
        }
        c.append(&head);
        c.append(&hscroll(body));
        c
    }

    /// Wide lists scroll sideways in narrow windows instead of squashing their names.
    pub fn hscroll(body: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
        let sw = gtk::ScrolledWindow::new();
        sw.set_vscrollbar_policy(gtk::PolicyType::Never);
        sw.set_hscrollbar_policy(gtk::PolicyType::Automatic);
        sw.set_propagate_natural_height(true);
        sw.set_propagate_natural_width(true);
        sw.set_child(Some(body));
        sw
    }

    /// How far the training skill's current level is, by time (as EVE's own queue shows it).
    pub fn training_progress(q: &QueueEntry, now: chrono::DateTime<chrono::Utc>) -> f64 {
        match (q.start_date, q.finish_date) {
            (Some(s), Some(f)) if f > s => ((now - s).num_milliseconds() as f64 / (f - s).num_milliseconds() as f64).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }

    pub fn not_training() -> gtk::Label {
        w::status("Not training", Status::Warn)
    }
}
