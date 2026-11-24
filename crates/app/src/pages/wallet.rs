//! Wallet: the wallets of every character together, laid out like the character Wallet tab, and where the ISK comes
//! from and goes to over a chosen period (income by source, spending by category, income over time, the characters
//! side by side, the activities that pay best), with a drill-down from a category to its journal entries.

use crate::ctx::{CharTab, Ctx, Page, Route};
use crate::pages::character::wallet::{date_cell, isk_cell, source, CSS as WALLET_CSS, SCOPE};
use crate::pages::contracts::overview::bar;
use crate::pages::market_orders::{character_cell, label_cell, set_child, type_cell};
use crate::widgets::{self as w, BannerKind, Btn, Table};
use chrono::{DateTime, NaiveDate, Utc};
use gtk::prelude::*;
use podstack_core::domains::wallet::Transaction;
use podstack_core::features::comms;
use podstack_core::features::wallet_flow::{self as flow, Analysis, Bucket, BucketSize, Category, CharacterTotal, Line, Period};
use podstack_core::{fmt, Character};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const PERIOD_KEY: &str = "wallets.period";
const CHAR_KEY: &str = "wallets.char";
const DRILL_KEY: &str = "wallets.drill";
const VIEW_KEY: &str = "wallets.view";
const SEARCH_KEY: &str = "wallets.search";

const CSS: &str = "
.wlt-split > * { min-width: 0; }
.wlt-row { padding: 6px 8px; border-radius: 12px; }
.wlt-row.wlt-selected { background-color: alpha(@primary, 0.12); }
.wlt-row .wlt-icon { color: @primary; }
.wlt-row.wlt-out .wlt-icon { color: @tertiary; }
.wlt-drill-chip { padding: 2px 4px 2px 12px; border-radius: 8px; background-color: @secondary_container; color: @on_secondary_container; }
";

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(WALLET_CSS);
    crate::theme::add_css(CSS);
    let body = w::page_body();
    let all_chars = ctx.core().characters();
    if all_chars.is_empty() {
        body.append(&w::empty_state("account_balance_wallet", "No characters yet", "Add a character to see its wallet here."));
        return Page::new("Wallet", body);
    }

    let now = Utc::now();
    let period = ctx.recall(PERIOD_KEY).and_then(|k| Period::from_key(&k)).unwrap_or(Period::Days(30));
    let since = period.since(now);
    let only: Option<i64> = ctx.recall(CHAR_KEY).and_then(|v| v.parse().ok()).filter(|id| all_chars.iter().any(|c| c.id == *id));
    let chars: Vec<Arc<Character>> = all_chars.iter().filter(|c| only.is_none_or(|id| c.id == id)).cloned().collect();
    let names: Rc<HashMap<i64, String>> = Rc::new(all_chars.iter().map(|c| (c.id, c.name())).collect());

    let all_lines = flow::lines(&all_chars, since);
    let all_ids: Vec<i64> = all_chars.iter().map(|c| c.id).collect();
    let everyone = flow::analyze(&all_lines, &all_ids, since, now);
    let lines: Rc<Vec<Line>> = Rc::new(match only {
        Some(id) => all_lines.iter().filter(|l| l.char_id == id).cloned().collect(),
        None => all_lines,
    });
    let a = match only {
        Some(id) => flow::analyze(&lines, &[id], since, now),
        None => everyone.clone(),
    };
    let drill: Option<Category> = ctx.recall(DRILL_KEY).and_then(|k| Category::from_key(&k));

    body.append(&filter_bar(ctx, &all_chars, period, only));

    // characters whose journal can't load: say so, and count the rest
    let missing = flow::missing_scope(&chars);
    if !missing.is_empty() {
        if only.is_some() || missing.len() == chars.len() {
            body.append(&w::scope_notice(SCOPE));
        } else {
            let who: Vec<String> = missing.iter().map(|c| c.name()).collect();
            body.append(&w::banner(
                "lock",
                &format!("Not counted: {}. Their logins don't include {SCOPE}; log in again with \"Add character\" to grant it.", who.join(", ")),
                BannerKind::Notice,
            ));
        }
    }
    let waiting = chars.iter().filter(|c| c.wallet_journal.is_none() && !missing.iter().any(|m| m.id == c.id)).count();
    if waiting > 0 {
        body.append(&w::text(
            &format!("Waiting for the wallet journal of {waiting} character{}; it appears after the next refresh.", if waiting == 1 { "" } else { "s" }),
            &["body-medium", "dim"],
        ));
    }

    // headline numbers beside the chart, like the character tab
    let top = w::hbox(16);
    top.append(&tiles(&chars, &a, period));
    let chart_body: gtk::Widget = if a.buckets.iter().any(|b| b.totals.count > 0) {
        income_chart(a.buckets.clone(), a.bucket_size.unwrap_or(BucketSize::Day))
    } else {
        w::text("No wallet activity in this period.", &["body-medium", "dim"]).upcast()
    };
    let per = match a.bucket_size {
        Some(BucketSize::Week) => "Per week",
        Some(BucketSize::Month) => "Per month",
        _ => "Per day",
    };
    let chart = w::card_with("Income over time", Some(&format!("{per}, EVE time · income and spending")), &[], &chart_body);
    chart.set_hexpand(true);
    top.append(&chart);
    body.append(&top);

    // where it comes from, where it goes
    let split = w::hbox(16);
    split.add_css_class("wlt-split");
    split.set_homogeneous(true);
    let income = breakdown(ctx, &a.income(), a.totals.income, false, drill, "No income in this period.");
    split.append(&w::card_with("Income by source", Some(&format!("{} ISK", fmt::isk_short(a.totals.income))), &[], &income));
    let spending = breakdown(ctx, &a.spending(), a.totals.spending, true, drill, "No spending in this period.");
    split.append(&w::card_with("Spending by category", Some(&format!("{} ISK", fmt::isk_short(a.totals.spending))), &[], &spending));
    body.append(&split);

    let split = w::hbox(16);
    split.add_css_class("wlt-split");
    split.set_homogeneous(true);
    split.append(&w::card_with("Top earning activities", Some("Net of each activity's own costs"), &[], &activities(&a)));
    split.append(&w::card_with(
        "Characters",
        Some(if only.is_some() { "All characters, this period · double-click to show one" } else { "This period · double-click to show one" }),
        &[],
        &characters_table(ctx, &everyone.characters, &all_chars),
    ));
    body.append(&split);

    body.append(&journal_section(ctx, &chars, lines, drill, since, names, period));
    Page::new("Wallet", body)
}

/// Remembers a choice and rebuilds the page once the clicked widget's handler has returned (the rebuild replaces it).
fn choose(ctx: &Ctx, key: &str, value: String) {
    ctx.remember(key, value);
    let ctx = ctx.clone();
    gtk::glib::idle_add_local_once(move || ctx.rebuild());
}

fn filter_bar(ctx: &Ctx, chars: &[Arc<Character>], period: Period, only: Option<i64>) -> gtk::Box {
    let bar = w::hbox(12);
    bar.add_css_class("comms-filter-bar");
    let labels: Vec<String> = Period::ALL.iter().map(|p| p.label()).collect();
    let labels: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    let selected = Period::ALL.iter().position(|p| *p == period).unwrap_or(1);
    {
        let ctx = ctx.clone();
        bar.append(&w::segmented(&labels, selected, move |i| choose(&ctx, PERIOD_KEY, Period::ALL[i].key())));
    }
    let mut items = vec!["All characters".to_string()];
    items.extend(chars.iter().map(|c| c.name()));
    let items: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
    let pick = w::dropdown(&items, only.and_then(|id| chars.iter().position(|c| c.id == id)).map(|i| i as u32 + 1).unwrap_or(0));
    {
        let (ctx, ids) = (ctx.clone(), chars.iter().map(|c| c.id).collect::<Vec<_>>());
        pick.connect_selected_notify(move |d| {
            let v = (d.selected() as usize).checked_sub(1).and_then(|i| ids.get(i)).map(|id| id.to_string()).unwrap_or_default();
            choose(&ctx, CHAR_KEY, v);
        });
    }
    bar.append(&pick);
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let updated = chars.iter().filter(|c| only.is_none_or(|id| c.id == id)).filter_map(|c| c.refreshes.get("wallet_journal").and_then(|r| r.last)).max();
    if let Some(t) = updated {
        let l = w::line(&format!("Updated {}", fmt::relative(t)), &["body-small", "dim"]);
        l.set_valign(gtk::Align::Center);
        bar.append(&l);
    }
    bar
}

fn signed_short(v: f64) -> String {
    format!("{}{}", if v > 0.0 { "+" } else { "" }, fmt::isk_short(v))
}

fn tiles(chars: &[Arc<Character>], a: &Analysis, period: Period) -> gtk::Grid {
    let tiles = gtk::Grid::new();
    tiles.add_css_class("comms-tile-grid");
    tiles.set_row_spacing(12);
    tiles.set_column_spacing(12);
    tiles.set_valign(gtk::Align::Start);
    let known: Vec<f64> = chars.iter().filter_map(|c| c.wallet).collect();
    let balance: f64 = known.iter().sum();
    let balance_tile = w::stat_tile(
        "Balance",
        &if known.is_empty() { "—".into() } else { format!("{} ISK", fmt::isk_short(balance)) },
        Some(&if chars.len() == 1 { fmt::isk(balance) } else { format!("{} wallets", known.len()) }),
    );
    balance_tile.set_tooltip_text(Some(&fmt::isk(balance)));
    balance_tile.add_css_class("accent");
    tiles.attach(&balance_tile, 0, 0, 1, 1);
    let when = match (period, a.first) {
        (Period::All, Some(first)) => format!("since {}", first.format("%b %-d, %Y")),
        (Period::All, None) => "all kept".into(),
        (p, _) => p.label(),
    };
    let income = w::stat_tile(&format!("Income · {when}"), &format!("{} ISK", fmt::isk_short(a.totals.income)), None);
    income.set_tooltip_text(Some(&fmt::isk(a.totals.income)));
    tiles.attach(&income, 1, 0, 1, 1);
    let spent = w::stat_tile(&format!("Spending · {when}"), &format!("{} ISK", fmt::isk_short(a.totals.spending)), None);
    spent.set_tooltip_text(Some(&fmt::isk(a.totals.spending)));
    tiles.attach(&spent, 0, 1, 1, 1);
    let net = a.totals.net();
    let net_tile = w::stat_tile(&format!("Net · {when}"), &format!("{} ISK", signed_short(net)), None);
    net_tile.set_tooltip_text(Some(&fmt::isk(net)));
    if let Some(v) = net_tile.first_child().and_then(|l| l.next_sibling()) {
        v.add_css_class(if net < 0.0 { "comms-isk-out" } else { "comms-isk-in" });
    }
    tiles.attach(&net_tile, 1, 1, 1, 1);
    tiles
}

/// One row per category: icon, name, a bar for its share and the amount. Clicking a row shows its entries in the
/// journal below (clicking it again shows all).
fn breakdown(ctx: &Ctx, items: &[(Category, f64)], total: f64, spending: bool, drill: Option<Category>, empty: &str) -> gtk::Widget {
    if items.is_empty() {
        return w::text(empty, &["body-medium", "dim"]).upcast();
    }
    let list = w::vbox(2);
    let biggest = items.first().map(|i| i.1).unwrap_or(1.0).max(1.0);
    for (cat, v) in items {
        let row = w::hbox(12);
        row.add_css_class("wlt-row");
        if spending {
            row.add_css_class("wlt-out");
        }
        if drill == Some(*cat) {
            row.add_css_class("wlt-selected");
        }
        let icon = w::icon(cat.icon());
        icon.add_css_class("wlt-icon");
        row.append(&icon);
        let mid = w::vbox(4);
        mid.set_hexpand(true);
        let top = w::hbox(8);
        let name = w::line(cat.label(), &["body-medium"]);
        name.set_hexpand(true);
        name.set_xalign(0.0);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        top.append(&name);
        let share = w::line(&format!("{:.0}%", v / total.max(1.0) * 100.0), &["body-small", "dim", "numeric"]);
        share.set_ellipsize(gtk::pango::EllipsizeMode::None);
        top.append(&share);
        mid.append(&top);
        let p = w::progress(v / biggest);
        if spending {
            p.add_css_class("tertiary");
        }
        mid.append(&p);
        row.append(&mid);
        let amount = w::line(&fmt::isk_short(*v), &["title-small", "numeric"]);
        amount.set_width_chars(8);
        amount.set_xalign(1.0);
        row.append(&amount);

        let button = gtk::Button::new();
        button.add_css_class("flat");
        button.set_child(Some(&row));
        button.set_tooltip_text(Some(&format!("{} ISK · click to show its journal entries", fmt::isk(*v))));
        let (ctx, cat) = (ctx.clone(), *cat);
        button.connect_clicked(move |_| choose(&ctx, DRILL_KEY, if drill == Some(cat) { String::new() } else { cat.key() }));
        list.append(&button);
    }
    list.upcast()
}

fn activities(a: &Analysis) -> gtk::Widget {
    let earners = a.earners();
    if earners.is_empty() {
        return w::text("Nothing made ISK in this period.", &["body-medium", "dim"]).upcast();
    }
    let list = w::vbox(2);
    let total: f64 = earners.iter().map(|x| x.totals.net()).sum();
    let best = earners[0].totals.net().max(1.0);
    for (rank, x) in earners.iter().enumerate() {
        let row = w::hbox(12);
        row.add_css_class("wlt-row");
        let icon = w::icon(x.activity.icon());
        icon.add_css_class("wlt-icon");
        row.append(&icon);
        let mid = w::vbox(4);
        mid.set_hexpand(true);
        let top = w::hbox(8);
        let name = w::line(&format!("{}. {}", rank + 1, x.activity.label()), &["body-medium"]);
        name.set_hexpand(true);
        name.set_xalign(0.0);
        top.append(&name);
        let share = w::line(&format!("{:.0}% of earnings", x.totals.net() / total.max(1.0) * 100.0), &["body-small", "dim", "numeric"]);
        share.set_ellipsize(gtk::pango::EllipsizeMode::None);
        top.append(&share);
        mid.append(&top);
        mid.append(&w::progress(x.totals.net() / best));
        let detail = if x.totals.spending > 0.0 {
            format!("Made {} · costs {}", fmt::isk_short(x.totals.income), fmt::isk_short(x.totals.spending))
        } else {
            format!("Made {}", fmt::isk_short(x.totals.income))
        };
        mid.append(&w::line(&detail, &["body-small", "dim"]));
        row.append(&mid);
        let net = w::line(&signed_short(x.totals.net()), &["title-small", "numeric", "comms-isk-in"]);
        net.set_width_chars(8);
        net.set_xalign(1.0);
        row.append(&net);
        row.set_tooltip_text(Some(&format!("{} ISK net\nCounts: {}", fmt::isk(x.totals.net()), x.activity.covers())));
        list.append(&row);
    }
    // the costs that don't belong to an earning activity, so the list adds up to something
    let losers: Vec<_> = a.activities.iter().filter(|x| x.totals.net() < 0.0).collect();
    if !losers.is_empty() {
        let parts: Vec<String> = losers.iter().take(3).map(|x| format!("{} {}", x.activity.label(), fmt::isk_short(x.totals.net()))).collect();
        let l = w::text(&format!("Costing ISK: {}", parts.join(" · ")), &["body-small", "dim"]);
        l.set_margin_top(8);
        l.set_margin_start(8);
        list.append(&l);
    }
    list.upcast()
}

/// A row of the character comparison.
#[derive(Clone)]
struct CharRow {
    id: i64,
    name: String,
    balance: Option<f64>,
    t: CharacterTotal,
}

fn characters_table(ctx: &Ctx, totals: &[CharacterTotal], chars: &[Arc<Character>]) -> gtk::Widget {
    let rows: Vec<CharRow> = totals
        .iter()
        .filter_map(|t| {
            let c = chars.iter().find(|c| c.id == t.char_id)?;
            Some(CharRow { id: c.id, name: c.name(), balance: c.wallet, t: t.clone() })
        })
        .collect();
    let ctx = ctx.clone();
    let cmp = |a: f64, b: f64| a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
    Table::new(rows)
        .widget_sorted("Character", |r| character_cell(r.id, &r.name), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .num("Balance", |r| r.balance.map(fmt::isk_short).unwrap_or_else(|| "—".into()), |r| r.balance.unwrap_or(0.0))
        .num("Income", |r| fmt::isk_short(r.t.totals.income), |r| r.t.totals.income)
        .num("Spending", |r| fmt::isk_short(r.t.totals.spending), |r| r.t.totals.spending)
        .widget_sorted(
            "Net",
            |r| {
                let n = r.t.totals.net();
                let l = w::line(&signed_short(n), &["numeric"]);
                l.set_xalign(1.0);
                l.set_tooltip_text(Some(&fmt::isk(n)));
                if n != 0.0 {
                    l.add_css_class(if n > 0.0 { "comms-isk-in" } else { "comms-isk-out" });
                }
                l.upcast()
            },
            move |a, b| cmp(a.t.totals.net(), b.t.totals.net()),
        )
        .widget("Earns most by", |r| match r.t.top {
            Some((act, net)) => {
                let l = label_cell(act.label(), &[]);
                l.set_tooltip_text(Some(&format!("{} ISK net", fmt::isk(net))));
                l
            }
            None => label_cell("—", &["dim"]),
        })
        .width(150)
        .sort_by(4, true)
        .on_activate(move |r| choose(&ctx, CHAR_KEY, r.id.to_string()))
        .natural_height()
        .build()
}

// ---------- journal and transactions ----------

fn journal_section(
    ctx: &Ctx,
    chars: &[Arc<Character>],
    lines: Rc<Vec<Line>>,
    drill: Option<Category>,
    since: Option<DateTime<Utc>>,
    names: Rc<HashMap<i64, String>>,
    period: Period,
) -> gtk::Widget {
    // the market transactions of the period, and of the drilled category's entries when there is one
    let refs: Option<HashSet<(i64, i64)>> = drill.map(|d| lines.iter().filter(|l| l.category == d).map(|l| (l.char_id, l.entry.id)).collect());
    let mut transactions: Vec<(i64, Transaction)> = chars
        .iter()
        .flat_map(|c| c.wallet_transactions.iter().flat_map(|t| t.transactions.iter()).map(move |t| (c.id, t)))
        .filter(|(_, t)| since.is_none_or(|s| t.date >= s))
        .filter(|(id, t)| refs.as_ref().is_none_or(|r| t.journal_ref_id.is_some_and(|j| r.contains(&(*id, j)))))
        .map(|(id, t)| (id, t.clone()))
        .collect();
    transactions.sort_by_key(|t| std::cmp::Reverse(t.1.date));
    let transactions = Rc::new(transactions);
    let shown: Rc<Vec<Line>> = Rc::new(match drill {
        Some(d) => lines.iter().filter(|l| l.category == d).cloned().collect(),
        None => (*lines).clone(),
    });
    let multi = chars.len() > 1;

    let transactions_view = ctx.recall(VIEW_KEY).as_deref() == Some("transactions");
    let state = Rc::new(RefCell::new((transactions_view, ctx.recall(SEARCH_KEY).unwrap_or_default())));
    let holder = w::vbox(0);
    let refill = {
        let (holder, state, shown, transactions, names) = (holder.clone(), state.clone(), shown.clone(), transactions.clone(), names.clone());
        let when = if period == Period::All { "kept".to_string() } else { format!("in the last {}", period.label()) };
        Rc::new(move || {
            let (tx_view, query) = state.borrow().clone();
            let q = query.trim().to_lowercase();
            let matches = |fields: &[Option<&str>]| q.is_empty() || fields.iter().flatten().any(|t| t.to_lowercase().contains(&q));
            let name = |id: i64| names.get(&id).cloned().unwrap_or_default();
            if tx_view {
                let rows: Vec<(i64, Transaction)> = transactions
                    .iter()
                    .filter(|(id, t)| matches(&[Some(&t.name), t.client.as_deref(), Some(&t.location.name), Some(&name(*id))]))
                    .cloned()
                    .collect();
                let empty = if !q.is_empty() {
                    "Nothing matches.".to_string()
                } else if drill.is_some() {
                    "No market transactions in this category.".to_string()
                } else {
                    format!("No market transactions {when}.")
                };
                set_child(&holder, &transactions_table(rows, names.clone(), multi, &empty));
            } else {
                let rows: Vec<Line> = shown
                    .iter()
                    .filter(|l| {
                        let e = &l.entry;
                        matches(&[
                            Some(l.category.label()),
                            Some(&comms::title_case(&e.ref_type)),
                            Some(&e.description),
                            e.reason.as_deref(),
                            e.first_party.as_deref(),
                            e.second_party.as_deref(),
                            l.item.as_ref().map(|i| i.name.as_str()),
                            Some(&name(l.char_id)),
                        ])
                    })
                    .cloned()
                    .collect();
                let empty = if q.is_empty() { format!("No wallet activity {when}.") } else { "Nothing matches.".to_string() };
                set_child(&holder, &journal_table(rows, names.clone(), multi, &empty));
            }
        })
    };

    let bar = w::hbox(12);
    bar.add_css_class("comms-filter-bar");
    let journal_label = format!("Journal ({})", shown.len());
    let tx_label = format!("Transactions ({})", transactions.len());
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        bar.append(&w::segmented(&[&journal_label, &tx_label], transactions_view as usize, move |i| {
            ctx.remember(VIEW_KEY, if i == 1 { "transactions" } else { "journal" });
            state.borrow_mut().0 = i == 1;
            refill();
        }));
    }
    if let Some(d) = drill {
        let chip = w::hbox(4);
        chip.add_css_class("wlt-drill-chip");
        chip.set_valign(gtk::Align::Center);
        let l = w::line(d.label(), &["label-large"]);
        l.set_valign(gtk::Align::Center);
        chip.append(&l);
        let clear = w::icon_button("close", "Show every category");
        let ctx = ctx.clone();
        clear.connect_clicked(move |_| choose(&ctx, DRILL_KEY, String::new()));
        chip.append(&clear);
        bar.append(&chip);
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let search = w::search_entry("Search category, type, description, party, item, character");
    search.set_text(&state.borrow().1);
    search.set_hexpand(false);
    search.set_max_width_chars(40);
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        search.connect_search_changed(move |s| {
            ctx.remember(SEARCH_KEY, s.text().as_str());
            state.borrow_mut().1 = s.text().to_string();
            refill();
        });
    }
    bar.append(&search);

    let section = w::vbox(8);
    section.append(&bar);
    section.append(&holder);
    refill();
    let open = w::button_icon("open_in_new", "Character wallet", Btn::Text);
    open.set_tooltip_text(Some("Open the character's own Wallet tab"));
    let actions: Vec<gtk::Widget> = match chars {
        [c] => {
            let (ctx, id) = (ctx.clone(), c.id);
            open.connect_clicked(move |_| ctx.navigate(Route::Character(id, CharTab::Wallet)));
            vec![open.upcast()]
        }
        _ => vec![],
    };
    let subtitle = match drill {
        Some(d) => format!("{} · its entries only", d.label()),
        None => "Every entry of the period".into(),
    };
    w::card_with("Journal", Some(&subtitle), &actions, &section).upcast()
}

fn journal_table(rows: Vec<Line>, names: Rc<HashMap<i64, String>>, multi: bool, empty: &str) -> gtk::Widget {
    let mut t = Table::new(rows).widget_sorted("Date", |l| date_cell(l.entry.date), |a, b| a.entry.date.cmp(&b.entry.date).then(a.entry.id.cmp(&b.entry.id)));
    if multi {
        let n = names.clone();
        t = t
            .widget_sorted(
                "Character",
                move |l| character_cell(l.char_id, names.get(&l.char_id).map(|s| s.as_str()).unwrap_or("")),
                move |a, b| n.get(&a.char_id).cmp(&n.get(&b.char_id)),
            )
            .width(170);
    }
    t.widget_sorted(
        "Category",
        |l| {
            let c = label_cell(l.category.label(), &[]);
            c.set_tooltip_text(Some(&comms::title_case(&l.entry.ref_type)));
            c
        },
        |a, b| a.category.label().cmp(b.category.label()),
    )
    .width(220)
    .widget_sorted(
        "Amount",
        |l| {
            let c = isk_cell(l.entry.amount);
            if l.amount == 0.0 && l.entry.amount != 0.0 {
                c.set_tooltip_text(Some("Paid from a buy order's escrow: counted with the Market Escrow entry"));
            }
            c
        },
        |a, b| a.entry.amount.partial_cmp(&b.entry.amount).unwrap_or(std::cmp::Ordering::Equal))
    .widget("Description", |l| {
        let e = &l.entry;
        let b = w::vbox(2);
        match &l.item {
            Some(i) => {
                let row = w::hbox(8);
                row.append(&type_cell(i.type_id, &i.name));
                row.append(&w::line(&format!("× {}", fmt::int(i.quantity)), &["dim", "numeric"]));
                b.append(&row);
            }
            None => b.append(&w::line(&e.description, &[])),
        }
        if let Some(r) = &e.reason {
            b.append(&w::line(r, &["body-small", "dim"]));
        }
        let parties: Vec<&str> = [e.first_party.as_deref(), e.second_party.as_deref()].into_iter().flatten().collect();
        let mut tip = format!("{}\n{}", comms::title_case(&e.ref_type), e.description);
        if !parties.is_empty() {
            tip.push_str(&format!("\n{}", parties.join(" → ")));
        }
        if let Some(t) = e.tax.filter(|t| *t != 0.0) {
            tip.push_str(&format!("\nTax: {}", fmt::isk(t)));
        }
        b.set_tooltip_text(Some(&tip));
        b.upcast()
    })
    .expand()
    .sort_by(0, true)
    .max_height(560)
    .empty_text(empty)
    .build()
}

fn transactions_table(rows: Vec<(i64, Transaction)>, names: Rc<HashMap<i64, String>>, multi: bool, empty: &str) -> gtk::Widget {
    let mut t = Table::new(rows).widget_sorted(
        "Date",
        |(_, t)| date_cell(t.date),
        |a, b| a.1.date.cmp(&b.1.date).then(a.1.transaction_id.cmp(&b.1.transaction_id)),
    );
    if multi {
        let n = names.clone();
        t = t
            .widget_sorted(
                "Character",
                move |(id, _)| character_cell(*id, names.get(id).map(|s| s.as_str()).unwrap_or("")),
                move |a, b| n.get(&a.0).cmp(&n.get(&b.0)),
            )
            .width(170);
    }
    t.widget_sorted("Item", |(_, t)| type_cell(t.type_id, &t.name), |a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
        .expand()
        .num("Qty", |(_, t)| fmt::int(t.quantity), |(_, t)| t.quantity as f64)
        .num("Price", |(_, t)| fmt::decimal(t.unit_price, 2), |(_, t)| t.unit_price)
        .widget_sorted("Total", |(_, t)| isk_cell(t.total()), |a, b| a.1.total().partial_cmp(&b.1.total()).unwrap_or(std::cmp::Ordering::Equal))
        .widget("Client", |(_, t)| label_cell(t.client.as_deref().unwrap_or(""), &[]))
        .width(170)
        .text("Where", |(_, t)| t.location.name.clone())
        .width(260)
        .sort_by(0, true)
        .max_height(560)
        .empty_text(empty)
        .build()
}

// ---------- income over time ----------

const CHART_HEIGHT: i32 = 200;
const PAD_L: f64 = 64.0;
const PAD_R: f64 = 16.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 26.0;

/// Income (primary) and spending (tertiary) side by side per bucket, like the Contracts overview chart; hovering a
/// bucket shows its numbers.
fn income_chart(buckets: Vec<Bucket>, size: BucketSize) -> gtk::Widget {
    let n = buckets.len().max(1);
    let top = buckets.iter().map(|b| b.totals.income.max(b.totals.spending)).fold(0.0f64, f64::max).max(1.0) * 1.08;
    let ticks = comms::nice_ticks(0.0, top, 4);
    let top = ticks.last().copied().unwrap_or(top).max(top);
    let buckets = Rc::new(buckets);
    let hover: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let plot_w = |w: f64| (w - PAD_L - PAD_R).max(1.0);
    let slot = move |w: f64| plot_w(w) / n as f64;
    let y_of = move |h: f64, v: f64| PAD_T + (1.0 - v / top) * (h - PAD_T - PAD_B).max(1.0);
    let label = move |d: NaiveDate| if size == BucketSize::Month { d.format("%b %Y").to_string() } else { d.format("%b %-d").to_string() };

    let area = gtk::DrawingArea::new();
    area.set_content_height(CHART_HEIGHT);
    area.set_hexpand(true);
    {
        let (buckets, hover) = (buckets.clone(), hover.clone());
        area.set_draw_func(move |_, cr, width, height| {
            let (wd, ht) = (width as f64, height as f64);
            cr.select_font_face("Inter", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(11.0);
            for v in &ticks {
                let y = y_of(ht, *v).round() + 0.5;
                source(cr, "outline_variant", if *v == 0.0 { 1.0 } else { 0.5 });
                cr.set_line_width(1.0);
                cr.move_to(PAD_L, y);
                cr.line_to(PAD_L + plot_w(wd), y);
                let _ = cr.stroke();
                let text = fmt::isk_short(*v);
                if let Ok(ext) = cr.text_extents(&text) {
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(PAD_L - 8.0 - ext.width() - ext.x_bearing(), y + ext.height() / 2.0);
                    let _ = cr.show_text(&text);
                }
            }
            let s = slot(wd);
            let count = ((plot_w(wd) / 90.0).floor() as usize).clamp(2, 8).min(buckets.len());
            for k in 0..count {
                let i = if count <= 1 { 0 } else { k * (buckets.len() - 1) / (count - 1) };
                let text = label(buckets[i].start);
                if let Ok(ext) = cr.text_extents(&text) {
                    let x = (PAD_L + (i as f64 + 0.5) * s - ext.width() / 2.0).clamp(PAD_L, (PAD_L + plot_w(wd) - ext.width()).max(PAD_L));
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(x, ht - 8.0);
                    let _ = cr.show_text(&text);
                }
            }
            if let Some(i) = hover.get() {
                source(cr, "on_surface", 0.06);
                cr.rectangle(PAD_L + i as f64 * s, PAD_T, s, ht - PAD_T - PAD_B);
                let _ = cr.fill();
            }
            let zero = y_of(ht, 0.0);
            let bw = (s * 0.36).clamp(1.0, 18.0);
            for (i, b) in buckets.iter().enumerate() {
                let cx = PAD_L + (i as f64 + 0.5) * s;
                let dim = hover.get().is_some_and(|h| h != i);
                for (v, role, x) in [(b.totals.income, "primary", cx - bw - 1.0), (b.totals.spending, "tertiary", cx + 1.0)] {
                    if v <= 0.0 {
                        continue;
                    }
                    let y = y_of(ht, v);
                    source(cr, role, if dim { 0.45 } else { 1.0 });
                    bar(cr, x, y, bw, (zero - y).max(1.0));
                    let _ = cr.fill();
                }
            }
        });
    }

    let tip = w::vbox(2);
    tip.add_css_class("comms-chart-tip");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);
    let tip_day = w::line("", &["body-small", "dim"]);
    let tip_net = w::line("", &["title-small", "numeric"]);
    let tip_detail = w::line("", &["body-small"]);
    for l in [&tip_day, &tip_net, &tip_detail] {
        tip.append(l);
    }
    let motion = gtk::EventControllerMotion::new();
    {
        let (area, buckets, hover, tip) = (area.clone(), buckets.clone(), hover.clone(), tip.clone());
        motion.connect_motion(move |_, px, _| {
            let wd = area.width() as f64;
            let s = slot(wd);
            let i = ((px - PAD_L) / s).floor();
            if i < 0.0 || i as usize >= buckets.len() {
                return;
            }
            let i = i as usize;
            if hover.get() == Some(i) {
                return;
            }
            hover.set(Some(i));
            let b = &buckets[i];
            tip_day.set_text(&match size {
                BucketSize::Day => b.start.format("%A, %b %-d").to_string(),
                BucketSize::Week => format!("Week of {}", b.start.format("%b %-d")),
                BucketSize::Month => b.start.format("%B %Y").to_string(),
            });
            let net = b.totals.net();
            tip_net.set_text(&format!("{} ISK net", signed_short(net)));
            tip_net.remove_css_class("comms-isk-in");
            tip_net.remove_css_class("comms-isk-out");
            if net != 0.0 {
                tip_net.add_css_class(if net > 0.0 { "comms-isk-in" } else { "comms-isk-out" });
            }
            tip_detail.set_text(&format!("Made {} · spent {}", fmt::isk_short(b.totals.income), fmt::isk_short(b.totals.spending)));
            let x = PAD_L + (i as f64 + 0.5) * s;
            let left = if x + 240.0 > wd { x - 232.0 } else { x + 12.0 };
            tip.set_margin_start(left.max(0.0) as i32);
            tip.set_margin_top(PAD_T as i32);
            tip.set_visible(true);
            area.queue_draw();
        });
    }
    {
        let (area, hover, tip) = (area.clone(), hover.clone(), tip.clone());
        motion.connect_leave(move |_| {
            hover.set(None);
            tip.set_visible(false);
            area.queue_draw();
        });
    }
    area.add_controller(motion);
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&area));
    overlay.add_overlay(&tip);
    overlay.upcast()
}
