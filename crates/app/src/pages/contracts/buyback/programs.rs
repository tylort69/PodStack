//! Buyback → Programs: the list of programs and the program editor.

use super::{pct, select, show_view};
use crate::ctx::Ctx;
use crate::pages::industry::form::{fields, labelled, number_entry, parse_amount, percent_text, suggest_entry};
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::buyback::{self as bb, Basis, Program, Receiver, Rule, Target, Treatment};
use podstack_core::features::contracts::Mine;
use podstack_core::fmt;
use podstack_core::sde::{MARKET, TRADE_HUBS};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn view(ctx: &Ctx) -> gtk::Box {
    let col = w::vbox(16);
    let doc = bb::load_doc();
    if doc.programs.is_empty() {
        col.append(&super::no_programs(ctx));
        return col;
    }
    let head = w::hbox(12);
    let intro = w::text(
        "A program is a buyback's terms: which hub prices it uses, the share it pays, and what it won't take. Add the terms of a buyback you sell to, to check its quotes, or of one you run, to price incoming contracts.",
        &["body-medium", "dim"],
    );
    intro.set_hexpand(true);
    head.append(&intro);
    let add = w::button_icon("add", "New program", Btn::Filled);
    add.set_valign(gtk::Align::Center);
    {
        let ctx = ctx.clone();
        add.connect_clicked(move |_| edit(&ctx, Program::default()));
    }
    head.append(&add);
    col.append(&head);
    for p in &doc.programs {
        col.append(&program_card(ctx, p));
    }
    col
}

fn treatment_text(t: Treatment) -> String {
    match t {
        Treatment::Rate(r) => format!("pays {}", pct(r)),
        Treatment::Refuse => "not accepted".into(),
    }
}

fn program_card(ctx: &Ctx, p: &Program) -> gtk::Box {
    let appraise = w::button_icon("calculate", "Appraise", Btn::Tonal);
    let queue = w::button_icon("inbox", "Queue", Btn::Text);
    let edit_b = w::icon_button("edit", "Edit");
    let dup = w::icon_button("content_copy", "Duplicate");
    let del = w::icon_button("delete", "Delete");
    for (b, view) in [(&appraise, 1usize), (&queue, 2)] {
        let (ctx, id) = (ctx.clone(), p.id);
        b.connect_clicked(move |_| {
            select(&ctx, id);
            show_view(&ctx, view);
        });
    }
    {
        let (ctx, p) = (ctx.clone(), p.clone());
        edit_b.connect_clicked(move |_| edit(&ctx, p.clone()));
    }
    {
        let (ctx, id) = (ctx.clone(), p.id);
        dup.connect_clicked(move |_| {
            let mut doc = bb::load_doc();
            if let Some(new_id) = doc.duplicate(id) {
                select(&ctx, new_id);
                bb::save_doc(ctx.core(), &doc);
                ctx.toast("Program duplicated");
            }
        });
    }
    {
        let (ctx, id, name) = (ctx.clone(), p.id, p.name.clone());
        del.connect_clicked(move |_| {
            let ctx2 = ctx.clone();
            w::confirm(&ctx, "Delete program?", &format!("\"{name}\" and its rules will be deleted. Contracts and history aren't affected."), "Delete", true, move || {
                let mut doc = bb::load_doc();
                doc.remove(id);
                bb::save_doc(ctx2.core(), &doc);
            });
        });
    }

    let (hub, _, _) = p.hub();
    let receiver = p.receiver_label();
    let mut rows: Vec<(&str, String)> = vec![
        ("Prices", format!("{hub} · {}", p.basis.label().to_lowercase())),
        ("Pays", format!("{} of the hub price", pct(p.rate))),
        ("Hauling", if p.haul_per_m3 > 0.0 { format!("{} ISK per m³ taken off", fmt::decimal(p.haul_per_m3, 2)) } else { "None".into() }),
        ("Contracts to", if receiver.is_empty() { "Not set".into() } else { receiver }),
    ];
    if !p.location.trim().is_empty() {
        rows.push(("Location", p.location.trim().to_string()));
    }
    rows.push(("Queue tolerance", format!("up to {} over the offer needs a look", pct(p.tolerance))));
    let body = w::vbox(12);
    body.append(&w::key_values(&rows));
    if !p.rules.is_empty() {
        let rules = w::vbox(4);
        rules.append(&w::line("Rules", &["label-large"]));
        for r in &p.rules {
            let row = w::hbox(8);
            row.append(&w::icon_sized(if matches!(r.target, Target::Group(_)) { "folder" } else { "category" }, 18));
            let l = w::line(&r.target.label(), &["body-medium"]);
            l.set_hexpand(true);
            row.append(&l);
            row.append(&w::status(&treatment_text(r.treatment), if r.treatment == Treatment::Refuse { w::Status::Bad } else { w::Status::Info }));
            rules.append(&row);
        }
        body.append(&rules);
    }
    if !p.notes.trim().is_empty() {
        let n = w::text(p.notes.trim(), &["body-medium", "dim"]);
        n.set_selectable(true);
        body.append(&n);
    }
    let card = w::card_with(&p.name, Some(&p.summary()), &[appraise.upcast(), queue.upcast(), edit_b.upcast(), dup.upcast(), del.upcast()], &body);
    card.add_css_class("bb-program-card");
    card
}

// ---------- editor ----------

/// What the receiver dropdown offers: not set, the user's characters, their corporations, someone else.
#[derive(Clone)]
enum Choice {
    Unset,
    Character(i64, String),
    Corporation(i64, String),
    Other,
}

fn choices(ctx: &Ctx) -> Vec<Choice> {
    let mine = Mine::of(&ctx.core().characters());
    let mut v = vec![Choice::Unset];
    v.extend(mine.characters.iter().map(|(id, n)| Choice::Character(*id, n.clone())));
    v.extend(mine.corporations.iter().map(|(id, n)| Choice::Corporation(*id, n.clone())));
    v.push(Choice::Other);
    v
}

fn choice_label(c: &Choice) -> String {
    match c {
        Choice::Unset => "Not set".into(),
        Choice::Character(_, n) => n.clone(),
        Choice::Corporation(_, n) => format!("{n} (corporation)"),
        Choice::Other => "Someone else (by name)".into(),
    }
}

/// Market groups whose name contains every word of `q`, as (id, name, path above it).
fn search_groups(q: &str, limit: usize) -> Vec<(i64, String, String)> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut hits: Vec<(u8, usize, i64, String)> = MARKET
        .groups
        .values()
        .filter_map(|g| {
            let n = g.name.to_lowercase();
            if !words.iter().all(|w| n.contains(w)) {
                return None;
            }
            let rank = if n == q { 0 } else if n.starts_with(&q) { 1 } else { 2 };
            let depth = MARKET.path(g.id).len();
            Some((rank, depth, g.id, g.name.clone()))
        })
        .collect();
    hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)).then(a.3.cmp(&b.3)));
    hits.into_iter()
        .take(limit)
        .map(|(_, _, id, name)| {
            let path = MARKET.path(id);
            let above: Vec<&str> = path.iter().take(path.len().saturating_sub(1)).map(|g| g.name.as_str()).collect();
            (id, name, if above.is_empty() { "Group".into() } else { format!("Group in {}", above.join(" › ")) })
        })
        .collect()
}

/// The rules list's own refill, held weakly.
type Refill = std::rc::Weak<dyn Fn()>;

/// Opens the editor; saving puts the program in the document (a new one gets an id then).
pub(super) fn edit(ctx: &Ctx, p: Program) {
    crate::theme::add_css(crate::pages::industry::common::CSS);
    let is_new = p.id == 0;
    let (win, content, actions) = w::dialog(ctx, if is_new { "New buyback program" } else { "Edit buyback program" }, 680);
    win.set_default_height(760);
    let form = w::vbox(16);

    let name = gtk::Entry::new();
    name.set_text(&p.name);
    name.set_hexpand(true);
    form.append(&labelled("Name", &name));

    let hubs: Vec<&str> = TRADE_HUBS.iter().map(|h| h.0).collect();
    let hub_dd = w::dropdown(&hubs, TRADE_HUBS.iter().position(|h| h.1 == p.station).unwrap_or(0) as u32);
    let bases: Vec<&str> = Basis::ALL.iter().map(|b| b.label()).collect();
    let basis_dd = w::dropdown(&bases, Basis::ALL.iter().position(|b| *b == p.basis).unwrap_or(0) as u32);
    let rate = number_entry(&percent_text(p.rate), 7);
    let haul = number_entry(&if p.haul_per_m3 > 0.0 { p.haul_per_m3.to_string() } else { String::new() }, 9);
    haul.set_placeholder_text(Some("0"));
    let tolerance = number_entry(&percent_text(p.tolerance), 6);
    form.append(&fields(vec![
        labelled("Price hub", &hub_dd).upcast(),
        labelled("Price basis", &basis_dd).upcast(),
        labelled("Pays (%)", &rate).upcast(),
        labelled("Hauling (ISK per m³)", &haul).upcast(),
        labelled("Queue tolerance (%)", &tolerance).upcast(),
    ]));

    let opts = choices(ctx);
    let labels: Vec<String> = opts.iter().map(choice_label).collect();
    let label_refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let current = match &p.receiver {
        Receiver::Unset => 0,
        Receiver::Character(id) => opts.iter().position(|c| matches!(c, Choice::Character(i, _) if i == id)).unwrap_or(opts.len() - 1),
        Receiver::Corporation(id) => opts.iter().position(|c| matches!(c, Choice::Corporation(i, _) if i == id)).unwrap_or(opts.len() - 1),
        Receiver::Name(_) => opts.len() - 1,
    };
    let receiver_dd = w::dropdown(&label_refs, current as u32);
    let other = gtk::Entry::new();
    other.set_placeholder_text(Some("Character or corporation name"));
    other.set_width_chars(28);
    match &p.receiver {
        Receiver::Name(n) => other.set_text(n),
        // a character or corporation no longer added: keep it by name
        Receiver::Character(_) | Receiver::Corporation(_) if current == opts.len() - 1 => other.set_text(&p.receiver_name),
        _ => {}
    }
    other.set_visible(current == opts.len() - 1);
    {
        let (other, last) = (other.clone(), opts.len() - 1);
        receiver_dd.connect_selected_notify(move |dd| other.set_visible(dd.selected() as usize == last));
    }
    let location = gtk::Entry::new();
    location.set_text(&p.location);
    location.set_placeholder_text(Some("e.g. Jita IV - Moon 4 - Caldari Navy Assembly Plant"));
    location.set_width_chars(36);
    form.append(&fields(vec![labelled("Contracts go to", &receiver_dd).upcast(), other.clone().upcast(), labelled("Contract location", &location).upcast()]));

    // rules
    let rules: Rc<RefCell<Vec<Rule>>> = Rc::new(RefCell::new(p.rules.clone()));
    let rules_list = w::vbox(4);
    // weak: the list's own buttons call it (the rule entry below keeps it alive)
    let refill: Rc<RefCell<Option<Refill>>> = Rc::default();
    let fill: Rc<dyn Fn()> = {
        let (rules, rules_list, refill) = (rules.clone(), rules_list.clone(), refill.clone());
        Rc::new(move || {
            while let Some(c) = rules_list.first_child() {
                rules_list.remove(&c);
            }
            if rules.borrow().is_empty() {
                rules_list.append(&w::line("No rules: everything is bought at the default share.", &["body-medium", "dim"]));
            }
            let snapshot = rules.borrow().clone();
            for (i, r) in snapshot.iter().enumerate() {
                let row = w::hbox(8);
                row.add_css_class("bb-rule-row");
                row.append(&w::icon_sized(if matches!(r.target, Target::Group(_)) { "folder" } else { "category" }, 18));
                let l = w::line(&r.target.label(), &["body-medium"]);
                l.set_hexpand(true);
                l.set_tooltip_text(Some(&r.target.label()));
                row.append(&l);
                let kind = w::dropdown(&["Pays", "Not accepted"], if r.treatment == Treatment::Refuse { 1 } else { 0 });
                let share = number_entry(&match r.treatment {
                    Treatment::Rate(x) => percent_text(x),
                    Treatment::Refuse => String::new(),
                }, 6);
                share.set_placeholder_text(Some("%"));
                share.set_sensitive(r.treatment != Treatment::Refuse);
                share.set_tooltip_text(Some("Share of the hub price paid (%)"));
                {
                    let (rules, share) = (rules.clone(), share.clone());
                    kind.connect_selected_notify(move |dd| {
                        let refuse = dd.selected() == 1;
                        share.set_sensitive(!refuse);
                        if let Some(r) = rules.borrow_mut().get_mut(i) {
                            r.treatment = if refuse { Treatment::Refuse } else { Treatment::Rate(parse_amount(&share.text()).unwrap_or(0.0) / 100.0) };
                        }
                    });
                }
                {
                    let rules = rules.clone();
                    share.connect_changed(move |e| {
                        if let (Some(r), Some(v)) = (rules.borrow_mut().get_mut(i), parse_amount(&e.text())) {
                            if r.treatment != Treatment::Refuse {
                                r.treatment = Treatment::Rate(v / 100.0);
                            }
                        }
                    });
                }
                row.append(&kind);
                row.append(&share);
                let remove = w::icon_button("close", "Remove rule");
                {
                    let (rules, refill) = (rules.clone(), refill.clone());
                    remove.connect_clicked(move |_| {
                        rules.borrow_mut().remove(i);
                        if let Some(f) = refill.borrow().as_ref().and_then(|w| w.upgrade()) {
                            gtk::glib::idle_add_local_once(move || f());
                        }
                    });
                }
                row.append(&remove);
                rules_list.append(&row);
            }
        })
    };
    *refill.borrow_mut() = Some(Rc::downgrade(&fill));
    fill();
    // groups are offered with negative ids so one entry can suggest both groups and items
    // weak: the entry owns the closure that holds this
    let entry_slot: Rc<RefCell<Option<gtk::glib::WeakRef<gtk::SearchEntry>>>> = Rc::default();
    let add_rule = {
        let (rules, fill, entry_slot) = (rules.clone(), fill.clone(), entry_slot.clone());
        suggest_entry(
            "Add a rule for a market group or item",
            |q| {
                let mut v: Vec<(i64, String, Option<String>)> = search_groups(q, 6).into_iter().map(|(id, n, d)| (-id, n, Some(d))).collect();
                v.extend(MARKET.search(q, 6).into_iter().map(|t| (t.id, t.name.clone(), Some("Item".to_string()))));
                v
            },
            move |id, _| {
                let target = if id < 0 { Target::Group(-id) } else { Target::Type(id) };
                let added = {
                    let mut r = rules.borrow_mut();
                    let new = !r.iter().any(|x| x.target == target);
                    if new {
                        r.push(Rule { target, treatment: Treatment::Rate(0.8) });
                    }
                    new
                };
                // after the entry has shown the picked name: clear it for the next rule
                let (fill, entry) = (fill.clone(), entry_slot.borrow().as_ref().and_then(|w| w.upgrade()));
                gtk::glib::idle_add_local_once(move || {
                    if added {
                        fill();
                    }
                    if let Some(e) = entry {
                        e.set_text("");
                    }
                });
            },
        )
    };
    *entry_slot.borrow_mut() = Some(add_rule.downgrade());
    let rules_box = w::vbox(8);
    rules_box.add_css_class("card-filled");
    rules_box.append(&w::text("Rules", &["title-small"]));
    rules_box.append(&w::text(
        "Pay a different share for, or refuse, a market group (and everything under it) or a single item. An item's own rule wins over its group's, and a group's over the group above it.",
        &["body-small", "dim"],
    ));
    rules_box.append(&add_rule);
    rules_box.append(&rules_list);
    form.append(&rules_box);

    let notes = gtk::TextView::new();
    notes.add_css_class("bb-paste");
    notes.set_wrap_mode(gtk::WrapMode::WordChar);
    notes.buffer().set_text(&p.notes);
    notes.set_size_request(-1, 72);
    form.append(&labelled("Notes", &notes));

    let error = w::line("", &["error-text", "body-medium"]);
    error.set_visible(false);
    form.append(&error);

    let sw = w::scrolled(&form);
    sw.set_min_content_height(420);
    content.append(&sw);

    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button(if is_new { "Add program" } else { "Save" }, Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (win, ctx) = (win.clone(), ctx.clone());
        save.connect_clicked(move |_| {
            let fail = |msg: &str| {
                error.set_text(msg);
                error.set_visible(true);
            };
            let n = name.text().trim().to_string();
            if n.is_empty() {
                return fail("Give the program a name.");
            }
            let Some(r) = parse_amount(&rate.text()).filter(|r| (0.0..=200.0).contains(r)) else {
                return fail("Enter the share paid as a percent, e.g. 90.");
            };
            let h = if haul.text().trim().is_empty() { Some(0.0) } else { parse_amount(&haul.text()).filter(|h| *h >= 0.0) };
            let Some(h) = h else { return fail("Enter the hauling charge in ISK per m³, or leave it empty.") };
            let Some(t) = parse_amount(&tolerance.text()).filter(|t| *t >= 0.0) else {
                return fail("Enter the queue tolerance as a percent, e.g. 2.");
            };
            let (receiver, receiver_name) = match opts.get(receiver_dd.selected() as usize) {
                Some(Choice::Character(id, n)) => (Receiver::Character(*id), n.clone()),
                Some(Choice::Corporation(id, n)) => (Receiver::Corporation(*id), n.clone()),
                Some(Choice::Other) if !other.text().trim().is_empty() => (Receiver::Name(other.text().trim().to_string()), String::new()),
                _ => (Receiver::Unset, String::new()),
            };
            let (s, e) = notes.buffer().bounds();
            let program = Program {
                id: p.id,
                name: n,
                station: TRADE_HUBS.get(hub_dd.selected() as usize).map(|h| h.1).unwrap_or(TRADE_HUBS[0].1),
                basis: Basis::ALL.get(basis_dd.selected() as usize).copied().unwrap_or_default(),
                rate: r / 100.0,
                rules: rules.borrow().clone(),
                haul_per_m3: h,
                receiver,
                receiver_name,
                location: location.text().trim().to_string(),
                notes: notes.buffer().text(&s, &e, false).trim().to_string(),
                tolerance: t / 100.0,
            };
            let mut doc = bb::load_doc();
            let id = doc.put(program);
            select(&ctx, id);
            bb::save_doc(ctx.core(), &doc);
            win.close();
        });
    }
    actions.append(&cancel);
    actions.append(&save);
    win.present();
}
