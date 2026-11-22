//! Queue Health: training status across every character, grouped into the accounts the player sets up, so idle
//! queues, paused training, failing logins and ready injectors are visible at once.

use super::overview::fleet::{self, Updaters, CSS};
use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::widgets::{self as w, BannerKind, Btn, Status};
use gtk::prelude::*;
use podstack_core::features::accounts::{self, Account};
use podstack_core::features::plans;
use podstack_core::features::queue_health::{self as qh, Health, Level, LOW_QUEUE_OPTIONS};
use podstack_core::{fmt, Character, Core};
use std::sync::Arc;

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let core = ctx.core().clone();
    let accounts = accounts::accounts();
    let hours = qh::low_queue_hours(&core);

    let mut rows: Vec<(Arc<Character>, Health)> = core.characters().into_iter().map(|c| {
        let h = qh::check_in(&core, &c);
        (c, h)
    }).collect();
    rows.sort_by(|a, b| accounts::natural_cmp(&a.0.name(), &b.0.name()));

    let body = w::page_body();
    let new_account = w::button_icon("group_add", "New account", Btn::Filled);
    {
        let ctx = ctx.clone();
        let n = accounts.len();
        new_account.connect_clicked(move |_| name_dialog(&ctx, None, n));
    }

    if rows.is_empty() {
        body.append(&w::empty_state("monitor_heart", "No characters yet", "Add a character on the Overview page to watch its skill queue here."));
        return Page::new("Queue Health", body).action(new_account);
    }

    // tiles
    let count = |level: Level| rows.iter().filter(|(_, h)| h.has(level)).count();
    let not_training = rows.iter().filter(|(_, h)| !h.training).count();
    let low = count(Level::Warning);
    let extraction = count(Level::Info);
    let idle_accounts = accounts.iter().filter(|a| {
        let members: Vec<_> = rows.iter().filter(|(c, _)| accounts::account_of(c.id).as_deref() == Some(a.id.as_str())).collect();
        !members.is_empty() && members.iter().all(|(_, h)| !h.training)
    }).count();
    body.append(&w::tile_row(vec![
        fleet::tile(
            "pause_circle",
            "Not training",
            &not_training.to_string(),
            Some(if not_training > 0 { "Paused or empty queues" } else { "Every character is training" }),
            not_training > 0,
        ),
        fleet::tile("hourglass_bottom", "Low queues", &low.to_string(), Some(&format!("Ending within {}", qh::hours_label(hours))), low > 0),
        fleet::tile(
            "no_accounts",
            "Idle accounts",
            &idle_accounts.to_string(),
            Some(&format!("{} account{} set up", accounts.len(), if accounts.len() == 1 { "" } else { "s" })),
            idle_accounts > 0,
        ),
        fleet::tile("opacity", "Extraction", &extraction.to_string(), Some("Farms ready or due soon"), false),
    ]));

    // the low-queue window
    let window = w::hbox(16);
    let wl = w::line("Flag queues ending within", &["label-large", "dim"]);
    wl.set_valign(gtk::Align::Center);
    window.append(&wl);
    let labels: Vec<String> = LOW_QUEUE_OPTIONS.iter().map(|h| qh::hours_label(*h)).collect();
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let selected = LOW_QUEUE_OPTIONS.iter().position(|h| *h == hours).unwrap_or(1);
    {
        let core = core.clone();
        window.append(&w::segmented(&label_refs, selected, move |i| qh::set_low_queue_hours(&core, LOW_QUEUE_OPTIONS[i])));
    }
    body.append(&window);

    if accounts.is_empty() {
        body.append(&w::banner(
            "info",
            "EVE doesn't say which characters share an account. Use \"New account\" and pick an account for each \
             character to see when a whole account has nothing training.",
            BannerKind::Info,
        ));
    }

    let updaters = Updaters::default();
    for a in &accounts {
        let members: Vec<&(Arc<Character>, Health)> = rows.iter().filter(|(c, _)| accounts::account_of(c.id).as_deref() == Some(a.id.as_str())).collect();
        body.append(&group(ctx, &a.name, &members, Some(a), &accounts, accounts.len(), &updaters));
    }
    let unassigned: Vec<&(Arc<Character>, Health)> =
        rows.iter().filter(|(c, _)| !accounts.iter().any(|a| accounts::account_of(c.id).as_deref() == Some(a.id.as_str()))).collect();
    if !unassigned.is_empty() {
        let title = if accounts.is_empty() { "Characters" } else { "No account" };
        body.append(&group(ctx, title, &unassigned, None, &accounts, accounts.len(), &updaters));
    }
    // countdowns move with time even when no new data arrives
    updaters.tick(&body, 30);

    Page::new("Queue Health", body).action(new_account)
}

/// Count shown on the navigation drawer item (None hides the badge).
pub fn nav_badge(core: &podstack_core::Core) -> Option<u32> {
    Some(qh::count_problems(core))
}

fn group(
    ctx: &Ctx,
    title: &str,
    rows: &[&(Arc<Character>, Health)],
    account: Option<&Account>,
    all: &[Account],
    n_accounts: usize,
    updaters: &Updaters,
) -> gtk::Widget {
    let training = rows.iter().filter(|(_, h)| h.training).count();
    let idle = account.is_some() && !rows.is_empty() && training == 0;

    let card = w::vbox(0);
    card.add_css_class("card");
    card.add_css_class("flush");
    if idle {
        card.add_css_class("fleet-idle-card");
    }
    let head = w::hbox(12);
    head.add_css_class("fleet-account-head");
    let icon = w::icon(if account.is_some() { "account_circle" } else { "help_outline" });
    icon.add_css_class(if idle { "error-text" } else { "primary-text" });
    icon.set_valign(gtk::Align::Center);
    head.append(&icon);
    let titles = w::vbox(2);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    let top = w::hbox(8);
    top.append(&w::line(title, &["title-medium"]));
    if idle {
        top.append(&w::status("No character training", Status::Bad));
    }
    titles.append(&top);
    if !rows.is_empty() {
        titles.append(&w::line(&format!("{training} of {} training", rows.len()), &["body-medium", "dim"]));
    }
    head.append(&titles);
    if let Some(a) = account {
        let rename = w::button("Rename", Btn::Text);
        {
            let ctx = ctx.clone();
            let a = a.clone();
            rename.connect_clicked(move |_| name_dialog(&ctx, Some(&a), n_accounts));
        }
        let delete = w::button("Delete", Btn::Danger);
        {
            let ctx = ctx.clone();
            let a = a.clone();
            delete.connect_clicked(move |_| {
                let core = ctx.core().clone();
                let id = a.id.clone();
                w::confirm(
                    &ctx,
                    "Delete account",
                    &format!("Delete \"{}\"? Its characters become unassigned.", a.name),
                    "Delete",
                    true,
                    move || accounts::delete(&core, &id),
                );
            });
        }
        rename.set_valign(gtk::Align::Center);
        delete.set_valign(gtk::Align::Center);
        head.append(&rename);
        head.append(&delete);
    }
    card.append(&head);

    if rows.is_empty() {
        let l = w::text("No characters yet. Pick this account in a character's account menu.", &["body-medium", "dim"]);
        l.set_margin_start(16);
        l.set_margin_end(16);
        l.set_margin_bottom(16);
        card.append(&l);
        return card.upcast();
    }

    let list = fleet::list();
    for (c, h) in rows {
        list.append(&character_row(ctx, c, h, all, updaters));
    }
    fleet::activate_rows(ctx, &list, 0, rows.iter().map(|(c, _)| c.id).collect(), CharTab::Summary);
    card.append(&list);
    card.upcast()
}

fn meta_text(c: &Character, h: &Health) -> String {
    match (c.current_skill(), h.queue_ends) {
        (Some(q), Some(end)) => format!("Training {} · queue ends in {}", q.skill_name, fmt::duration(end - chrono::Utc::now(), 3)),
        (Some(q), None) => format!("Training {}", q.skill_name),
        _ => "Not training".into(),
    }
}

fn fill_flags(b: &gtk::FlowBox, h: &Health) {
    b.remove_all();
    if h.flags.is_empty() {
        b.append(&w::status("OK", Status::Good));
    }
    for f in &h.flags {
        let kind = match f.level {
            Level::Critical => Status::Bad,
            Level::Warning => Status::Warn,
            Level::Info => Status::Info,
        };
        b.append(&w::status(&f.text, kind));
    }
}

fn character_row(ctx: &Ctx, c: &Arc<Character>, h: &Health, accounts_list: &[Account], updaters: &Updaters) -> gtk::Widget {
    let core = ctx.core().clone();
    let id = c.id;
    let row = w::hbox(16);
    row.add_css_class("fleet-row");
    row.append(&fleet::portrait(&core, c, 40));

    let texts = w::vbox(2);
    texts.set_valign(gtk::Align::Center);
    texts.set_size_request(260, -1);
    let name = w::hbox(8);
    name.append(&w::line(&c.name(), &["title-small"]));
    if let Some(b) = fleet::clone_badge(c) {
        name.append(&b);
    }
    texts.append(&name);
    let meta = w::line(&meta_text(c, h), &["body-small", "dim"]);
    texts.append(&meta);
    row.append(&texts);

    // flags wrap onto more lines rather than widening the row
    let flag_box = gtk::FlowBox::new();
    flag_box.set_selection_mode(gtk::SelectionMode::None);
    flag_box.set_hexpand(true);
    flag_box.set_valign(gtk::Align::Center);
    flag_box.set_column_spacing(6);
    flag_box.set_row_spacing(6);
    flag_box.set_min_children_per_line(1);
    flag_box.set_max_children_per_line(4);
    flag_box.set_can_target(false);
    flag_box.set_size_request(200, -1);
    fill_flags(&flag_box, h);
    row.append(&flag_box);
    {
        let core = core.clone();
        let flag_box = flag_box.clone();
        updaters.add(move || {
            if let Some(c) = core.character(id) {
                let h = qh::check_in(&core, &c);
                meta.set_text(&meta_text(&c, &h));
                fill_flags(&flag_box, &h);
            }
        });
    }

    // a character with nothing to train can start a skill plan
    let plan_slot = w::hbox(0);
    plan_slot.set_size_request(40, -1);
    if !h.training {
        let plan = w::icon_button("playlist_add", "New skill plan");
        let ctx = ctx.clone();
        plan.connect_clicked(move |_| new_plan(&ctx, id));
        plan_slot.append(&plan);
    }
    row.append(&plan_slot);

    // account
    let mut names = vec!["No account".to_string()];
    names.extend(accounts_list.iter().map(|a| a.name.clone()));
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let current = accounts::account_of(id).and_then(|aid| accounts_list.iter().position(|a| a.id == aid)).map(|i| i + 1).unwrap_or(0);
    let dd = w::dropdown(&refs, current as u32);
    dd.set_tooltip_text(Some("Account"));
    dd.set_size_request(180, -1);
    let ids: Vec<String> = accounts_list.iter().map(|a| a.id.clone()).collect();
    dd.connect_selected_notify(move |dd| {
        let i = dd.selected() as usize;
        let account = if i == 0 { None } else { ids.get(i - 1).map(String::as_str) };
        accounts::assign(&core, id, account);
    });
    row.append(&dd);
    row.upcast()
}

/// Creates an empty skill plan for the character and opens its plans.
fn new_plan(ctx: &Ctx, id: i64) {
    let core: &Core = ctx.core();
    let n = plans::plans_for(id).len();
    plans::create(core, id, &format!("Plan {}", n + 1));
    ctx.navigate(Route::Character(id, CharTab::Plans));
}

/// Name an account: a new one (`account` None) or rename it.
fn name_dialog(ctx: &Ctx, account: Option<&Account>, n_accounts: usize) {
    let (win, content, actions) = w::dialog(ctx, if account.is_some() { "Rename account" } else { "New account" }, 380);
    content.append(&w::text("Account name", &["label-large"]));
    let entry = gtk::Entry::new();
    entry.set_text(&account.map(|a| a.name.clone()).unwrap_or_else(|| format!("Account {}", n_accounts + 1)));
    entry.set_activates_default(true);
    content.append(&entry);
    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button(if account.is_some() { "Rename" } else { "Create" }, Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    let account_id = account.map(|a| a.id.clone());
    let submit = {
        let win = win.clone();
        let ctx = ctx.clone();
        let entry = entry.clone();
        move || {
            let name = entry.text().trim().to_string();
            if name.is_empty() {
                return;
            }
            match &account_id {
                Some(id) => accounts::rename(ctx.core(), id, &name),
                None => {
                    accounts::create(ctx.core(), &name);
                }
            }
            win.close();
        }
    };
    {
        let submit = submit.clone();
        save.connect_clicked(move |_| submit());
    }
    entry.connect_activate(move |_| submit());
    actions.append(&cancel);
    actions.append(&save);
    win.present();
    entry.grab_focus();
    entry.select_region(0, -1);
}
