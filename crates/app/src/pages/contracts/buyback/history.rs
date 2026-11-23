//! Buyback → History: what the user's characters and corporations bought as a buyback and sold to buybacks over a
//! period, from the finished contracts.

use super::{colored, pct, section, signed_class};
use crate::ctx::Ctx;
use crate::pages::contracts::common::open_contract;
use crate::pages::market_orders::type_cell;
use crate::widgets::{self as w, Table};
use chrono::{Datelike, NaiveDate, Utc};
use gtk::prelude::*;
use podstack_core::features::buyback::{self as bb, Receiver, Summary};
use podstack_core::features::contracts::{self as ct, Flow, Mine, Party, Period};
use podstack_core::fmt;
use podstack_core::sde::MARKET;
use std::collections::BTreeMap;
use std::rc::Rc;

const PERIOD_KEY: &str = "contracts.buyback.history.period";
const FILTER_KEY: &str = "contracts.buyback.history.program";

pub(super) fn view(ctx: &Ctx) -> gtk::Box {
    let col = w::vbox(16);
    let doc = bb::load_doc();
    let chars = ctx.core().characters();
    let mine = Mine::of(&chars);
    let archive = ct::archive_snapshot();
    let settings = ct::settings();
    let now = Utc::now();
    let period = Period::from_key(&ctx.recall(PERIOD_KEY).unwrap_or_else(|| Period::Month.key().into()));

    // programs with a receiver can narrow the history to their buyback
    let programs: Vec<_> = doc.programs.iter().filter(|p| p.receiver.is_set()).cloned().collect();
    let filter_id = ctx.recall(FILTER_KEY).and_then(|v| v.parse::<u64>().ok());
    let chosen = programs.iter().position(|p| Some(p.id) == filter_id);
    let receiver = chosen.map(|i| programs[i].receiver.clone()).unwrap_or(Receiver::Unset);

    let bar = w::hbox(12);
    {
        let labels: Vec<&str> = Period::ALL.iter().map(|p| p.label()).collect();
        let ctx = ctx.clone();
        bar.append(&w::segmented(&labels, Period::ALL.iter().position(|p| *p == period).unwrap_or(1), move |i| {
            ctx.remember(PERIOD_KEY, Period::ALL[i].key());
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        }));
    }
    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let mut names = vec!["All item exchanges".to_string()];
    names.extend(programs.iter().map(|p| format!("{} ({})", p.name, p.receiver_label())));
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    let dd = w::dropdown(&refs, chosen.map(|i| i as u32 + 1).unwrap_or(0));
    {
        let (ctx, ids) = (ctx.clone(), programs.iter().map(|p| p.id).collect::<Vec<_>>());
        dd.connect_selected_notify(move |dd| {
            let v = match dd.selected() as usize {
                0 => "all".to_string(),
                i => ids.get(i - 1).map(|id| id.to_string()).unwrap_or_else(|| "all".into()),
            };
            ctx.remember(FILTER_KEY, v);
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        });
    }
    bar.append(&dd);
    col.append(&bar);

    let flows = ct::all_flows(&chars, &archive, settings.value_at_sell);
    let a = bb::activity(&flows, &archive, &receiver, period, now, settings.count_internal);
    if a.bought.is_empty() && a.sold.is_empty() {
        let since = archive.since().map(|s| format!(" PodStack has kept contracts since {}.", fmt::date(s))).unwrap_or_default();
        col.append(&w::empty_state(
            "history",
            "No buyback trades",
            &format!(
                "No finished item exchanges {} where your side got items for ISK (buying, as a buyback) or gave items for ISK (selling to one).{since}",
                if period == Period::All { "yet".to_string() } else { format!("in the last {}", period.label()) }
            ),
        ));
        return col;
    }

    let bought = Summary::of(&a.bought, true);
    let sold = Summary::of(&a.sold, false);
    let hub = crate::pages::contracts::common::hub_name(&settings);
    let basis = if settings.value_at_sell { "sell" } else { "buy" };
    let margin_tile = w::stat_tile(
        "Buyback margin",
        &format!("{} ISK", fmt::isk_short(bought.margin(true))),
        Some(&match bought.rate() {
            Some(r) => format!("Paid {} of {hub} {basis} value", pct(r)),
            None => "Not valued yet".into(),
        }),
    );
    if bought.margin(true) > 0.0 {
        margin_tile.add_css_class("accent");
    }
    if bought.count > 0 {
        col.append(&section("Buying", Some("Item exchanges where your side paid ISK for items: the buyback you run.")));
        col.append(&w::tile_row(vec![
            w::stat_tile("Contracts bought", &fmt::int(bought.count as i64), Some(&format!("{} m³", fmt::decimal(bought.volume, 0)))),
            w::stat_tile("ISK paid", &format!("{} ISK", fmt::isk_short(bought.isk)), None),
            w::stat_tile("Items' value", &format!("{} ISK", fmt::isk_short(bought.value)), Some(&format!("At {hub} {basis} when finished"))),
            margin_tile,
        ]));
    }
    if sold.count > 0 {
        col.append(&section("Selling", Some("Item exchanges where your side gave items for ISK: selling to buybacks (and anyone else who buys by contract).")));
        col.append(&w::tile_row(vec![
            w::stat_tile("Contracts sold", &fmt::int(sold.count as i64), Some(&format!("{} m³", fmt::decimal(sold.volume, 0)))),
            w::stat_tile("ISK received", &format!("{} ISK", fmt::isk_short(sold.isk)), None),
            w::stat_tile("Items' value", &format!("{} ISK", fmt::isk_short(sold.value)), Some(&format!("At {hub} {basis} when finished"))),
            w::stat_tile(
                "Rate received",
                &sold.rate().map(pct).unwrap_or_else(|| "—".into()),
                Some(&format!("{} ISK vs the hub", fmt::isk_short(sold.margin(false)))),
            ),
        ]));
    }
    let unvalued = bought.unvalued + sold.unvalued;
    if unvalued > 0 {
        col.append(&w::banner(
            "hourglass_empty",
            &format!("{unvalued} trades aren't valued yet (their items haven't loaded): margins leave them out."),
            w::BannerKind::Info,
        ));
    }

    // ISK per day or month
    let start = period.start(now).map(|s| s.date_naive()).or_else(|| a.bought.iter().chain(a.sold.iter()).map(|f| f.date.date_naive()).min()).unwrap_or(now.date_naive());
    let by_month = period.by_month();
    let tb = ct::timeline(&a.bought, start, now.date_naive(), by_month);
    let ts = ct::timeline(&a.sold, start, now.date_naive(), by_month);
    let buckets: Vec<(NaiveDate, f64, f64)> = tb.iter().zip(ts.iter()).map(|(b, s)| (b.0, b.1.isk_out - b.1.isk_in, s.1.isk_in - s.1.isk_out)).collect();
    let legend = w::hbox(16);
    for (class, label) in [("bb-legend-paid", "ISK paid (buying)"), ("bb-legend-received", "ISK received (selling)")] {
        let item = w::hbox(6);
        let sw = w::hbox(0);
        sw.add_css_class("bb-legend-swatch");
        sw.add_css_class(class);
        sw.set_valign(gtk::Align::Center);
        item.append(&sw);
        item.append(&w::line(label, &["body-small", "dim"]));
        legend.append(&item);
    }
    let chart_box = w::vbox(8);
    chart_box.append(&volume_chart(buckets, by_month));
    chart_box.append(&legend);
    col.append(&w::card_with(if by_month { "ISK per month" } else { "ISK per day" }, None, &[], &chart_box));

    // who with
    let grid = pair();
    if !a.bought.is_empty() {
        grid.append(&w::card_with("Sellers", Some("Who sold to your buyback"), &[], &counterparty_table(&a.bought, true)));
    }
    if !a.sold.is_empty() {
        grid.append(&w::card_with("Buyers", Some("The buybacks you sold to"), &[], &counterparty_table(&a.sold, false)));
    }
    col.append(&grid);
    col.append(&w::card_with("By character and corporation", None, &[], &party_table(&mine, &a.bought, &a.sold)));
    let items = pair();
    if !a.bought.is_empty() {
        items.append(&w::card_with("Top items bought", None, &[], &items_table(&a.bought, true)));
    }
    if !a.sold.is_empty() {
        items.append(&w::card_with("Top items sold", None, &[], &items_table(&a.sold, false)));
    }
    col.append(&items);
    col.append(&w::card_with("Trades", Some("Open one for the contract's details"), &[], &trades_table(ctx, &mine, &a.bought, &a.sold)));
    col
}

/// Two cards side by side, sharing the width (their tables scroll sideways when narrow).
fn pair() -> gtk::Box {
    let b = w::hbox(16);
    b.set_homogeneous(true);
    b
}

/// Summaries grouped by name, most ISK first.
fn grouped<'a>(flows: impl IntoIterator<Item = &'a Flow>, buying: bool, key: impl Fn(&Flow) -> String) -> Vec<(String, Summary)> {
    let mut m: BTreeMap<String, Vec<&Flow>> = BTreeMap::new();
    for f in flows {
        m.entry(key(f)).or_default().push(f);
    }
    let mut v: Vec<(String, Summary)> = m.into_iter().map(|(k, fs)| (k, Summary::of(fs, buying))).collect();
    v.sort_by(|a, b| b.1.isk.total_cmp(&a.1.isk));
    v
}

fn counterparty_table(flows: &[Flow], buying: bool) -> gtk::Widget {
    let rows = grouped(flows, buying, |f| f.counterparty.as_ref().map(|c| c.1.clone()).unwrap_or_else(|| "Unknown".into()));
    Table::new(rows)
        .text(if buying { "Seller" } else { "Buyer" }, |r: &(String, Summary)| r.0.clone())
        .expand()
        .num("Contracts", |r| fmt::int(r.1.count as i64), |r| r.1.count as f64)
        .num(if buying { "Paid" } else { "Received" }, |r| fmt::isk_short(r.1.isk), |r| r.1.isk)
        .num("Value", |r| fmt::isk_short(r.1.value), |r| r.1.value)
        .num("Rate", |r| r.1.rate().map(pct).unwrap_or_else(|| "—".into()), |r| r.1.rate().unwrap_or(0.0))
        .widget_sorted(
            if buying { "Margin" } else { "vs hub" },
            move |r| {
                let m = r.1.margin(buying);
                colored(&fmt::isk_short(m), signed_class(m))
            },
            move |a, b| a.1.margin(buying).total_cmp(&b.1.margin(buying)),
        )
        .sort_by(2, true)
        .max_height(320)
        .build()
}

struct PartyRow {
    name: String,
    bought: Summary,
    sold: Summary,
}

fn party_table(mine: &Mine, bought: &[Flow], sold: &[Flow]) -> gtk::Widget {
    let mut m: BTreeMap<Party, (Vec<&Flow>, Vec<&Flow>)> = BTreeMap::new();
    for f in bought {
        m.entry(f.party).or_default().0.push(f);
    }
    for f in sold {
        m.entry(f.party).or_default().1.push(f);
    }
    let rows: Vec<PartyRow> = m
        .into_iter()
        .map(|(p, (b, s))| PartyRow {
            name: match p {
                Party::Corporation(_) => format!("{} (corporation)", mine.name(p)),
                Party::Character(_) => mine.name(p),
            },
            bought: Summary::of(b, true),
            sold: Summary::of(s, false),
        })
        .collect();
    Table::new(rows)
        .text("Character or corporation", |r: &PartyRow| r.name.clone())
        .expand()
        .num("Bought", |r| fmt::int(r.bought.count as i64), |r| r.bought.count as f64)
        .num("Paid", |r| fmt::isk_short(r.bought.isk), |r| r.bought.isk)
        .widget_sorted(
            "Margin",
            |r| colored(&fmt::isk_short(r.bought.margin(true)), signed_class(r.bought.margin(true))),
            |a, b| a.bought.margin(true).total_cmp(&b.bought.margin(true)),
        )
        .num("Sold", |r| fmt::int(r.sold.count as i64), |r| r.sold.count as f64)
        .num("Received", |r| fmt::isk_short(r.sold.isk), |r| r.sold.isk)
        .num("Rate received", |r| r.sold.rate().map(pct).unwrap_or_else(|| "—".into()), |r| r.sold.rate().unwrap_or(0.0))
        .sort_by(2, true)
        .natural_height()
        .build()
}

fn items_table(flows: &[Flow], buying: bool) -> gtk::Widget {
    let rows = ct::by_item(flows, buying);
    let name = |t: i64| MARKET.get(t).map(|t| t.name.clone()).unwrap_or_else(|| format!("Type #{t}"));
    Table::new(rows)
        .widget_sorted("Item", move |r: &ct::ItemTotals| type_cell(r.type_id, &name(r.type_id)), |a, b| a.type_id.cmp(&b.type_id))
        .expand()
        .num("Quantity", |r| fmt::int(r.quantity), |r| r.quantity as f64)
        .num(if buying { "Paid" } else { "Received" }, |r| fmt::isk_short(r.isk), |r| r.isk)
        .num("Per unit", |r| if r.quantity > 0 { fmt::isk_short(r.isk / r.quantity as f64) } else { "—".into() }, |r| if r.quantity > 0 { r.isk / r.quantity as f64 } else { 0.0 })
        .num("Contracts", |r| fmt::int(r.contracts as i64), |r| r.contracts as f64)
        .sort_by(2, true)
        .max_height(320)
        .build()
}

#[derive(Clone)]
struct Trade {
    f: Flow,
    buying: bool,
    party: String,
}

impl Trade {
    fn isk(&self) -> f64 {
        if self.buying {
            self.f.isk_out - self.f.isk_in
        } else {
            self.f.isk_in - self.f.isk_out
        }
    }
    fn value(&self) -> Option<f64> {
        if self.buying {
            self.f.value_in
        } else {
            self.f.value_out
        }
    }
    /// For the buyback: value over paid; for a seller: received over value.
    fn margin(&self) -> Option<f64> {
        self.value().map(|v| if self.buying { v - self.isk() } else { self.isk() - v })
    }
}

fn trades_table(ctx: &Ctx, mine: &Mine, bought: &[Flow], sold: &[Flow]) -> gtk::Widget {
    let mut rows: Vec<Trade> = bought
        .iter()
        .map(|f| (f, true))
        .chain(sold.iter().map(|f| (f, false)))
        .map(|(f, buying)| Trade { f: f.clone(), buying, party: mine.name(f.party) })
        .collect();
    rows.sort_by_key(|t| std::cmp::Reverse(t.f.date));
    let open = {
        let ctx = ctx.clone();
        Rc::new(move |t: &Trade| {
            if let Some(e) = ct::archive_snapshot().get(t.f.contract_id) {
                open_contract(&ctx, &e.contract);
            }
        })
    };
    let open2 = open.clone();
    Table::new(rows)
        .widget_sorted(
            "Finished",
            |t: &Trade| {
                let l = w::line(&fmt::date(t.f.date), &["dim"]);
                l.set_tooltip_text(Some(&fmt::datetime(t.f.date)));
                l.upcast()
            },
            |a, b| a.f.date.cmp(&b.f.date),
        )
        .widget_sorted(
            "Side",
            |t| w::status(if t.buying { "Bought" } else { "Sold" }, if t.buying { w::Status::Info } else { w::Status::Neutral }).upcast(),
            |a, b| a.buying.cmp(&b.buying),
        )
        .text("Yours", |t| t.party.clone())
        .text("With", |t| t.f.counterparty.as_ref().map(|c| c.1.clone()).unwrap_or_default())
        .expand()
        .num("Lines", |t| fmt::int(if t.buying { t.f.items_in.len() } else { t.f.items_out.len() } as i64), |t| t.f.items_in.len().max(t.f.items_out.len()) as f64)
        .num("ISK", |t| fmt::isk_short(t.isk()), |t| t.isk())
        .num("Value", |t| t.value().map(fmt::isk_short).unwrap_or_else(|| "—".into()), |t| t.value().unwrap_or(0.0))
        .widget_sorted(
            "Margin",
            |t| match t.margin() {
                Some(m) => colored(&fmt::isk_short(m), signed_class(m)),
                None => colored("—", "dim"),
            },
            |a, b| a.margin().unwrap_or(0.0).total_cmp(&b.margin().unwrap_or(0.0)),
        )
        .widget("", move |t| {
            let b = w::icon_button("chevron_right", "Contract details");
            let (open, t) = (open2.clone(), t.clone());
            b.connect_clicked(move |_| open(&t));
            b.upcast()
        })
        .on_activate(move |t| open(t))
        .sort_by(0, true)
        .max_height(420)
        .build()
}

// ---------- chart ----------

const CHART_HEIGHT: i32 = 180;
const PAD_L: f64 = 56.0;
const PAD_R: f64 = 8.0;
const PAD_T: f64 = 10.0;
const PAD_B: f64 = 24.0;

fn source(cr: &gtk::cairo::Context, role: &str, alpha: f64) {
    let c = crate::theme::rgba(role);
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64 * alpha);
}

fn bucket_label(d: NaiveDate, by_month: bool) -> String {
    if by_month {
        d.format("%b %Y").to_string()
    } else {
        d.format("%b %-d").to_string()
    }
}

/// Paired bars per bucket: ISK paid (buying) and received (selling); hovering a bucket shows its numbers.
fn volume_chart(buckets: Vec<(NaiveDate, f64, f64)>, by_month: bool) -> gtk::Widget {
    let buckets = Rc::new(buckets);
    let area = gtk::DrawingArea::new();
    area.set_content_height(CHART_HEIGHT);
    area.set_hexpand(true);
    let hi = buckets.iter().map(|b| b.1.max(b.2)).fold(0.0, f64::max).max(1.0);
    let ticks = podstack_core::features::comms::nice_ticks(0.0, hi, 4);
    let top = ticks.last().copied().unwrap_or(hi).max(hi);
    {
        let buckets = buckets.clone();
        area.set_draw_func(move |_, cr, width, height| {
            let (w, h) = (width as f64, height as f64);
            let plot_w = (w - PAD_L - PAD_R).max(1.0);
            let plot_h = (h - PAD_T - PAD_B).max(1.0);
            let y = |v: f64| PAD_T + (1.0 - v / top) * plot_h;
            cr.select_font_face("Inter", gtk::cairo::FontSlant::Normal, gtk::cairo::FontWeight::Normal);
            cr.set_font_size(11.0);
            for v in &ticks {
                let yy = y(*v).round() + 0.5;
                source(cr, "outline_variant", 0.6);
                cr.set_line_width(1.0);
                cr.move_to(PAD_L, yy);
                cr.line_to(PAD_L + plot_w, yy);
                let _ = cr.stroke();
                let label = fmt::isk_short(*v);
                if let Ok(ext) = cr.text_extents(&label) {
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(PAD_L - 8.0 - ext.width() - ext.x_bearing(), yy + ext.height() / 2.0);
                    let _ = cr.show_text(&label);
                }
            }
            let n = buckets.len().max(1) as f64;
            let slot = plot_w / n;
            let bar = ((slot - 4.0) / 2.0).clamp(1.0, 18.0);
            for (i, (_, paid, received)) in buckets.iter().enumerate() {
                let cx = PAD_L + slot * (i as f64 + 0.5);
                for (v, role, x) in [(*paid, "primary", cx - bar), (*received, "tertiary", cx)] {
                    if v > 0.0 {
                        source(cr, role, 1.0);
                        cr.rectangle(x, y(v), bar, (PAD_T + plot_h - y(v)).max(1.0));
                        let _ = cr.fill();
                    }
                }
            }
            // a few date labels, spread out
            let count = ((plot_w / 90.0).floor() as usize).clamp(1, buckets.len().max(1));
            let step = (buckets.len() as f64 / count as f64).ceil().max(1.0) as usize;
            for (i, (d, _, _)) in buckets.iter().enumerate().step_by(step) {
                let label = bucket_label(*d, by_month);
                if let Ok(ext) = cr.text_extents(&label) {
                    let cx = PAD_L + slot * (i as f64 + 0.5);
                    let x = (cx - ext.width() / 2.0).clamp(PAD_L, (w - PAD_R - ext.width()).max(PAD_L));
                    source(cr, "on_surface_variant", 1.0);
                    cr.move_to(x, h - 6.0);
                    let _ = cr.show_text(&label);
                }
            }
        });
    }
    area.set_has_tooltip(true);
    area.connect_query_tooltip(move |a, x, _, _, tip| {
        let plot_w = (a.width() as f64 - PAD_L - PAD_R).max(1.0);
        let i = ((x as f64 - PAD_L) / plot_w * buckets.len() as f64).floor();
        if i < 0.0 || i as usize >= buckets.len() {
            return false;
        }
        let (d, paid, received) = buckets[i as usize];
        let when = if by_month { d.format("%B %Y").to_string() } else { format!("{} {}", d.format("%a %b %-d"), d.year()) };
        tip.set_text(Some(&format!("{when}\nPaid {} ISK\nReceived {} ISK", fmt::isk_short(paid), fmt::isk_short(received))));
        true
    });
    area.upcast()
}
