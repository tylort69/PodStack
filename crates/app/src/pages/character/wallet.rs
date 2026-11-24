//! Character tab: the wallet. Balance and 30-day totals, a balance chart, and the journal and market
//! transactions, like EVE's own wallet window.

use crate::ctx::{Ctx, Page};
use crate::pages::market_orders::{label_cell, scope_failed, set_child, type_cell};
use crate::widgets::{self as w, Table};
use chrono::{DateTime, Local, TimeZone, Utc};
use gtk::prelude::*;
use podstack_core::domains::wallet::{JournalEntry, Transaction};
use podstack_core::features::comms::{self, BalancePoint};
use podstack_core::fmt;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub(crate) const SCOPE: &str = "esi-wallet.read_character_wallet.v1";

pub(crate) const CSS: &str = "
.comms-filter-bar { padding: 4px 0; }
.comms-isk-in { color: @success; }
.comms-isk-out { color: @error; }
.comms-tile-grid .stat-tile { min-width: 170px; }
.comms-chart-tip {
  background-color: @surface_container_highest; color: @on_surface; border-radius: 12px; padding: 8px 12px;
  box-shadow: 0 2px 6px alpha(@shadow, 0.3);
}
";

pub fn build(ctx: &Ctx, id: i64) -> Page {
    crate::theme::add_css(CSS);
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Wallet", body) };

    let journal: Rc<Vec<JournalEntry>> = Rc::new(c.wallet_journal.as_ref().map(|j| j.entries.clone()).unwrap_or_default());
    let transactions: Rc<Vec<Transaction>> = Rc::new(c.wallet_transactions.as_ref().map(|t| t.transactions.clone()).unwrap_or_default());
    let (income, expenses) = comms::journal_totals(&journal);
    let net = income + expenses;

    // headline numbers beside the chart
    let tiles = gtk::Grid::new();
    tiles.add_css_class("comms-tile-grid");
    tiles.set_row_spacing(12);
    tiles.set_column_spacing(12);
    tiles.set_valign(gtk::Align::Start);
    let balance_tile = w::stat_tile(
        "Balance",
        &c.wallet.map(|b| format!("{} ISK", fmt::isk_short(b))).unwrap_or_else(|| "—".into()),
        c.wallet.map(fmt::isk).as_deref(),
    );
    balance_tile.add_css_class("accent");
    tiles.attach(&balance_tile, 0, 0, 1, 1);
    tiles.attach(&w::stat_tile("Income · 30 days", &format!("{} ISK", fmt::isk_short(income)), None), 1, 0, 1, 1);
    tiles.attach(&w::stat_tile("Expenses · 30 days", &format!("{} ISK", fmt::isk_short(-expenses)), None), 0, 1, 1, 1);
    let net_tile = w::stat_tile("Net · 30 days", &format!("{}{} ISK", if net > 0.0 { "+" } else { "" }, fmt::isk_short(net)), None);
    if let Some(v) = net_tile.first_child().and_then(|l| l.next_sibling()) {
        v.add_css_class(if net < 0.0 { "comms-isk-out" } else { "comms-isk-in" });
    }
    tiles.attach(&net_tile, 1, 1, 1, 1);

    let top = w::hbox(16);
    top.append(&tiles);
    let chart_body: gtk::Widget = if c.wallet_journal.is_some() {
        balance_chart(journal.clone(), c.wallet)
    } else if scope_failed(&c, "wallet_journal") {
        w::scope_notice(SCOPE).upcast()
    } else {
        w::text("The journal hasn't loaded yet.", &["body-medium", "dim"]).upcast()
    };
    let chart = w::card_with("Balance", Some("Last 30 days"), &[], &chart_body);
    chart.set_hexpand(true);
    top.append(&chart);
    body.append(&top);

    // journal / transactions
    let transactions_view = ctx.recall("wallet.view").as_deref() == Some("transactions");
    let state = Rc::new(RefCell::new((transactions_view, ctx.recall("wallet.search").unwrap_or_default())));
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let refill = {
        let (holder, state, c, journal) = (holder.clone(), state.clone(), c.clone(), journal.clone());
        Rc::new(move || {
            let (tx_view, query) = state.borrow().clone();
            let q = query.trim().to_lowercase();
            let matches = |fields: &[Option<&str>]| q.is_empty() || fields.iter().flatten().any(|t| t.to_lowercase().contains(&q));
            let empty = if q.is_empty() { None } else { Some("Nothing matches.") };
            if tx_view {
                if c.wallet_transactions.is_none() {
                    set_child(&holder, &missing(&c, "wallet_transactions", "Market transactions haven't loaded yet."));
                    return;
                }
                let rows: Vec<Transaction> = transactions
                    .iter()
                    .filter(|t| matches(&[Some(&t.name), t.client.as_deref(), Some(&t.location.name)]))
                    .cloned()
                    .collect();
                set_child(&holder, &transactions_table(rows, empty.unwrap_or("No market transactions.")));
            } else {
                if c.wallet_journal.is_none() {
                    set_child(&holder, &missing(&c, "wallet_journal", "The wallet journal hasn't loaded yet."));
                    return;
                }
                let rows: Vec<JournalEntry> = journal
                    .iter()
                    .filter(|e| {
                        matches(&[
                            Some(&comms::title_case(&e.ref_type)),
                            Some(&e.description),
                            e.reason.as_deref(),
                            e.first_party.as_deref(),
                            e.second_party.as_deref(),
                        ])
                    })
                    .cloned()
                    .collect();
                set_child(&holder, &journal_table(rows, empty.unwrap_or("No wallet activity in the last 30 days.")));
            }
        })
    };

    let bar = w::hbox(12);
    bar.add_css_class("comms-filter-bar");
    let journal_label = format!("Journal ({})", journal.len());
    let tx_label = format!("Transactions ({})", c.wallet_transactions.as_ref().map(|t| t.transactions.len()).unwrap_or(0));
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        bar.append(&w::segmented(&[&journal_label, &tx_label], transactions_view as usize, move |i| {
            ctx.remember("wallet.view", if i == 1 { "transactions" } else { "journal" });
            state.borrow_mut().0 = i == 1;
            refill();
        }));
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let updated = ["wallet_journal", "wallet_transactions"].iter().filter_map(|k| c.refreshes.get(*k).and_then(|r| r.last)).max();
    if let Some(t) = updated {
        let l = w::line(&format!("Updated {}", fmt::relative(t)), &["body-small", "dim"]);
        l.set_valign(gtk::Align::Center);
        bar.append(&l);
    }
    let search = w::search_entry("Search type, description, party, item or location");
    search.set_text(&state.borrow().1);
    search.set_hexpand(false);
    search.set_max_width_chars(40);
    {
        let (ctx, state, refill) = (ctx.clone(), state.clone(), refill.clone());
        search.connect_search_changed(move |s| {
            ctx.remember("wallet.search", s.text().as_str());
            state.borrow_mut().1 = s.text().to_string();
            refill();
        });
    }
    bar.append(&search);
    body.append(&bar);
    body.append(&holder);
    refill();
    Page::new("Wallet", body).no_scroll()
}

fn missing(c: &podstack_core::Character, key: &str, waiting: &str) -> gtk::Widget {
    if scope_failed(c, key) {
        w::scope_notice(SCOPE).upcast()
    } else {
        w::empty_state("hourglass_empty", waiting, "It appears after the next refresh.").upcast()
    }
}

/// "+1,234.50" in the income color, "-1,234.50" in the expense color.
pub(crate) fn isk_cell(v: f64) -> gtk::Widget {
    let l = w::line(&format!("{}{}", if v > 0.0 { "+" } else { "" }, fmt::decimal(v, 2)), &["numeric"]);
    l.set_xalign(1.0);
    if v > 0.0 {
        l.add_css_class("comms-isk-in");
    } else if v < 0.0 {
        l.add_css_class("comms-isk-out");
    }
    l.upcast()
}

pub(crate) fn date_cell(t: DateTime<Utc>) -> gtk::Widget {
    let l = w::line(&fmt::datetime(t), &["dim", "numeric"]);
    l.set_tooltip_text(Some(&fmt::relative(t)));
    l.upcast()
}

fn journal_table(rows: Vec<JournalEntry>, empty: &str) -> gtk::Widget {
    Table::new(rows)
        .widget_sorted("Date", |e| date_cell(e.date), |a, b| a.date.cmp(&b.date).then(a.id.cmp(&b.id)))
        .text("Type", |e| comms::title_case(&e.ref_type))
        .width(200)
        .widget_sorted("Amount", |e| isk_cell(e.amount), |a, b| a.amount.partial_cmp(&b.amount).unwrap_or(std::cmp::Ordering::Equal))
        .num("Balance", |e| e.balance.map(|b| fmt::decimal(b, 2)).unwrap_or_default(), |e| e.balance.unwrap_or(0.0))
        .widget("Description", |e| {
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
        .sort_by(0, true)
        .empty_text(empty)
        .build()
}

fn transactions_table(rows: Vec<Transaction>, empty: &str) -> gtk::Widget {
    Table::new(rows)
        .widget_sorted("Date", |t| date_cell(t.date), |a, b| a.date.cmp(&b.date).then(a.transaction_id.cmp(&b.transaction_id)))
        .widget_sorted("Item", |t| type_cell(t.type_id, &t.name), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .num("Qty", |t| fmt::int(t.quantity), |t| t.quantity as f64)
        .num("Price", |t| fmt::decimal(t.unit_price, 2), |t| t.unit_price)
        .widget_sorted("Total", |t| isk_cell(t.total()), |a, b| a.total().partial_cmp(&b.total()).unwrap_or(std::cmp::Ordering::Equal))
        .widget("Client", |t| label_cell(t.client.as_deref().unwrap_or(""), &[]))
        .width(180)
        .text("Where", |t| t.location.name.clone())
        .width(280)
        .sort_by(0, true)
        .empty_text(empty)
        .build()
}

// ---------- balance chart ----------

const CHART_HEIGHT: i32 = 200;
const PAD_L: f64 = 64.0;
const PAD_R: f64 = 16.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 26.0;

/// Where points go for a given widget size.
struct Geo {
    t0: f64,
    t1: f64,
    lo: f64,
    hi: f64,
    w: f64,
    h: f64,
}

impl Geo {
    fn new(points: &[BalancePoint], w: f64, h: f64) -> Geo {
        let t0 = points[0].time.timestamp() as f64;
        let t1 = (points[points.len() - 1].time.timestamp() as f64).max(t0 + 1.0);
        let lo = points.iter().map(|p| p.balance).fold(f64::INFINITY, f64::min);
        let hi = points.iter().map(|p| p.balance).fold(f64::NEG_INFINITY, f64::max);
        let mut margin = (hi - lo) * 0.08;
        if margin <= 0.0 {
            margin = (hi.abs() * 0.05).max(1.0);
        }
        Geo { t0, t1, lo: (lo - margin).max(0.0), hi: hi + margin, w, h }
    }
    fn plot_w(&self) -> f64 {
        (self.w - PAD_L - PAD_R).max(1.0)
    }
    fn plot_h(&self) -> f64 {
        (self.h - PAD_T - PAD_B).max(1.0)
    }
    fn x(&self, t: DateTime<Utc>) -> f64 {
        PAD_L + (t.timestamp() as f64 - self.t0) / (self.t1 - self.t0) * self.plot_w()
    }
    fn y(&self, v: f64) -> f64 {
        PAD_T + (1.0 - (v - self.lo) / (self.hi - self.lo)) * self.plot_h()
    }
}

pub(crate) fn source(cr: &gtk::cairo::Context, role: &str, alpha: f64) {
    let c = crate::theme::rgba(role);
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64 * alpha);
}

fn short_date(t: f64, span_days: f64) -> String {
    let dt = Local.timestamp_opt(t as i64, 0).single().unwrap_or_else(Local::now);
    if span_days > 2.0 {
        dt.format("%b %-d").to_string()
    } else {
        dt.format("%H:%M").to_string()
    }
}

/// The wallet balance over the journal's 30 days: a stepped line (the balance only changes with an entry), and a
/// crosshair that snaps to the nearest entry and shows it.
fn balance_chart(entries: Rc<Vec<JournalEntry>>, balance: Option<f64>) -> gtk::Widget {
    let points = Rc::new(comms::balance_points(&entries, balance, Utc::now()));
    if points.len() < 2 {
        return w::text("Not enough wallet activity in the last 30 days to chart.", &["body-medium", "dim"]).upcast();
    }
    let area = gtk::DrawingArea::new();
    area.set_content_height(CHART_HEIGHT);
    area.set_hexpand(true);
    let hover: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));

    {
        let (points, hover) = (points.clone(), hover.clone());
        area.set_draw_func(move |_, cr, width, height| {
            let g = Geo::new(&points, width as f64, height as f64);
            cr.select_font_face("Inter", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(11.0);

            // horizontal grid and value ticks
            for v in comms::nice_ticks(g.lo, g.hi, 4) {
                let y = g.y(v).round() + 0.5;
                source(cr, "outline_variant", 0.6);
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
            // time ticks
            let span_days = (g.t1 - g.t0) / 86400.0;
            let count = ((g.plot_w() / 110.0).floor() as usize).clamp(2, 6);
            for i in 0..count {
                let t = g.t0 + (g.t1 - g.t0) * i as f64 / (count - 1) as f64;
                let label = short_date(t, span_days);
                if let Ok(ext) = cr.text_extents(&label) {
                    let x = PAD_L + (t - g.t0) / (g.t1 - g.t0) * g.plot_w();
                    let x = if i == 0 {
                        x
                    } else if i == count - 1 {
                        x - ext.width()
                    } else {
                        x - ext.width() / 2.0
                    };
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(x, g.h - 8.0);
                    let _ = cr.show_text(&label);
                }
            }

            // the stepped line, and the area under it
            let path = |cr: &gtk::cairo::Context| {
                cr.move_to(g.x(points[0].time), g.y(points[0].balance));
                for p in points.iter().skip(1) {
                    let x = g.x(p.time);
                    cr.line_to(x, cr.current_point().map(|(_, y)| y).unwrap_or(0.0));
                    cr.line_to(x, g.y(p.balance));
                }
            };
            path(cr);
            cr.line_to(g.x(points[points.len() - 1].time), PAD_T + g.plot_h());
            cr.line_to(g.x(points[0].time), PAD_T + g.plot_h());
            cr.close_path();
            let grad = gtk::cairo::LinearGradient::new(0.0, PAD_T, 0.0, PAD_T + g.plot_h());
            let p = crate::theme::rgba("primary");
            grad.add_color_stop_rgba(0.0, p.red() as f64, p.green() as f64, p.blue() as f64, 0.30);
            grad.add_color_stop_rgba(1.0, p.red() as f64, p.green() as f64, p.blue() as f64, 0.02);
            let _ = cr.set_source(&grad);
            let _ = cr.fill();
            path(cr);
            source(cr, "primary", 1.0);
            cr.set_line_width(2.0);
            cr.set_line_join(gtk::cairo::LineJoin::Round);
            let _ = cr.stroke();

            if let Some(i) = hover.get() {
                let p = &points[i];
                let (x, y) = (g.x(p.time), g.y(p.balance));
                source(cr, "outline", 0.8);
                cr.set_line_width(1.0);
                cr.set_dash(&[4.0, 3.0], 0.0);
                cr.move_to(x.round() + 0.5, PAD_T);
                cr.line_to(x.round() + 0.5, PAD_T + g.plot_h());
                let _ = cr.stroke();
                cr.set_dash(&[], 0.0);
                source(cr, "surface", 1.0);
                cr.arc(x, y, 6.5, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
                source(cr, "primary", 1.0);
                cr.arc(x, y, 4.5, 0.0, std::f64::consts::TAU);
                let _ = cr.fill();
            }
        });
    }

    // hover readout
    let tip = w::vbox(2);
    tip.add_css_class("comms-chart-tip");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);
    let tip_when = w::line("", &["body-small", "dim"]);
    let tip_value = w::line("", &["title-small", "numeric"]);
    let tip_what = w::line("", &["body-small"]);
    tip.append(&tip_when);
    tip.append(&tip_value);
    tip.append(&tip_what);

    let motion = gtk::EventControllerMotion::new();
    {
        let (area, points, hover, tip) = (area.clone(), points.clone(), hover.clone(), tip.clone());
        motion.connect_motion(move |_, px, _| {
            let g = Geo::new(&points, area.width() as f64, area.height() as f64);
            let nearest = (0..points.len())
                .min_by(|a, b| (g.x(points[*a].time) - px).abs().total_cmp(&(g.x(points[*b].time) - px).abs()))
                .unwrap_or(0);
            if hover.get() != Some(nearest) {
                hover.set(Some(nearest));
                let p = &points[nearest];
                tip_when.set_text(&p.time.with_timezone(&Local).format("%b %-d, %H:%M").to_string());
                tip_value.set_text(&fmt::isk(p.balance));
                tip_what.remove_css_class("comms-isk-in");
                tip_what.remove_css_class("comms-isk-out");
                match p.entry.map(|i| &entries[i]) {
                    Some(e) => {
                        tip_what.set_text(&format!("{}{} · {}", if e.amount >= 0.0 { "+" } else { "" }, fmt::isk_short(e.amount), comms::title_case(&e.ref_type)));
                        tip_what.add_css_class(if e.amount >= 0.0 { "comms-isk-in" } else { "comms-isk-out" });
                    }
                    None => tip_what.set_text("Now"),
                }
                let x = g.x(p.time);
                let left = if x + 240.0 > g.w { x - 232.0 } else { x + 12.0 };
                tip.set_margin_start(left.max(PAD_L) as i32);
                tip.set_margin_top(PAD_T as i32);
                tip.set_visible(true);
                area.queue_draw();
            }
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
