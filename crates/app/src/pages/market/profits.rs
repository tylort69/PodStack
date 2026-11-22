//! Market → Profits: trade profit tracking in the manner of EVE Tycoon. Sales are matched first in, first out against
//! purchases across all characters (`podstack_core::features::trade::trade_report`), and shown as:
//!
//! - Overview: headline totals for a period, a daily profit chart with the running total, best and worst items;
//! - Trade profits: every matched sale;
//! - Per item: the sales totalled by type (activating one shows its sales);
//! - Warehouse: bought units not sold yet, at cost;
//! - Transactions and Journal: every character's wallet history in one table.
//!
//! The view, period, character and searches are kept in `market.profits.*` so live rebuilds restore them.

use super::{tab_bar, Tab};
use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::{character_cell, label_cell, scope_failed, set_child, type_cell, CSS as ORDERS_CSS};
use crate::widgets::{self as w, Status, Table};
use chrono::{DateTime, Duration, Local, NaiveDate, Utc};
use gtk::prelude::*;
use podstack_core::domains::wallet::{JournalEntry, Transaction};
use podstack_core::features::comms;
use podstack_core::features::trade::{self, Holding, ProfitTotals, Sale};
use podstack_core::{fmt, Character};
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const SCOPE: &str = "esi-wallet.read_character_wallet.v1";

const CSS: &str = "
.mkt-profits-bar { padding: 4px 0; }
.mkt-profits-gain, .stat-tile .stat-value.mkt-profits-gain { color: @success; }
.mkt-profits-loss, .stat-tile .stat-value.mkt-profits-loss { color: @error; }
.mkt-profits-tiles .stat-tile { min-width: 160px; }
.mkt-profits-tip {
  background-color: @surface_container_highest; color: @on_surface; border-radius: 12px; padding: 8px 12px;
  box-shadow: 0 2px 6px alpha(@shadow, 0.3);
}
.mkt-profits-legend { color: @on_surface_variant; font-size: 12px; }
.mkt-profits-swatch { min-width: 12px; min-height: 4px; border-radius: 2px; }
.mkt-profits-swatch.bars { background-color: @primary; min-height: 10px; }
.mkt-profits-swatch.line { background-color: @tertiary; }
button.mkt-profits-row { background: none; box-shadow: none; padding: 0; border-radius: 12px; min-height: 0; }
button.mkt-profits-row:hover { background-color: alpha(@on_surface, 0.08); }
.mkt-profits-rank { color: @on_surface_variant; min-width: 20px; }
.mkt-profits-items { min-width: 380px; }
";

// ---------------------------------------------------------------------------------------------------------------
// State

const KEY_VIEW: &str = "market.profits.view";
const KEY_PERIOD: &str = "market.profits.period";
const KEY_CHAR: &str = "market.profits.character";
const KEY_ITEM: &str = "market.profits.item";
const KEY_SIDE: &str = "market.profits.side";
const KEY_MARKET_ONLY: &str = "market.profits.market_only";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Overview,
    Sales,
    Items,
    Warehouse,
    Transactions,
    Journal,
}

impl View {
    const ALL: [View; 6] = [View::Overview, View::Sales, View::Items, View::Warehouse, View::Transactions, View::Journal];

    fn label(self) -> &'static str {
        match self {
            View::Overview => "Overview",
            View::Sales => "Trade profits",
            View::Items => "Per item",
            View::Warehouse => "Warehouse",
            View::Transactions => "Transactions",
            View::Journal => "Journal",
        }
    }

    fn key(self) -> &'static str {
        match self {
            View::Overview => "overview",
            View::Sales => "sales",
            View::Items => "items",
            View::Warehouse => "warehouse",
            View::Transactions => "transactions",
            View::Journal => "journal",
        }
    }

    /// Whether the period selection applies (the wallet history views show everything kept).
    fn has_period(self) -> bool {
        matches!(self, View::Overview | View::Sales | View::Items)
    }

    fn search_key(self) -> String {
        format!("market.profits.search.{}", self.key())
    }
}

const PERIODS: [(&str, Option<i64>); 4] = [("7d", Some(7)), ("30d", Some(30)), ("90d", Some(90)), ("All", None)];

/// The open view: the one last chosen, or `PODSTACK_PROFITS_VIEW` (a view label, for UI testing), or Overview.
fn current_view(ctx: &Ctx) -> View {
    let name = ctx.recall(KEY_VIEW).or_else(|| std::env::var("PODSTACK_PROFITS_VIEW").ok()).unwrap_or_default();
    let name = name.trim();
    View::ALL.into_iter().find(|v| v.label().eq_ignore_ascii_case(name) || v.key() == name).unwrap_or(View::Overview)
}

fn current_period(ctx: &Ctx) -> usize {
    ctx.recall(KEY_PERIOD).and_then(|p| PERIODS.iter().position(|(l, _)| *l == p)).unwrap_or(1)
}

/// Remembers a choice and rebuilds once the handler that made it has returned (the rebuild replaces its widget).
fn choose(ctx: &Ctx, key: &str, value: String) {
    ctx.remember(key, value);
    let ctx = ctx.clone();
    gtk::glib::idle_add_local_once(move || ctx.rebuild());
}

/// What every view gets.
struct Data {
    chars: Vec<Arc<Character>>,
    names: Rc<HashMap<i64, String>>,
    /// None: all characters.
    character: Option<i64>,
    /// Start of the period (None: everything kept).
    since: Option<DateTime<Utc>>,
    days: Option<i64>,
    report: trade::TradeReport,
    now: DateTime<Utc>,
}

impl Data {
    fn wants(&self, character: i64) -> bool {
        self.character.is_none_or(|c| c == character)
    }
    fn in_period(&self, t: DateTime<Utc>) -> bool {
        self.since.is_none_or(|s| t >= s)
    }
    /// The sales the filters select, newest first.
    fn sales(&self) -> Vec<Sale> {
        self.report.sales.iter().filter(|s| self.wants(s.character) && self.in_period(s.date)).cloned().collect()
    }
    fn chosen(&self) -> Vec<&Arc<Character>> {
        self.chars.iter().filter(|c| self.wants(c.id)).collect()
    }
    fn period_label(&self) -> String {
        match self.days {
            Some(d) => format!("Last {d} days"),
            None => "All kept history".into(),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Page

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(ORDERS_CSS);
    crate::theme::add_css(CSS);
    let view = current_view(ctx);
    let body = w::page_body();
    body.append(&tab_bar(ctx, Tab::Profits));

    let chars = ctx.core().characters();
    if chars.is_empty() {
        body.append(&w::empty_state("insights", "No characters", "Add a character to track the profits of its market trading."));
        return Page::new("Market", body).no_scroll();
    }

    let names: HashMap<i64, String> = chars.iter().map(|c| (c.id, c.name())).collect();
    let character = ctx.recall(KEY_CHAR).and_then(|s| s.parse::<i64>().ok()).filter(|id| names.contains_key(id));
    let period = current_period(ctx);
    let days = PERIODS[period].1;
    let now = Utc::now();
    let data = Rc::new(Data {
        report: trade::trade_report(&chars, &trade::settings()),
        chars,
        names: Rc::new(names),
        character,
        days,
        since: days.map(|d| period_start(now, d)),
        now,
    });

    body.append(&controls(ctx, view, &data, period));
    if let Some(b) = missing_banner(&data, view) {
        body.append(&b);
    }

    // Before anything has loaded there's nothing to compute from.
    let key = if view == View::Journal { "wallet_journal" } else { "wallet_transactions" };
    let loaded = data.chosen().iter().any(|c| if view == View::Journal { c.wallet_journal.is_some() } else { c.wallet_transactions.is_some() });
    if !loaded {
        let chosen = data.chosen();
        if chosen.iter().all(|c| scope_failed(c, key)) {
            body.append(&w::empty_state("lock", "No access to the wallet", &format!("Log in again with \"Add character\" to grant {SCOPE}.")));
        } else {
            let what = if view == View::Journal { "The wallet journal hasn't" } else { "Market transactions haven't" };
            body.append(&w::empty_state("hourglass_empty", &format!("{what} loaded yet"), "They appear after the next refresh."));
        }
        return Page::new("Market", body).no_scroll();
    }

    match view {
        View::Overview => {
            body.append(&overview(ctx, &data));
            return Page::new("Market", body);
        }
        View::Sales => body.append(&sales_view(ctx, &data)),
        View::Items => body.append(&items_view(ctx, &data)),
        View::Warehouse => body.append(&warehouse_view(ctx, &data)),
        View::Transactions => body.append(&transactions_view(ctx, &data)),
        View::Journal => body.append(&journal_view(ctx, &data)),
    }
    body.set_vexpand(true);
    Page::new("Market", body).no_scroll()
}

/// The start of the local day `days - 1` days ago, so "7d" is today and the six days before, as the chart shows.
fn period_start(now: DateTime<Utc>, days: i64) -> DateTime<Utc> {
    let day = now.with_timezone(&Local).date_naive() - Duration::days(days - 1);
    day.and_hms_opt(0, 0, 0)
        .and_then(|t| t.and_local_timezone(Local).earliest())
        .map(|t| t.with_timezone(&Utc))
        .unwrap_or(now - Duration::days(days))
}

/// The view switch, then the character filter and the period.
fn controls(ctx: &Ctx, view: View, data: &Data, period: usize) -> gtk::Box {
    let out = w::vbox(8);
    let bar = w::hbox(12);
    bar.add_css_class("mkt-profits-bar");
    // Filter chips rather than a segmented button: six views don't fit a narrow window in one row, chips wrap.
    let views = gtk::FlowBox::new();
    views.set_selection_mode(gtk::SelectionMode::None);
    views.set_column_spacing(8);
    views.set_row_spacing(8);
    views.set_max_children_per_line(View::ALL.len() as u32);
    views.set_hexpand(true);
    let mut first: Option<gtk::ToggleButton> = None;
    for v in View::ALL {
        let chip = w::chip(v.label(), v == view);
        if let Some(f) = &first {
            chip.set_group(Some(f));
        } else {
            first = Some(chip.clone());
        }
        let ctx = ctx.clone();
        chip.connect_toggled(move |c| {
            if c.is_active() {
                choose(&ctx, KEY_VIEW, v.key().into());
            }
        });
        let child = gtk::FlowBoxChild::new();
        child.set_child(Some(&chip));
        child.set_focusable(false);
        views.append(&child);
    }
    out.append(&views);

    if data.chars.len() > 1 {
        let mut ids: Vec<(i64, String)> = data.names.iter().map(|(id, n)| (*id, n.clone())).collect();
        ids.sort_by_key(|a| a.1.to_lowercase());
        let mut items = vec!["All characters".to_string()];
        items.extend(ids.iter().map(|(_, n)| n.clone()));
        let refs: Vec<&str> = items.iter().map(|s| s.as_str()).collect();
        let sel = data.character.and_then(|c| ids.iter().position(|(id, _)| *id == c)).map(|i| i + 1).unwrap_or(0);
        let dd = w::dropdown(&refs, sel as u32);
        dd.set_tooltip_text(Some("Show one character's trading"));
        dd.set_valign(gtk::Align::Center);
        let ctx = ctx.clone();
        dd.connect_selected_notify(move |d| {
            let i = d.selected() as usize;
            let v = if i == 0 { "all".to_string() } else { ids.get(i - 1).map(|(id, _)| id.to_string()).unwrap_or_default() };
            choose(&ctx, KEY_CHAR, v);
        });
        bar.append(&dd);
    }
    if view.has_period() {
        let labels: Vec<&str> = PERIODS.iter().map(|(l, _)| *l).collect();
        let ctx = ctx.clone();
        let periods = w::segmented(&labels, period, move |i| choose(&ctx, KEY_PERIOD, PERIODS[i].0.into()));
        periods.set_valign(gtk::Align::Center);
        bar.append(&periods);
    }
    if bar.first_child().is_some() {
        out.append(&bar);
    }
    out
}

/// Which characters' wallets are missing, and why.
fn missing_banner(data: &Data, view: View) -> Option<gtk::Box> {
    let journal = view == View::Journal;
    let key = if journal { "wallet_journal" } else { "wallet_transactions" };
    let missing: Vec<&Arc<Character>> =
        data.chosen().into_iter().filter(|c| if journal { c.wallet_journal.is_none() } else { c.wallet_transactions.is_none() }).collect();
    // with nothing loaded at all the page shows an empty state instead
    if missing.is_empty() || missing.len() == data.chosen().len() {
        return None;
    }
    let names: Vec<String> = missing.iter().map(|c| c.name()).collect();
    let scope = missing.iter().any(|c| scope_failed(c, key));
    let what = if journal { "wallet journal" } else { "market transactions" };
    let msg = if scope {
        format!("No {what} for {}: log in again with \"Add character\" to grant Read Character Wallet.", names.join(", "))
    } else {
        format!("The {what} for {} haven't loaded yet, so the numbers leave them out.", names.join(", "))
    };
    Some(w::banner(if scope { "lock" } else { "hourglass_empty" }, &msg, w::BannerKind::Notice))
}

// ---------------------------------------------------------------------------------------------------------------
// Cells

fn tone(widget: &impl IsA<gtk::Widget>, v: f64) {
    if v > 0.0 {
        widget.add_css_class("mkt-profits-gain");
    } else if v < 0.0 {
        widget.add_css_class("mkt-profits-loss");
    }
}

fn signed_short(v: f64) -> String {
    format!("{}{}", if v > 0.0 { "+" } else { "" }, fmt::isk_short(v))
}

/// "+1,234.50" in the gain color, "-1,234.50" in the loss color.
fn isk_cell(v: f64) -> gtk::Widget {
    let l = w::line(&format!("{}{}", if v > 0.0 { "+" } else { "" }, fmt::decimal(v, 2)), &["numeric"]);
    l.set_xalign(1.0);
    tone(&l, v);
    l.upcast()
}

fn num_label(t: &str) -> gtk::Widget {
    let l = w::line(t, &["numeric"]);
    l.set_xalign(1.0);
    l.upcast()
}

fn date_cell(t: DateTime<Utc>) -> gtk::Widget {
    let l = w::line(&fmt::datetime(t), &["dim", "numeric"]);
    l.set_tooltip_text(Some(&fmt::relative(t)));
    l.upcast()
}

fn percent(m: Option<f64>) -> String {
    m.map(|m| format!("{:.1}%", m * 100.0)).unwrap_or_else(|| "—".into())
}

fn cmp_f(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// Every sale had units of unknown cost and none known, so there's no profit to show yet.
fn cost_unknown(t: &ProfitTotals) -> bool {
    t.sales > 0 && t.unmatched == t.sales && t.cost == 0.0
}

fn unknown_cost_tip(unknown: i64, quantity: i64) -> String {
    format!(
        "{} of {} units have no recorded purchase (looted, built, from a contract, or bought before the kept wallet \
         history), so they're left out of the cost and the profit. Set a custom cost for this item in Market → Tools.",
        fmt::int(unknown),
        fmt::int(quantity)
    )
}

/// The cost, or a chip when some of the units have no known cost.
fn cost_cell(s: &Sale) -> gtk::Widget {
    if s.fully_matched() {
        return num_label(&fmt::decimal(s.cost, 2));
    }
    let b = w::hbox(8);
    b.set_halign(gtk::Align::End);
    let chip = if s.matched == 0 { w::status("Unknown cost", Status::Warn) } else { w::status("Partial", Status::Warn) };
    b.append(&chip);
    if s.matched > 0 {
        b.append(&w::line(&fmt::decimal(s.cost, 2), &["numeric"]));
    }
    b.set_tooltip_text(Some(&unknown_cost_tip(s.quantity - s.matched, s.quantity)));
    b.upcast()
}

/// A search box that keeps its text in `key` and calls `on_change` with it.
fn search_box(ctx: &Ctx, placeholder: &str, key: String, on_change: impl Fn(String) + 'static) -> gtk::SearchEntry {
    let search = w::search_entry(placeholder);
    search.set_text(&ctx.recall(&key).unwrap_or_default());
    search.set_hexpand(false);
    search.set_max_width_chars(36);
    let ctx = ctx.clone();
    search.connect_search_changed(move |s| {
        ctx.remember(&key, s.text().as_str());
        on_change(s.text().to_string());
    });
    search
}

fn matches(q: &str, fields: &[Option<&str>]) -> bool {
    q.is_empty() || fields.iter().flatten().any(|t| t.to_lowercase().contains(q))
}

fn spacer() -> gtk::Box {
    let s = w::hbox(0);
    s.set_hexpand(true);
    s
}

fn note(t: &str) -> gtk::Label {
    let l = w::text(t, &["body-small", "dim"]);
    l.set_hexpand(true);
    l.set_valign(gtk::Align::Center);
    l
}

// ---------------------------------------------------------------------------------------------------------------
// Overview

fn overview(ctx: &Ctx, data: &Data) -> gtk::Box {
    let out = w::vbox(16);
    let sales = data.sales();
    let t = trade::totals(&sales);
    let (tax, broker) = sales.iter().fold((0.0, 0.0), |(t, b), s| (t + s.tax, b + s.broker));
    let purchases: Vec<_> = data.report.purchases.iter().filter(|p| data.wants(p.character) && data.in_period(p.date)).collect();
    let spent: f64 = purchases.iter().map(|p| p.total).sum();
    let held: f64 = data.report.holdings.iter().map(|h| h.cost).sum();

    let profit = w::stat_tile("Profit", &format!("{} ISK", signed_short(t.profit)), Some(&data.period_label()));
    if let Some(v) = profit.first_child().and_then(|l| l.next_sibling()) {
        tone(&v, t.profit);
    }
    let sales_tile = w::stat_tile(
        "Sales",
        &fmt::int(t.sales as i64),
        Some(&if t.unmatched > 0 { format!("{} with unknown cost", t.unmatched) } else { "All costs known".into() }),
    );
    if t.unmatched > 0 {
        sales_tile.set_tooltip_text(Some("Units sold with no recorded purchase are left out of the profit. Set custom costs in Market → Tools."));
    }
    let tiles = w::tile_row(vec![
        profit,
        w::stat_tile("Revenue", &format!("{} ISK", fmt::isk_short(t.revenue)), None),
        w::stat_tile("Cost of goods", &format!("{} ISK", fmt::isk_short(t.cost)), None),
        w::stat_tile("Fees", &format!("{} ISK", fmt::isk_short(t.fees)), Some(&format!("{} tax · {} broker", fmt::isk_short(tax), fmt::isk_short(broker)))),
        w::stat_tile("Margin", &percent(t.margin()), Some("Profit / revenue")),
        sales_tile,
        w::stat_tile("Spent on purchases", &format!("{} ISK", fmt::isk_short(spent)), Some(&format!("{} purchases", fmt::int(purchases.len() as i64)))),
        w::stat_tile(
            "Warehouse",
            &format!("{} ISK", fmt::isk_short(held)),
            Some(&format!("{} item types at cost", fmt::int(data.report.holdings.len() as i64))),
        ),
    ]);
    tiles.add_css_class("mkt-profits-tiles");
    tiles.set_max_children_per_line(4);
    out.append(&tiles);

    // The chart covers the period; for all history, from the first sale (at least a week, at most a year).
    let days = data.days.unwrap_or_else(|| {
        let first = sales.last().map(|s| s.date.with_timezone(&Local).date_naive()).unwrap_or_else(|| data.now.with_timezone(&Local).date_naive());
        ((data.now.with_timezone(&Local).date_naive() - first).num_days() + 1).clamp(7, 365)
    });
    let daily = daily_local(&sales, days, data.now);
    let legend = w::hbox(16);
    legend.add_css_class("mkt-profits-legend");
    for (class, label) in [("bars", "Daily profit"), ("line", "Running total")] {
        let item = w::hbox(6);
        let sw = w::hbox(0);
        sw.add_css_class("mkt-profits-swatch");
        sw.add_css_class(class);
        sw.set_valign(gtk::Align::Center);
        item.append(&sw);
        item.append(&w::line(label, &[]));
        legend.append(&item);
    }
    let chart_body: gtk::Widget = if sales.is_empty() {
        w::text("No sales in this period.", &["body-medium", "dim"]).upcast()
    } else {
        profit_chart(daily)
    };
    let sub = format!("{} · local days", if data.days.is_some() { data.period_label() } else { format!("Last {days} days") });
    out.append(&w::card_with("Daily profit", Some(&sub), &[legend.upcast()], &chart_body));

    // best and worst items
    let items = trade::by_item(&sales);
    let top: Vec<_> = items.iter().take(10).collect();
    // worst first; with few items traded they can also be in the top 10
    let worst: Vec<_> = items.iter().rev().take(5).collect();
    let best = w::card_with("Most profitable", Some("Top 10 items"), &[], &item_list(ctx, &top, "No sales in this period."));
    let worst = w::card_with("Least profitable", Some("Bottom 5 items"), &[], &item_list(ctx, &worst, "No sales in this period."));
    for c in [&best, &worst] {
        c.add_css_class("mkt-profits-items");
        c.set_valign(gtk::Align::Start);
    }
    // side by side when there's room for the names, stacked otherwise
    let row = w::flow(vec![best.upcast(), worst.upcast()], 2);
    row.set_homogeneous(false);
    out.append(&row);
    out
}

/// `trade::daily_profit`, but by local day so the bars match the dates the tables show.
fn daily_local(sales: &[Sale], days: i64, now: DateTime<Utc>) -> Vec<(NaiveDate, ProfitTotals)> {
    let today = now.with_timezone(&Local).date_naive();
    let start = today - Duration::days(days - 1);
    let mut out: Vec<(NaiveDate, Vec<&Sale>)> = (0..days).map(|i| (start + Duration::days(i), vec![])).collect();
    for s in sales {
        let d = s.date.with_timezone(&Local).date_naive();
        if d >= start && d <= today {
            out[(d - start).num_days() as usize].1.push(s);
        }
    }
    out.into_iter().map(|(d, v)| (d, trade::totals(v))).collect()
}

/// Item rows that open the item's sales when clicked.
fn item_list(ctx: &Ctx, items: &[&(i64, String, ProfitTotals, i64)], empty: &str) -> gtk::Widget {
    if items.is_empty() {
        return w::text(empty, &["body-medium", "dim"]).upcast();
    }
    let list = w::vbox(0);
    for (rank, (type_id, name, t, qty)) in items.iter().map(|i| (i.0, &i.1, &i.2, i.3)).enumerate() {
        let lead = w::hbox(10);
        let r = w::line(&format!("{}", rank + 1), &["body-small", "mkt-profits-rank", "numeric"]);
        lead.append(&r);
        lead.append(&w::eve_image(&podstack_core::config::type_icon(type_id, 32), 32));
        let margin_text = percent(t.margin());
        let value = w::vbox(0);
        let p = w::line(&format!("{} ISK", signed_short(t.profit)), &["title-small", "numeric"]);
        p.set_xalign(1.0);
        tone(&p, t.profit);
        value.append(&p);
        let unknown = cost_unknown(t);
        if unknown {
            p.set_text("Cost unknown");
            p.remove_css_class("mkt-profits-gain");
            p.remove_css_class("mkt-profits-loss");
            p.add_css_class("warning-text");
            value.set_tooltip_text(Some("No recorded purchase of this item. Set a custom cost for it in Market → Tools."));
        }
        let m = w::line(if unknown { "" } else { &margin_text }, &["body-small", "dim", "numeric"]);
        m.set_xalign(1.0);
        value.append(&m);
        let supporting = format!("{} units · {} sales · {} ISK revenue", fmt::int(qty), t.sales, fmt::isk_short(t.revenue));
        let row = w::list_row(Some(lead.upcast_ref()), name, Some(&supporting), Some(value.upcast_ref()));
        let button = gtk::Button::new();
        button.add_css_class("mkt-profits-row");
        button.set_child(Some(&row));
        button.set_tooltip_text(Some("Show this item's sales"));
        let ctx = ctx.clone();
        button.connect_clicked(move |_| show_item(&ctx, type_id));
        list.append(&button);
    }
    list.upcast()
}

/// Switches to Trade profits, showing one item's sales.
fn show_item(ctx: &Ctx, type_id: i64) {
    ctx.remember(KEY_ITEM, type_id.to_string());
    ctx.remember(&View::Sales.search_key(), "");
    choose(ctx, KEY_VIEW, View::Sales.key().into());
}

// ---------- profit chart ----------

const CHART_HEIGHT: i32 = 220;
const PAD_L: f64 = 64.0;
const PAD_R: f64 = 64.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 26.0;

/// A value range that contains 0, with its share below zero.
#[derive(Clone, Copy)]
struct Range {
    lo: f64,
    hi: f64,
}

impl Range {
    fn of(values: impl Iterator<Item = f64>) -> Range {
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for v in values {
            lo = lo.min(v);
            hi = hi.max(v);
        }
        if hi - lo <= 0.0 {
            hi = 1.0;
        }
        Range { lo, hi }
    }
    fn below(&self) -> f64 {
        -self.lo / (self.hi - self.lo)
    }
    /// Stretched so its zero sits `below` of the way up, padded a little.
    fn aligned(&self, below: f64) -> Range {
        let (pos, neg) = (self.hi.max(0.0), (-self.lo).max(0.0));
        let mut span = 0.0f64;
        if below < 1.0 - 1e-9 {
            span = span.max(pos / (1.0 - below));
        }
        if below > 1e-9 {
            span = span.max(neg / below);
        }
        let span = span.max(1.0) * 1.08;
        Range { lo: -span * below, hi: span * (1.0 - below) }
    }
}

struct Geo {
    n: usize,
    w: f64,
    h: f64,
    bars: Range,
    line: Range,
}

impl Geo {
    fn plot_w(&self) -> f64 {
        (self.w - PAD_L - PAD_R).max(1.0)
    }
    fn plot_h(&self) -> f64 {
        (self.h - PAD_T - PAD_B).max(1.0)
    }
    fn slot(&self) -> f64 {
        self.plot_w() / self.n as f64
    }
    fn x(&self, i: usize) -> f64 {
        PAD_L + (i as f64 + 0.5) * self.slot()
    }
    fn y(&self, r: Range, v: f64) -> f64 {
        PAD_T + (1.0 - (v - r.lo) / (r.hi - r.lo)) * self.plot_h()
    }
    fn index_at(&self, px: f64) -> Option<usize> {
        let i = ((px - PAD_L) / self.slot()).floor();
        (i >= 0.0 && (i as usize) < self.n).then_some(i as usize)
    }
}

fn source(cr: &gtk::cairo::Context, role: &str, alpha: f64) {
    let c = crate::theme::rgba(role);
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64 * alpha);
}

/// Daily profit as bars (gains in primary, losses in error) with the running total as a line on its own scale (right
/// axis), both with zero at the same height. Hovering a day shows its numbers.
fn profit_chart(daily: Vec<(NaiveDate, ProfitTotals)>) -> gtk::Widget {
    let mut running = 0.0;
    let cumulative: Vec<f64> = daily
        .iter()
        .map(|(_, t)| {
            running += t.profit;
            running
        })
        .collect();
    let bars = Range::of(daily.iter().map(|(_, t)| t.profit));
    let line = Range::of(cumulative.iter().copied());
    // Zero sits at the same height for both. Keep room for both signs where a series has them, so a running total
    // that's all negative doesn't squash the gains of single days into nothing.
    let mut below = bars.below().max(line.below());
    if bars.hi > 0.0 || line.hi > 0.0 {
        below = below.min(0.8);
    }
    if bars.lo < 0.0 || line.lo < 0.0 {
        below = below.max(0.2);
    }
    let (bars, line) = (bars.aligned(below), line.aligned(below));
    let daily = Rc::new(daily);
    let cumulative = Rc::new(cumulative);
    let hover: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let geo = move |w: f64, h: f64, n: usize| Geo { n, w, h, bars, line };

    let area = gtk::DrawingArea::new();
    area.set_content_height(CHART_HEIGHT);
    area.set_hexpand(true);
    {
        let (daily, cumulative, hover) = (daily.clone(), cumulative.clone(), hover.clone());
        area.set_draw_func(move |_, cr, width, height| {
            let g = geo(width as f64, height as f64, daily.len());
            cr.select_font_face("Inter", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(11.0);

            // grid and labels for the daily scale (left)
            for v in comms::nice_ticks(g.bars.lo, g.bars.hi, 4) {
                let y = g.y(g.bars, v).round() + 0.5;
                source(cr, "outline_variant", if v == 0.0 { 1.0 } else { 0.5 });
                cr.set_line_width(1.0);
                cr.move_to(PAD_L, y);
                cr.line_to(PAD_L + g.plot_w(), y);
                let _ = cr.stroke();
                let label = fmt::isk_short(v);
                if let Ok(ext) = cr.text_extents(&label) {
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(PAD_L - 8.0 - ext.width() - ext.x_bearing(), y + ext.height() / 2.0);
                    let _ = cr.show_text(&label);
                }
            }
            // labels for the running total's scale (right)
            for v in comms::nice_ticks(g.line.lo, g.line.hi, 4) {
                let y = g.y(g.line, v);
                let label = fmt::isk_short(v);
                if let Ok(ext) = cr.text_extents(&label) {
                    source(cr, "tertiary", 1.0);
                    cr.move_to(PAD_L + g.plot_w() + 8.0, y + ext.height() / 2.0);
                    let _ = cr.show_text(&label);
                }
            }
            // day labels, spread out to fit
            let count = ((g.plot_w() / 90.0).floor() as usize).clamp(2, 8).min(g.n);
            for k in 0..count {
                let i = if count == 1 { 0 } else { k * (g.n - 1) / (count - 1) };
                let label = daily[i].0.format("%b %-d").to_string();
                if let Ok(ext) = cr.text_extents(&label) {
                    let x = (g.x(i) - ext.width() / 2.0).clamp(PAD_L, PAD_L + g.plot_w() - ext.width());
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(x, g.h - 8.0);
                    let _ = cr.show_text(&label);
                }
            }

            // bars
            let zero = g.y(g.bars, 0.0);
            let bw = (g.slot() * 0.7).clamp(1.0, 28.0);
            for (i, (_, t)) in daily.iter().enumerate() {
                if t.profit == 0.0 {
                    continue;
                }
                let y = g.y(g.bars, t.profit);
                let dim = hover.get().is_some_and(|h| h != i);
                source(cr, if t.profit > 0.0 { "primary" } else { "error" }, if dim { 0.45 } else { 1.0 });
                let (top, height) = if y < zero { (y, zero - y) } else { (zero, y - zero) };
                let r = (bw / 2.0).min(4.0).min(height);
                rounded_bar(cr, g.x(i) - bw / 2.0, top, bw, height.max(1.0), r, t.profit > 0.0);
                let _ = cr.fill();
            }

            // the running total
            source(cr, "tertiary", 1.0);
            cr.set_line_width(2.0);
            cr.set_line_join(gtk::cairo::LineJoin::Round);
            for (i, v) in cumulative.iter().enumerate() {
                let (x, y) = (g.x(i), g.y(g.line, *v));
                if i == 0 {
                    cr.move_to(x, y);
                } else {
                    cr.line_to(x, y);
                }
            }
            let _ = cr.stroke();

            if let Some(i) = hover.get() {
                let x = g.x(i).round() + 0.5;
                source(cr, "outline", 0.8);
                cr.set_line_width(1.0);
                cr.set_dash(&[4.0, 3.0], 0.0);
                cr.move_to(x, PAD_T);
                cr.line_to(x, PAD_T + g.plot_h());
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
                let y = g.y(g.line, cumulative[i]);
                source(cr, "surface", 1.0);
                cr.arc(g.x(i), y, 6.0, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
                source(cr, "tertiary", 1.0);
                cr.arc(g.x(i), y, 4.0, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
        });
    }

    // hover readout
    let tip = w::vbox(2);
    tip.add_css_class("mkt-profits-tip");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);
    let tip_day = w::line("", &["body-small", "dim"]);
    let tip_profit = w::line("", &["title-small", "numeric"]);
    let tip_detail = w::line("", &["body-small"]);
    let tip_total = w::line("", &["body-small", "dim"]);
    for l in [&tip_day, &tip_profit, &tip_detail, &tip_total] {
        tip.append(l);
    }

    let motion = gtk::EventControllerMotion::new();
    {
        let (area, daily, hover, tip) = (area.clone(), daily.clone(), hover.clone(), tip.clone());
        motion.connect_motion(move |_, px, _| {
            let g = geo(area.width() as f64, area.height() as f64, daily.len());
            let Some(i) = g.index_at(px) else { return };
            if hover.get() == Some(i) {
                return;
            }
            hover.set(Some(i));
            let (day, t) = &daily[i];
            tip_day.set_text(&day.format("%A, %b %-d").to_string());
            tip_profit.set_text(&format!("{} ISK", signed_short(t.profit)));
            tip_profit.remove_css_class("mkt-profits-gain");
            tip_profit.remove_css_class("mkt-profits-loss");
            tone(&tip_profit, t.profit);
            tip_detail.set_text(&if t.sales == 0 {
                "No sales".to_string()
            } else {
                format!("{} sales · {} revenue · {} margin", t.sales, fmt::isk_short(t.revenue), percent(t.margin()))
            });
            tip_total.set_text(&format!("Running total {} ISK", signed_short(cumulative[i])));
            let x = g.x(i);
            let left = if x + 280.0 > g.w { x - 272.0 } else { x + 12.0 };
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

/// A bar rounded at its outer end (the top for gains, the bottom for losses).
fn rounded_bar(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64, up: bool) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_path();
    if up {
        cr.move_to(x, y + h);
        cr.line_to(x, y + r);
        cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
        cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
        cr.line_to(x + w, y + h);
    } else {
        cr.move_to(x, y);
        cr.line_to(x + w, y);
        cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
        cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    }
    cr.close_path();
}

// ---------------------------------------------------------------------------------------------------------------
// Trade profits

fn sales_view(ctx: &Ctx, data: &Data) -> gtk::Box {
    let section = w::vbox(12);
    section.set_vexpand(true);
    let all = data.sales();
    let item = ctx.recall(KEY_ITEM).and_then(|s| s.parse::<i64>().ok()).filter(|id| all.iter().any(|s| s.type_id == *id));
    let sales = Rc::new(all);
    let names = data.names.clone();
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let summary = note("");

    let refill = {
        let (holder, sales, names, summary) = (holder.clone(), sales.clone(), names.clone(), summary.clone());
        Rc::new(move |query: String| {
            let q = query.trim().to_lowercase();
            let rows: Vec<Sale> = sales
                .iter()
                .filter(|s| item.is_none_or(|i| s.type_id == i))
                .filter(|s| matches(&q, &[Some(&s.name), Some(&s.location), names.get(&s.character).map(|n| n.as_str())]))
                .cloned()
                .collect();
            let t = trade::totals(&rows);
            summary.set_text(&format!("{} sales · {} ISK profit · {} margin", fmt::int(t.sales as i64), signed_short(t.profit), percent(t.margin())));
            let empty = if q.is_empty() && item.is_none() { "No sales in this period." } else { "Nothing matches." };
            set_child(&holder, &sales_table(rows, names.clone(), empty));
        })
    };

    let bar = w::hbox(12);
    bar.add_css_class("mkt-profits-bar");
    if let Some(id) = item {
        let name = sales.iter().find(|s| s.type_id == id).map(|s| s.name.clone()).unwrap_or_default();
        let chip = w::button_icon("close", &name, w::Btn::Tonal);
        chip.set_tooltip_text(Some("Show every item"));
        let ctx = ctx.clone();
        chip.connect_clicked(move |_| {
            ctx.remember(KEY_ITEM, "");
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        });
        bar.append(&chip);
    }
    bar.append(&summary);
    bar.append(&spacer());
    let r = refill.clone();
    let search = search_box(ctx, "Search items, characters or stations", View::Sales.search_key(), move |q| r(q));
    bar.append(&search);
    section.append(&bar);
    section.append(&holder);
    refill(search.text().to_string());
    section
}

fn sales_table(rows: Vec<Sale>, names: Rc<HashMap<i64, String>>, empty: &str) -> gtk::Widget {
    let n2 = names.clone();
    Table::new(rows)
        .widget_sorted("Date", |s| date_cell(s.date), |a, b| a.date.cmp(&b.date).then(a.transaction_id.cmp(&b.transaction_id)))
        .width(165)
        .widget_sorted("Item", |s| type_cell(s.type_id, &s.name), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .width(240)
        .widget_sorted(
            "Character",
            move |s| character_cell(s.character, names.get(&s.character).map(|n| n.as_str()).unwrap_or("")),
            move |a, b| n2.get(&a.character).cmp(&n2.get(&b.character)),
        )
        .width(170)
        .num("Qty", |s| fmt::int(s.quantity), |s| s.quantity as f64)
        .width(90)
        .num("Unit price", |s| fmt::decimal(s.unit_price, 2), |s| s.unit_price)
        .width(130)
        .num("Revenue", |s| fmt::decimal(s.revenue, 2), |s| s.revenue)
        .width(150)
        .widget_sorted("Cost", cost_cell, |a, b| cmp_f(a.cost, b.cost))
        .width(150)
        .widget_sorted(
            "Fees",
            |s| {
                let l = num_label(&fmt::decimal(s.tax + s.broker, 2));
                let broker = if s.broker > 0.0 { fmt::isk(s.broker) } else { "none (filled a buy order)".into() };
                l.set_tooltip_text(Some(&format!("Sales tax: {}\nBroker fee: {broker}", fmt::isk(s.tax))));
                l
            },
            |a, b| cmp_f(a.tax + a.broker, b.tax + b.broker),
        )
        .width(120)
        .widget_sorted("Profit", |s| isk_cell(s.profit), |a, b| cmp_f(a.profit, b.profit))
        .width(150)
        .num("Margin", |s| percent(s.margin()), |s| s.margin().unwrap_or(f64::NEG_INFINITY))
        .width(80)
        .text("Location", |s| s.location.clone())
        .width(260)
        .sort_by(0, true)
        .empty_text(empty)
        .build()
}

// ---------------------------------------------------------------------------------------------------------------
// Per item

type ItemRow = (i64, String, ProfitTotals, i64);

fn items_view(ctx: &Ctx, data: &Data) -> gtk::Box {
    let section = w::vbox(12);
    section.set_vexpand(true);
    let items = Rc::new(trade::by_item(&data.sales()));
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let refill = {
        let (ctx, holder, items) = (ctx.clone(), holder.clone(), items.clone());
        Rc::new(move |query: String| {
            let q = query.trim().to_lowercase();
            let rows: Vec<ItemRow> = items.iter().filter(|i| matches(&q, &[Some(&i.1)])).cloned().collect();
            let empty = if q.is_empty() { "No sales in this period." } else { "Nothing matches." };
            set_child(&holder, &items_table(&ctx, rows, empty));
        })
    };
    let bar = w::hbox(12);
    bar.add_css_class("mkt-profits-bar");
    bar.append(&note(&format!("{} items · double-click one to see its sales", fmt::int(items.len() as i64))));
    bar.append(&spacer());
    let r = refill.clone();
    let search = search_box(ctx, "Search items", View::Items.search_key(), move |q| r(q));
    bar.append(&search);
    section.append(&bar);
    section.append(&holder);
    refill(search.text().to_string());
    section
}

fn items_table(ctx: &Ctx, rows: Vec<ItemRow>, empty: &str) -> gtk::Widget {
    let ctx = ctx.clone();
    Table::new(rows)
        .widget_sorted("Item", |i| type_cell(i.0, &i.1), |a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
        .expand()
        .width(240)
        .num("Units sold", |i| fmt::int(i.3), |i| i.3 as f64)
        .width(100)
        .num("Sales", |i| fmt::int(i.2.sales as i64), |i| i.2.sales as f64)
        .width(70)
        .num("Revenue", |i| fmt::decimal(i.2.revenue, 2), |i| i.2.revenue)
        .width(150)
        .num("Cost", |i| fmt::decimal(i.2.cost, 2), |i| i.2.cost)
        .width(150)
        .num("Fees", |i| fmt::decimal(i.2.fees, 2), |i| i.2.fees)
        .width(120)
        .widget_sorted(
            "Profit",
            |i| {
                if !cost_unknown(&i.2) {
                    return isk_cell(i.2.profit);
                }
                let l = w::line("Unknown", &["warning-text"]);
                l.set_xalign(1.0);
                l.set_tooltip_text(Some("No recorded purchase of this item. Set a custom cost for it in Market → Tools."));
                l.upcast()
            },
            |a, b| cmp_f(a.2.profit, b.2.profit),
        )
        .width(150)
        .num(
            "Margin",
            |i| if cost_unknown(&i.2) { "—".into() } else { percent(i.2.margin()) },
            |i| if cost_unknown(&i.2) { f64::NEG_INFINITY } else { i.2.margin().unwrap_or(f64::NEG_INFINITY) },
        )
        .width(80)
        .widget("Costs", |i| {
            if i.2.unmatched == 0 {
                return label_cell("", &[]);
            }
            let chip = w::status(&format!("{} partial", i.2.unmatched), Status::Warn);
            chip.set_tooltip_text(Some(&format!(
                "{} of these sales include units with no recorded purchase, left out of the cost and profit. Set a custom \
                 cost for this item in Market → Tools.",
                i.2.unmatched
            )));
            chip.upcast()
        })
        .width(110)
        .sort_by(6, true)
        .on_activate(move |i| show_item(&ctx, i.0))
        .empty_text(empty)
        .build()
}

// ---------------------------------------------------------------------------------------------------------------
// Warehouse

fn warehouse_view(ctx: &Ctx, data: &Data) -> gtk::Box {
    let section = w::vbox(12);
    section.set_vexpand(true);
    let holdings = Rc::new(data.report.holdings.clone());
    let total: f64 = holdings.iter().map(|h| h.cost).sum();
    let units: i64 = holdings.iter().map(|h| h.quantity).sum();
    let oldest = holdings.iter().map(|h| h.oldest).min();
    // One tile: a row of three doesn't fit beside the drawer on a narrow window.
    let sub = format!(
        "{} item types · {} units · oldest bought {}",
        fmt::int(holdings.len() as i64),
        fmt::int(units),
        oldest.map(fmt::relative).unwrap_or_else(|| "—".into())
    );
    let tile = w::stat_tile("Warehouse at cost", &format!("{} ISK", fmt::isk_short(total)), Some(&sub));
    tile.set_tooltip_text(Some(&fmt::isk(total)));
    tile.set_halign(gtk::Align::Start);
    section.append(&tile);

    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let refill = {
        let (holder, holdings) = (holder.clone(), holdings.clone());
        Rc::new(move |query: String| {
            let q = query.trim().to_lowercase();
            let rows: Vec<Holding> = holdings.iter().filter(|h| matches(&q, &[Some(&h.name)])).cloned().collect();
            let empty = if q.is_empty() { "Everything bought has been sold." } else { "Nothing matches." };
            set_child(&holder, &warehouse_table(rows, empty));
        })
    };
    let bar = w::hbox(12);
    bar.add_css_class("mkt-profits-bar");
    let explain = if data.character.is_some() {
        "Bought and not sold yet, pooled across all characters (sales use up the oldest purchase of any character)"
    } else {
        "Bought and not sold yet, oldest purchases sold first"
    };
    bar.append(&note(explain));
    bar.append(&spacer());
    let r = refill.clone();
    let search = search_box(ctx, "Search items", View::Warehouse.search_key(), move |q| r(q));
    bar.append(&search);
    section.append(&bar);
    section.append(&holder);
    refill(search.text().to_string());
    section
}

fn warehouse_table(rows: Vec<Holding>, empty: &str) -> gtk::Widget {
    Table::new(rows)
        .widget_sorted("Item", |h| type_cell(h.type_id, &h.name), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .width(240)
        .num("Quantity", |h| fmt::int(h.quantity), |h| h.quantity as f64)
        .width(100)
        .num("Unit cost", |h| fmt::decimal(h.unit_cost(), 2), |h| h.unit_cost())
        .width(130)
        .num("Total cost", |h| fmt::decimal(h.cost, 2), |h| h.cost)
        .width(150)
        .widget_sorted(
            "Oldest purchase",
            |h| {
                let l = w::line(&fmt::relative(h.oldest), &["dim"]);
                l.set_tooltip_text(Some(&fmt::datetime(h.oldest)));
                l.upcast()
            },
            |a, b| a.oldest.cmp(&b.oldest),
        )
        .width(170)
        .sort_by(3, true)
        .empty_text(empty)
        .build()
}

// ---------------------------------------------------------------------------------------------------------------
// Transactions

fn transactions_view(ctx: &Ctx, data: &Data) -> gtk::Box {
    let section = w::vbox(12);
    section.set_vexpand(true);
    let mut all: Vec<(i64, Transaction)> = Vec::new();
    let mut seen = HashSet::new();
    for c in data.chosen() {
        for t in c.wallet_transactions.iter().flat_map(|d| d.transactions.iter()) {
            if seen.insert((c.id, t.transaction_id)) {
                all.push((c.id, t.clone()));
            }
        }
    }
    let rows = Rc::new(all);
    let names = data.names.clone();
    let side = Rc::new(Cell::new(match ctx.recall(KEY_SIDE).as_deref() {
        Some("buy") => 1usize,
        Some("sell") => 2,
        _ => 0,
    }));
    let query = Rc::new(RefCell::new(ctx.recall(&View::Transactions.search_key()).unwrap_or_default()));
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let refill = {
        let (holder, rows, names, side, query) = (holder.clone(), rows.clone(), names.clone(), side.clone(), query.clone());
        Rc::new(move || {
            let q = query.borrow().trim().to_lowercase();
            let side = side.get();
            let filtered: Vec<(i64, Transaction)> = rows
                .iter()
                .filter(|(_, t)| side == 0 || t.is_buy == (side == 1))
                .filter(|(cid, t)| matches(&q, &[Some(&t.name), t.client.as_deref(), Some(&t.location.name), names.get(cid).map(|n| n.as_str())]))
                .cloned()
                .collect();
            let empty = if q.is_empty() && side == 0 { "No market transactions." } else { "Nothing matches." };
            set_child(&holder, &transactions_table(filtered, names.clone(), empty));
        })
    };
    let bar = w::hbox(12);
    bar.add_css_class("mkt-profits-bar");
    let buys = rows.iter().filter(|(_, t)| t.is_buy).count();
    let labels = [format!("All ({})", rows.len()), format!("Buy ({buys})"), format!("Sell ({})", rows.len() - buys)];
    let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
    {
        let (ctx, side, refill) = (ctx.clone(), side.clone(), refill.clone());
        bar.append(&w::segmented(&refs, side.get(), move |i| {
            ctx.remember(KEY_SIDE, ["all", "buy", "sell"][i]);
            side.set(i);
            refill();
        }));
    }
    bar.append(&spacer());
    let (r, qs) = (refill.clone(), query.clone());
    bar.append(&search_box(ctx, "Search items, clients, characters or stations", View::Transactions.search_key(), move |q| {
        *qs.borrow_mut() = q;
        r();
    }));
    section.append(&bar);
    section.append(&holder);
    refill();
    section
}

fn transactions_table(rows: Vec<(i64, Transaction)>, names: Rc<HashMap<i64, String>>, empty: &str) -> gtk::Widget {
    let n2 = names.clone();
    Table::new(rows)
        .widget_sorted("Date", |(_, t)| date_cell(t.date), |a, b| a.1.date.cmp(&b.1.date).then(a.1.transaction_id.cmp(&b.1.transaction_id)))
        .width(165)
        .widget_sorted(
            "Character",
            move |(c, _)| character_cell(*c, names.get(c).map(|n| n.as_str()).unwrap_or("")),
            move |a, b| n2.get(&a.0).cmp(&n2.get(&b.0)),
        )
        .width(170)
        .widget_sorted(
            "Side",
            |(_, t)| if t.is_buy { w::status("Buy", Status::Info).upcast() } else { w::status("Sell", Status::Good).upcast() },
            |a, b| a.1.is_buy.cmp(&b.1.is_buy),
        )
        .width(80)
        .widget_sorted("Item", |(_, t)| type_cell(t.type_id, &t.name), |a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()))
        .expand()
        .width(240)
        .num("Qty", |(_, t)| fmt::int(t.quantity), |(_, t)| t.quantity as f64)
        .width(90)
        .num("Price", |(_, t)| fmt::decimal(t.unit_price, 2), |(_, t)| t.unit_price)
        .width(130)
        .widget_sorted("Total", |(_, t)| isk_cell(t.total()), |a, b| cmp_f(a.1.total(), b.1.total()))
        .width(150)
        .text("Client", |(_, t)| t.client.clone().unwrap_or_default())
        .width(170)
        .text("Location", |(_, t)| t.location.name.clone())
        .width(260)
        .sort_by(0, true)
        .empty_text(empty)
        .build()
}

// ---------------------------------------------------------------------------------------------------------------
// Journal

const MARKET_REFS: [&str; 4] = ["market_transaction", "market_escrow", "brokers_fee", "transaction_tax"];

fn journal_view(ctx: &Ctx, data: &Data) -> gtk::Box {
    let section = w::vbox(12);
    section.set_vexpand(true);
    let mut all: Vec<(i64, JournalEntry)> = Vec::new();
    let mut seen = HashSet::new();
    for c in data.chosen() {
        for e in c.wallet_journal.iter().flat_map(|d| d.entries.iter()) {
            if seen.insert((c.id, e.id)) {
                all.push((c.id, e.clone()));
            }
        }
    }
    let rows = Rc::new(all);
    let names = data.names.clone();
    let market_only = Rc::new(Cell::new(ctx.recall(KEY_MARKET_ONLY).as_deref() == Some("1")));
    let query = Rc::new(RefCell::new(ctx.recall(&View::Journal.search_key()).unwrap_or_default()));
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let refill = {
        let (holder, rows, names, market_only, query) = (holder.clone(), rows.clone(), names.clone(), market_only.clone(), query.clone());
        Rc::new(move || {
            let q = query.borrow().trim().to_lowercase();
            let market = market_only.get();
            let filtered: Vec<(i64, JournalEntry)> = rows
                .iter()
                .filter(|(_, e)| !market || MARKET_REFS.contains(&e.ref_type.as_str()))
                .filter(|(cid, e)| {
                    matches(
                        &q,
                        &[
                            Some(&comms::title_case(&e.ref_type)),
                            Some(&e.description),
                            e.reason.as_deref(),
                            e.first_party.as_deref(),
                            e.second_party.as_deref(),
                            names.get(cid).map(|n| n.as_str()),
                        ],
                    )
                })
                .cloned()
                .collect();
            let empty = if q.is_empty() && !market { "No wallet activity." } else { "Nothing matches." };
            set_child(&holder, &journal_table(filtered, names.clone(), empty));
        })
    };
    let bar = w::hbox(12);
    bar.add_css_class("mkt-profits-bar");
    let market_count = rows.iter().filter(|(_, e)| MARKET_REFS.contains(&e.ref_type.as_str())).count();
    let chip = w::chip(&format!("Market only ({market_count})"), market_only.get());
    chip.set_valign(gtk::Align::Center);
    chip.set_tooltip_text(Some("Market transactions, escrow, broker fees and sales tax"));
    {
        let (ctx, market_only, refill) = (ctx.clone(), market_only.clone(), refill.clone());
        chip.connect_toggled(move |c| {
            ctx.remember(KEY_MARKET_ONLY, if c.is_active() { "1" } else { "0" });
            market_only.set(c.is_active());
            refill();
        });
    }
    bar.append(&chip);
    bar.append(&note(&format!("{} entries", fmt::int(rows.len() as i64))));
    bar.append(&spacer());
    let (r, qs) = (refill.clone(), query.clone());
    bar.append(&search_box(ctx, "Search type, description, party or character", View::Journal.search_key(), move |q| {
        *qs.borrow_mut() = q;
        r();
    }));
    section.append(&bar);
    section.append(&holder);
    refill();
    section
}

fn journal_table(rows: Vec<(i64, JournalEntry)>, names: Rc<HashMap<i64, String>>, empty: &str) -> gtk::Widget {
    let n2 = names.clone();
    Table::new(rows)
        .widget_sorted("Date", |(_, e)| date_cell(e.date), |a, b| a.1.date.cmp(&b.1.date).then(a.1.id.cmp(&b.1.id)))
        .width(165)
        .widget_sorted(
            "Character",
            move |(c, _)| character_cell(*c, names.get(c).map(|n| n.as_str()).unwrap_or("")),
            move |a, b| n2.get(&a.0).cmp(&n2.get(&b.0)),
        )
        .width(170)
        .text("Type", |(_, e)| comms::title_case(&e.ref_type))
        .width(190)
        .widget_sorted("Amount", |(_, e)| isk_cell(e.amount), |a, b| cmp_f(a.1.amount, b.1.amount))
        .width(150)
        .num("Balance", |(_, e)| e.balance.map(|b| fmt::decimal(b, 2)).unwrap_or_default(), |(_, e)| e.balance.unwrap_or(0.0))
        .width(150)
        .widget("Description", |(_, e)| {
            let b = w::vbox(2);
            b.append(&w::line(&e.description, &[]));
            if let Some(r) = &e.reason {
                b.append(&w::line(r, &["body-small", "dim"]));
            }
            let parties: Vec<&str> = [e.first_party.as_deref(), e.second_party.as_deref()].into_iter().flatten().collect();
            let mut tip = e.description.clone();
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
        .width(280)
        .sort_by(0, true)
        .empty_text(empty)
        .build()
}
