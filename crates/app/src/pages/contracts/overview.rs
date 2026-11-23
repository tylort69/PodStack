//! The Overview tab: what the characters spent and made on contracts over a period, for everyone or one character
//! or corporation: headline tiles, ISK in and out over time, and breakdowns by kind, by character, by availability
//! (alliance and corporation contracts), by item category, by counterparty, by item and by tag. Every breakdown row
//! opens the Contracts tab filtered to it. Contracts waiting on the user are listed on top.

use super::browse::{self, scope_options, signed, Link};
use super::common::open_contract;
use super::Tab;
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::labelled;
use crate::pages::market_orders::{character_cell, scope_failed, type_cell};
use crate::widgets::{self as w, Table};
use chrono::{DateTime, NaiveDate, Utc};
use gtk::prelude::*;
use podstack_core::domains::contracts::{Contract, CORP_SCOPE};
use podstack_core::features::comms;
use podstack_core::features::contracts::{self as ct, Category, Flow, Mine, Party, Period, Scope, Totals};
use podstack_core::{config, fmt};
use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

const PERIOD_KEY: &str = "contracts.overview.period";
const SCOPE_KEY: &str = "contracts.overview.scope";
/// Rows in the top counterparties and items tables.
const TOP: usize = 15;

const CSS: &str = "
.ctr-chart-tip {
  background-color: @surface_container_highest; color: @on_surface; border-radius: 12px; padding: 8px 12px;
  box-shadow: 0 2px 6px alpha(@shadow, 0.3);
}
.ctr-dot { min-width: 10px; min-height: 10px; border-radius: 5px; }
.ctr-dot-in { background-color: @primary; }
.ctr-dot-out { background-color: @tertiary; }
";

/// Contracts assigned to the user waiting to be accepted, plus couriers they're hauling due within a day.
pub fn nav_badge(core: &podstack_core::Core) -> Option<u32> {
    Some(ct::attention_of(&core.characters(), Utc::now()).count() as u32)
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    body.append(&super::tab_bar(ctx, Tab::Overview));
    let chars = ctx.core().characters();
    if chars.is_empty() {
        body.append(&w::empty_state("assignment", "No characters", "Add a character to see what it spends and makes on contracts."));
        return Page::new("Contracts", body);
    }
    let now = Utc::now();
    let archive = ct::archive_snapshot();
    let doc = ct::load_doc();
    let s = doc.settings.clone();
    let mine = Mine::of(&chars);
    let options = scope_options(&mine);
    let scope = Scope::from_key(&ctx.recall(SCOPE_KEY).unwrap_or_default());
    let scope = if options.iter().any(|o| o.1 == scope) { scope } else { Scope::All };
    let period = Period::from_key(&ctx.recall(PERIOD_KEY).unwrap_or_else(|| Period::Month.key().into()));

    scope_banners(&body, &chars);
    body.append(&controls(ctx, &options, scope, period));

    // history
    let since = archive.since();
    let history = match since {
        Some(t) => format!(
            "History is complete since {}: ESI only lists the last 30 days, so PodStack keeps every contract it sees from its first refresh on.",
            fmt::date(t)
        ),
        None => "No contracts seen yet. ESI only lists the last 30 days; from the first refresh on, PodStack keeps every contract it sees.".into(),
    };
    let history = w::text(&history, &["body-medium", "dim"]);
    body.append(&history);

    let all = ct::all_flows(&chars, &archive, s.value_at_sell);
    let in_period: Vec<&Flow> = all.iter().filter(|f| period.contains(f.date, now)).collect();
    let flows: Vec<&Flow> = in_period.iter().copied().filter(|f| scope.includes(f, s.count_internal)).collect();
    let totals = Totals::of(flows.iter().copied());

    // what's open now, for the waiting list and the ISK tied up
    let known = ct::all_known(&chars, &archive);
    let open: Vec<&Contract> = known.iter().map(|e| &e.contract).filter(|c| ct::is_open_now(c, now)).collect();
    let scoped_mine = match scope {
        Scope::All => mine.clone(),
        Scope::Of(p) => mine.only(p),
    };
    body.append(&tiles(&flows, &totals, &ct::exposure(open.iter().copied(), &scoped_mine), period));
    if scope == Scope::All && !s.count_internal {
        let internal = in_period.iter().filter(|f| f.internal && f.side == ct::Side::Issuer).count();
        if internal > 0 {
            body.append(&w::text(
                &format!(
                    "{internal} contract{} between your own characters {} left out of these totals (each character's own figures keep them). Change it with the gear button.",
                    if internal == 1 { "" } else { "s" },
                    if internal == 1 { "is" } else { "are" }
                ),
                &["body-small", "dim"],
            ));
        }
    }

    let waiting = waiting_card(ctx, &chars, &scoped_mine, now);
    if let Some(card) = waiting {
        body.append(&card);
    }

    // chart
    let start = period.start(now).map(|t| t.date_naive()).or_else(|| flows.first().map(|f| f.date.date_naive())).or(since.map(|t| t.date_naive()));
    let chart_body: gtk::Widget = match start {
        Some(start) if totals.count > 0 => {
            // a long "All time" history reads better by month even when it's within a quarter
            let by_month = period.by_month() || (period == Period::All && (now.date_naive() - start).num_days() > 92);
            chart(ct::timeline(flows.iter().copied(), start, now.date_naive(), by_month), by_month)
        }
        _ => w::text("No finished contracts in this period.", &["body-medium", "dim"]).upcast(),
    };
    body.append(&w::card_with("ISK in and out", Some(&chart_subtitle(period)), &[legend()], &chart_body));

    let link = move |mut l: Link| {
        l.scope = scope;
        l.period = (period != Period::All).then_some(period);
        l
    };
    let link: LinkFn = Rc::new(link);

    if scope == Scope::All {
        body.append(&parties_card(ctx, &chars, &mine, &in_period, period, link.clone()));
    }
    let pairs = gtk::FlowBox::new();
    pairs.set_selection_mode(gtk::SelectionMode::None);
    pairs.set_column_spacing(16);
    pairs.set_row_spacing(16);
    pairs.set_max_children_per_line(2);
    pairs.set_min_children_per_line(1);
    pairs.set_homogeneous(true);
    pairs.set_valign(gtk::Align::Start);
    for card in [
        category_card(ctx, &flows, link.clone()),
        availability_card(ctx, &flows, link.clone()),
        item_category_card(ctx, &flows, link.clone()),
        tags_card(ctx, &flows, &doc.tags, link.clone()),
        items_card(ctx, &flows, true, link.clone()),
        items_card(ctx, &flows, false, link.clone()),
    ] {
        card.set_valign(gtk::Align::Start);
        pairs.insert(&card, -1);
    }
    body.append(&pairs);
    body.append(&counterparties_card(ctx, &flows, link));
    Page::new("Contracts", body)
}

/// Characters whose login lacks the contracts or the corporation contracts scope.
fn scope_banners(body: &gtk::Box, chars: &[std::sync::Arc<podstack_core::Character>]) {
    let names = |key: &str| chars.iter().filter(|c| scope_failed(c, key)).map(|c| c.name()).collect::<Vec<_>>();
    let personal = names("contracts");
    if !personal.is_empty() {
        body.append(&w::banner(
            "lock",
            &format!("{} need{} to log in again with \"Read contracts\" to show their contracts.", personal.join(", "), if personal.len() == 1 { "s" } else { "" }),
            w::BannerKind::Notice,
        ));
    }
    let corp = names("corp_contracts");
    if !corp.is_empty() {
        body.append(&w::banner(
            "lock",
            &format!(
                "Corporation contracts (a buyback's or courier corporation's work) need {CORP_SCOPE}. Log in again with \"Add character\" for {}. If you use your own EVE application, enable that scope on it at developers.eveonline.com first.",
                corp.join(", ")
            ),
            w::BannerKind::Notice,
        ));
    }
}

fn controls(ctx: &Ctx, options: &[(String, Scope)], scope: Scope, period: Period) -> gtk::Box {
    let row = w::hbox(16);
    let labels: Vec<&str> = Period::ALL.iter().map(|p| p.label()).collect();
    let selected = Period::ALL.iter().position(|p| *p == period).unwrap_or(1);
    let periods = {
        let ctx = ctx.clone();
        w::segmented(&labels, selected, move |i| {
            ctx.remember(PERIOD_KEY, Period::ALL[i].key());
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        })
    };
    row.append(&labelled("Period", &periods));
    let names: Vec<&str> = options.iter().map(|o| o.0.as_str()).collect();
    let dd = w::dropdown(&names, options.iter().position(|o| o.1 == scope).unwrap_or(0) as u32);
    {
        let (ctx, keys) = (ctx.clone(), options.iter().map(|o| o.1.key()).collect::<Vec<_>>());
        dd.connect_selected_notify(move |d| {
            if let Some(k) = keys.get(d.selected() as usize) {
                ctx.remember(SCOPE_KEY, k.clone());
                let ctx = ctx.clone();
                gtk::glib::idle_add_local_once(move || ctx.rebuild());
            }
        });
    }
    row.append(&labelled("Whose", &dd));
    row
}

/// A stat tile whose value is green for a gain and red for a loss.
fn money_tile(label: &str, v: f64, sub: Option<&str>) -> gtk::Box {
    let t = w::stat_tile(label, &signed(v), sub);
    if let Some(l) = t.first_child().and_then(|c| c.next_sibling()) {
        if v != 0.0 {
            l.add_css_class(if v > 0.0 { "success-text" } else { "error-text" });
        }
    }
    t.set_tooltip_text(Some(&fmt::isk(v)));
    t
}

fn sum(flows: &[&Flow], cat: Category, f: impl Fn(&Flow) -> f64) -> (f64, usize) {
    let of: Vec<&&Flow> = flows.iter().filter(|x| x.category == cat).collect();
    (of.iter().map(|x| f(x)).sum(), of.len())
}

fn plural(n: usize, one: &str) -> String {
    format!("{} {one}{}", fmt::int(n as i64), if n == 1 { "" } else { "s" })
}

fn tiles(flows: &[&Flow], t: &Totals, e: &ct::Exposure, period: Period) -> gtk::FlowBox {
    let spent = w::stat_tile("Spent", &fmt::isk_short(t.isk_out), Some(&format!("{} paid", plural(flows.iter().filter(|f| f.isk_out > 0.0).count(), "contract"))));
    spent.set_tooltip_text(Some(&fmt::isk(t.isk_out)));
    let made = w::stat_tile("Made", &fmt::isk_short(t.isk_in), Some(&format!("{} paid you", plural(flows.iter().filter(|f| f.isk_in > 0.0).count(), "contract"))));
    made.set_tooltip_text(Some(&fmt::isk(t.isk_in)));
    let valued = flows.iter().any(|f| f.value_in.is_some() || f.value_out.is_some());
    let with_items = t.net() + t.value_in - t.value_out;
    let net_sub = if valued { format!("{} with items", signed(with_items)) } else { period.label().to_string() };
    let net = money_tile("Net", t.net(), Some(&net_sub));
    let (hauled, hauls) = sum(flows, Category::Hauled, |f| f.isk_in);
    let (shipped, ships) = sum(flows, Category::Shipped, |f| f.isk_out);
    let hauling = w::stat_tile(
        "Hauling income",
        &fmt::isk_short(hauled),
        Some(&if ships > 0 { format!("{} · paid {} for {}", plural(hauls, "courier"), fmt::isk_short(shipped), plural(ships, "courier")) } else { plural(hauls, "courier") }),
    );
    let mut tiles = vec![spent, made, net, hauling];
    if valued {
        let items = w::stat_tile("Items in / out", &format!("{} / {}", fmt::isk_short(t.value_in), fmt::isk_short(t.value_out)), Some("Hub value when finished"));
        items.set_tooltip_text(Some(&format!(
            "Received items worth {}\nGave items worth {}\nValued once, at the contracts price hub on the day PodStack first saw each contract finished.",
            fmt::isk(t.value_in),
            fmt::isk(t.value_out)
        )));
        tiles.push(items);
    }
    let tied = e.listed + e.hauling_collateral + e.shipping_collateral;
    let mut parts = Vec::new();
    if e.listed_count > 0 {
        parts.push(format!("{} listed", fmt::isk_short(e.listed)));
    }
    if e.hauling_count > 0 {
        parts.push(format!("{} collateral", fmt::isk_short(e.hauling_collateral)));
    }
    if e.shipping_count > 0 {
        parts.push(format!("{} in transit", fmt::isk_short(e.shipping_collateral)));
    }
    let tile = w::stat_tile("ISK tied up now", &fmt::isk_short(tied), Some(&if parts.is_empty() { "Nothing open".to_string() } else { parts.join(" · ") }));
    tile.set_tooltip_text(Some(&format!(
        "Listed for sale: {} ({})\nCollateral on couriers you're hauling: {} ({})\nCargo of your couriers in transit: {} ({}), rewards owed {}",
        fmt::isk(e.listed),
        plural(e.listed_count, "contract"),
        fmt::isk(e.hauling_collateral),
        plural(e.hauling_count, "courier"),
        fmt::isk(e.shipping_collateral),
        plural(e.shipping_count, "courier"),
        fmt::isk_short(e.shipping_rewards)
    )));
    tiles.push(tile);
    w::tile_row(tiles)
}

/// Contracts waiting on the user (in scope): assigned and not accepted, couriers due within a day.
fn waiting_card(ctx: &Ctx, chars: &[std::sync::Arc<podstack_core::Character>], mine: &Mine, now: DateTime<Utc>) -> Option<gtk::Box> {
    let listed: Vec<&Contract> = chars.iter().flat_map(|c| [&c.contracts, &c.corp_contracts]).flatten().flat_map(|d| d.contracts.iter()).collect();
    let a = ct::attention(listed.iter().copied(), mine, now);
    if a.count() == 0 {
        return None;
    }
    let by_id: HashMap<i64, &Contract> = listed.iter().map(|c| (c.contract_id, *c)).collect();
    let list = w::vbox(0);
    for (id, due) in a.due_soon.iter().map(|i| (*i, true)).chain(a.assigned.iter().map(|i| (*i, false))) {
        let Some(c) = by_id.get(&id) else { continue };
        let (icon, headline, supporting, status) = if due {
            let when = ct::courier_due(c).map(|d| if d > now { format!("due in {}", fmt::duration(d - now, 2)) } else { "overdue".into() }).unwrap_or_default();
            ("local_shipping", format!("Deliver: {}", comms::contract_details(c)), format!("Collateral {} · reward {}", fmt::isk_short(c.collateral.unwrap_or(0.0)), fmt::isk_short(c.reward.unwrap_or(0.0))), w::status(&when, w::Status::Warn))
        } else {
            let who = c.assignee_name.clone().unwrap_or_default();
            let value = comms::contract_value(c).map(|v| format!(" · {}", fmt::isk_short(v))).unwrap_or_default();
            (
                "assignment_ind",
                format!("{} from {}", comms::title_case(&c.kind), comms::contract_from(c)),
                format!("{} · for {who}{value} · expires {}", comms::contract_details(c), fmt::relative(c.date_expired)),
                w::status("Waiting", w::Status::Info),
            )
        };
        let trailing = w::hbox(8);
        trailing.append(&status);
        let open = w::icon_button("chevron_right", "Contract details");
        {
            let (ctx, c) = (ctx.clone(), (*c).clone());
            open.connect_clicked(move |_| open_contract(&ctx, &c));
        }
        trailing.append(&open);
        let i = w::icon(icon);
        list.append(&w::list_row(Some(i.upcast_ref()), &headline, Some(&supporting), Some(trailing.upcast_ref())));
    }
    Some(w::card_with("Waiting on you", Some("Accept these in game"), &[], &list))
}

fn chart_subtitle(period: Period) -> String {
    match period {
        Period::All => "All time".into(),
        p => format!("Last {}", p.label()),
    }
}

fn legend() -> gtk::Widget {
    let b = w::hbox(12);
    b.set_valign(gtk::Align::Center);
    for (class, label) in [("ctr-dot-in", "Made"), ("ctr-dot-out", "Spent")] {
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.add_css_class("ctr-dot");
        dot.add_css_class(class);
        dot.set_valign(gtk::Align::Center);
        b.append(&dot);
        b.append(&w::line(label, &["body-small", "dim"]));
    }
    b.upcast()
}

// ---------- chart ----------

const CHART_HEIGHT: i32 = 220;
const PAD_L: f64 = 64.0;
const PAD_R: f64 = 16.0;
const PAD_T: f64 = 12.0;
const PAD_B: f64 = 26.0;

fn source(cr: &gtk::cairo::Context, role: &str, alpha: f64) {
    let c = crate::theme::rgba(role);
    cr.set_source_rgba(c.red() as f64, c.green() as f64, c.blue() as f64, c.alpha() as f64 * alpha);
}

/// A bar rounded at its top.
fn bar(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    let r = (w / 2.0).min(4.0).min(h);
    cr.new_path();
    cr.move_to(x, y + h);
    cr.line_to(x, y + r);
    cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.line_to(x + w, y + h);
    cr.close_path();
}

/// ISK in (primary) and out (tertiary) side by side per day or month; hovering a bucket shows its numbers.
fn chart(buckets: Vec<(NaiveDate, Totals)>, by_month: bool) -> gtk::Widget {
    let n = buckets.len().max(1);
    let top = buckets.iter().map(|(_, t)| t.isk_in.max(t.isk_out)).fold(0.0f64, f64::max).max(1.0) * 1.08;
    let ticks = comms::nice_ticks(0.0, top, 4);
    let top = ticks.last().copied().unwrap_or(top).max(top);
    let buckets = Rc::new(buckets);
    let hover: Rc<Cell<Option<usize>>> = Rc::new(Cell::new(None));
    let plot_w = |w: f64| (w - PAD_L - PAD_R).max(1.0);
    let slot = move |w: f64| plot_w(w) / n as f64;
    let y_of = move |h: f64, v: f64| PAD_T + (1.0 - v / top) * (h - PAD_T - PAD_B).max(1.0);
    let label = move |d: NaiveDate| if by_month { d.format("%b %Y").to_string() } else { d.format("%b %-d").to_string() };

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
                let text = label(buckets[i].0);
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
            for (i, (_, t)) in buckets.iter().enumerate() {
                let cx = PAD_L + (i as f64 + 0.5) * s;
                let dim = hover.get().is_some_and(|h| h != i);
                for (v, role, x) in [(t.isk_in, "primary", cx - bw - 1.0), (t.isk_out, "tertiary", cx + 1.0)] {
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
    tip.add_css_class("ctr-chart-tip");
    tip.set_halign(gtk::Align::Start);
    tip.set_valign(gtk::Align::Start);
    tip.set_can_target(false);
    tip.set_visible(false);
    let tip_day = w::line("", &["body-small", "dim"]);
    let tip_net = w::line("", &["title-small", "numeric"]);
    let tip_detail = w::line("", &["body-small"]);
    let tip_count = w::line("", &["body-small", "dim"]);
    for l in [&tip_day, &tip_net, &tip_detail, &tip_count] {
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
            let (day, t) = &buckets[i];
            tip_day.set_text(&if by_month { day.format("%B %Y").to_string() } else { day.format("%A, %b %-d").to_string() });
            tip_net.set_text(&format!("{} ISK net", signed(t.net())));
            tip_net.remove_css_class("success-text");
            tip_net.remove_css_class("error-text");
            if t.net() != 0.0 {
                tip_net.add_css_class(if t.net() > 0.0 { "success-text" } else { "error-text" });
            }
            tip_detail.set_text(&format!("Made {} · spent {}", fmt::isk_short(t.isk_in), fmt::isk_short(t.isk_out)));
            tip_count.set_text(&if t.count == 0 { "No contracts".to_string() } else { plural(t.count, "contract") });
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

// ---------- breakdowns ----------

type LinkFn = Rc<dyn Fn(Link) -> Link>;

/// Opens the Contracts tab with this breakdown row's filter.
fn follow(ctx: &Ctx, link: &LinkFn, l: Link) {
    browse::show(ctx, link(l));
}

fn money_cell(v: f64) -> gtk::Widget {
    let l = w::line(&signed(v), &["numeric"]);
    l.set_xalign(1.0);
    if v != 0.0 {
        l.add_css_class(if v > 0.0 { "comms-isk-in" } else { "comms-isk-out" });
    }
    l.set_tooltip_text(Some(&fmt::isk(v)));
    l.upcast()
}

fn open_button(ctx: &Ctx, link: &LinkFn, l: Link) -> gtk::Widget {
    let b = w::icon_button("chevron_right", "Show these contracts");
    let (ctx, link) = (ctx.clone(), link.clone());
    b.connect_clicked(move |_| follow(&ctx, &link, l.clone()));
    b.upcast()
}

/// A breakdown table of (label, totals) rows: contracts, in, out, net, and a link to the contracts.
fn totals_table<K: Ord + Clone + 'static>(
    ctx: &Ctx,
    rows: Vec<(K, Totals)>,
    title: &str,
    cell: impl Fn(&K) -> gtk::Widget + 'static,
    to_link: impl Fn(&K) -> Link + 'static,
    link: LinkFn,
    empty: &str,
) -> gtk::Widget {
    let to_link = Rc::new(to_link);
    let (ctx2, link2, to_link2) = (ctx.clone(), link.clone(), to_link.clone());
    let ctx3 = ctx.clone();
    Table::new(rows)
        .widget_sorted(title, move |r| cell(&r.0), |a, b| a.0.cmp(&b.0))
        .expand()
        .num("Contracts", |r| fmt::int(r.1.count as i64), |r| r.1.count as f64)
        .num("In", |r| fmt::isk_short(r.1.isk_in), |r| r.1.isk_in)
        .num("Out", |r| fmt::isk_short(r.1.isk_out), |r| r.1.isk_out)
        .widget_sorted("Net", |r| money_cell(r.1.net()), |a, b| a.1.net().total_cmp(&b.1.net()))
        .widget("", move |r| open_button(&ctx2, &link2, to_link2(&r.0)))
        .on_activate(move |r| follow(&ctx3, &link, to_link(&r.0)))
        .natural_height()
        .empty_text(empty)
        .build()
}

fn label_with_icon(icon: &str, label: &str) -> gtk::Widget {
    let b = w::hbox(10);
    let i = w::icon_sized(icon, 18);
    i.add_css_class("dim");
    b.append(&i);
    b.append(&w::line(label, &[]));
    b.upcast()
}

const NOTHING: &str = "No finished contracts in this period.";

fn category_card(ctx: &Ctx, flows: &[&Flow], link: LinkFn) -> gtk::Box {
    let rows = ct::group_by(flows.iter().copied(), |f| Some(f.category));
    let t = totals_table(
        ctx,
        rows,
        "Kind",
        |c: &Category| label_with_icon(c.icon(), c.label()),
        |c| Link { category: Some(*c), ..Default::default() },
        link,
        NOTHING,
    );
    w::card_with("By kind", Some("Bought, sold, hauled and shipped"), &[], &t)
}

fn availability_label(a: &str) -> (&'static str, String) {
    let icon = match a {
        "alliance" => "shield",
        "corporation" => "corporate_fare",
        "personal" => "person",
        _ => "public",
    };
    (icon, comms::title_case(a))
}

fn availability_card(ctx: &Ctx, flows: &[&Flow], link: LinkFn) -> gtk::Box {
    let rows = ct::group_by(flows.iter().copied(), |f| Some(f.availability.clone()));
    let t = totals_table(
        ctx,
        rows,
        "Availability",
        |a: &String| {
            let (icon, label) = availability_label(a);
            label_with_icon(icon, &label)
        },
        |a| Link { availability: Some(a.clone()), ..Default::default() },
        link,
        NOTHING,
    );
    w::card_with("By availability", Some("Alliance and corporation contracts against public and personal ones"), &[], &t)
}

/// Spent on and made from each market category of items, the ISK shared out by the items' worth.
fn item_category_card(ctx: &Ctx, flows: &[&Flow], link: LinkFn) -> gtk::Box {
    let mut m: BTreeMap<String, (f64, f64, i64, i64)> = BTreeMap::new();
    for i in ct::by_item(flows.iter().copied(), true) {
        let e = m.entry(ct::item_category(i.type_id)).or_default();
        e.0 += i.isk;
        e.2 += i.quantity;
    }
    for i in ct::by_item(flows.iter().copied(), false) {
        let e = m.entry(ct::item_category(i.type_id)).or_default();
        e.1 += i.isk;
        e.3 += i.quantity;
    }
    let mut rows: Vec<(String, (f64, f64, i64, i64))> = m.into_iter().collect();
    rows.sort_by(|a, b| (b.1 .0 + b.1 .1).total_cmp(&(a.1 .0 + a.1 .1)));
    let (ctx2, link2) = (ctx.clone(), link.clone());
    let ctx3 = ctx.clone();
    let t = Table::new(rows)
        .text("Category", |r| r.0.clone())
        .expand()
        .num("Bought", |r| fmt::isk_short(r.1 .0), |r| r.1 .0)
        .num("Units in", |r| fmt::int(r.1 .2), |r| r.1 .2 as f64)
        .num("Sold", |r| fmt::isk_short(r.1 .1), |r| r.1 .1)
        .num("Units out", |r| fmt::int(r.1 .3), |r| r.1 .3 as f64)
        .widget("", move |r| open_button(&ctx2, &link2, Link { item_category: Some(r.0.clone()), ..Default::default() }))
        .on_activate(move |r| follow(&ctx3, &link, Link { item_category: Some(r.0.clone()), ..Default::default() }))
        .natural_height()
        .empty_text("No items bought or sold in this period.")
        .build();
    w::card_with("By item category", Some("ISK paid and received, shared out over each contract's items"), &[], &t)
}

fn tags_card(ctx: &Ctx, flows: &[&Flow], tags: &BTreeMap<i64, Vec<String>>, link: LinkFn) -> gtk::Box {
    let rows = ct::by_tag(flows.iter().copied(), tags);
    let empty = if tags.is_empty() {
        "Tag contracts in their details (doctrine ships, SRP, buyback, a project…) to total them here."
    } else {
        "No tagged contracts finished in this period."
    };
    let t = totals_table(
        ctx,
        rows,
        "Tag",
        |t: &String| label_with_icon("sell", t),
        |t| Link { tag: Some(t.clone()), ..Default::default() },
        link,
        empty,
    );
    w::card_with("By tag", None, &[], &t)
}

fn items_card(ctx: &Ctx, flows: &[&Flow], bought: bool, link: LinkFn) -> gtk::Box {
    let mut rows = ct::by_item(flows.iter().copied(), bought);
    rows.truncate(TOP);
    let names: HashMap<i64, String> = rows.iter().map(|i| (i.type_id, podstack_core::sde::MARKET.get(i.type_id).map(|t| t.name.clone()).unwrap_or_else(|| format!("Type #{}", i.type_id)))).collect();
    let names = Rc::new(names);
    let category = if bought { Category::Bought } else { Category::Sold };
    let mk = {
        let names = names.clone();
        move |id: i64| Link { type_id: Some((id, names.get(&id).cloned().unwrap_or_default())), category: Some(category), ..Default::default() }
    };
    let mk = Rc::new(mk);
    let (ctx2, link2, mk2) = (ctx.clone(), link.clone(), mk.clone());
    let ctx3 = ctx.clone();
    let n1 = names.clone();
    let n2 = names.clone();
    let t = Table::new(rows)
        .widget_sorted("Item", move |i| type_cell(i.type_id, n1.get(&i.type_id).map(|s| s.as_str()).unwrap_or("")), move |a, b| n2.get(&a.type_id).cmp(&n2.get(&b.type_id)))
        .expand()
        .num("Units", |i| fmt::int(i.quantity), |i| i.quantity as f64)
        .num("ISK", |i| fmt::isk_short(i.isk), |i| i.isk)
        .num("Per unit", |i| if i.quantity > 0 { fmt::isk_short(i.isk / i.quantity as f64) } else { String::new() }, |i| if i.quantity > 0 { i.isk / i.quantity as f64 } else { 0.0 })
        .num("Contracts", |i| fmt::int(i.contracts as i64), |i| i.contracts as f64)
        .sort_by(2, true)
        .widget("", move |i| open_button(&ctx2, &link2, mk2(i.type_id)))
        .on_activate(move |i| follow(&ctx3, &link, mk(i.type_id)))
        .natural_height()
        .empty_text(if bought { "Nothing bought in this period." } else { "Nothing sold in this period." })
        .build();
    let (title, sub) = if bought { ("Top items bought", "ISK paid, shared out over each contract's items") } else { ("Top items sold", "ISK received, shared out over each contract's items") };
    w::card_with(title, Some(sub), &[], &t)
}

/// One row per counterparty with their corporation.
fn counterparties_card(ctx: &Ctx, flows: &[&Flow], link: LinkFn) -> gtk::Box {
    let corps: HashMap<i64, String> = flows.iter().filter_map(|f| Some((f.counterparty.as_ref()?.0, f.counterparty_corp.as_ref()?.1.clone()))).collect();
    let mut rows = ct::group_by(flows.iter().copied(), |f| f.counterparty.clone());
    rows.truncate(TOP);
    let corps = Rc::new(corps);
    let (ctx2, link2) = (ctx.clone(), link.clone());
    let ctx3 = ctx.clone();
    let c1 = corps.clone();
    let to_link = |k: &(i64, String)| Link { counterparty: Some(k.clone()), ..Default::default() };
    let t = Table::new(rows)
        .widget_sorted("Counterparty", |r| character_cell(r.0 .0, &r.0 .1), |a, b| a.0 .1.to_lowercase().cmp(&b.0 .1.to_lowercase()))
        .expand()
        .text("Corporation", move |r| c1.get(&r.0 .0).cloned().unwrap_or_default())
        .num("Contracts", |r| fmt::int(r.1.count as i64), |r| r.1.count as f64)
        .num("Paid them", |r| fmt::isk_short(r.1.isk_out), |r| r.1.isk_out)
        .num("Got from them", |r| fmt::isk_short(r.1.isk_in), |r| r.1.isk_in)
        .widget_sorted("Net", |r| money_cell(r.1.net()), |a, b| a.1.net().total_cmp(&b.1.net()))
        .widget("", move |r| open_button(&ctx2, &link2, to_link(&r.0)))
        .on_activate(move |r| follow(&ctx3, &link, to_link(&r.0)))
        .natural_height()
        .empty_text(NOTHING)
        .build();
    w::card_with("Top counterparties", Some("Who you trade with most, by ISK both ways"), &[], &t)
}

/// One character's (or corporation's) figures in the per-character table.
struct PartyRow {
    party: Party,
    name: String,
    bought: f64,
    sold: f64,
    hauled: f64,
    shipped: f64,
    net: f64,
    count: usize,
    /// (used, max) contract slots, for characters with both known.
    slots: Option<(u32, u32)>,
}

/// Bought, sold, hauled, shipped and net per character and corporation, with contract slots.
fn parties_card(ctx: &Ctx, chars: &[std::sync::Arc<podstack_core::Character>], mine: &Mine, flows: &[&Flow], period: Period, link: LinkFn) -> gtk::Box {
    let mut by: BTreeMap<Party, Vec<&Flow>> = BTreeMap::new();
    for f in flows {
        by.entry(f.party).or_default().push(f);
    }
    let mut rows: Vec<PartyRow> = Vec::new();
    let mk = |party: Party, name: String, fs: &[&Flow], slots: Option<(u32, u32)>| {
        let cat = |c: Category, f: fn(&Flow) -> f64| fs.iter().filter(|x| x.category == c).map(|x| f(x)).sum::<f64>();
        PartyRow {
            party,
            name,
            bought: cat(Category::Bought, |f| f.isk_out),
            sold: cat(Category::Sold, |f| f.isk_in),
            hauled: cat(Category::Hauled, |f| f.isk_in),
            shipped: cat(Category::Shipped, |f| f.isk_out),
            net: fs.iter().map(|f| f.net()).sum(),
            count: fs.len(),
            slots,
        }
    };
    for c in chars {
        let p = Party::Character(c.id);
        let slots = c.contracts.as_ref().filter(|_| c.skills.is_some()).map(|d| (d.slots_used, c.max_contracts()));
        rows.push(mk(p, c.name(), by.get(&p).map(|v| v.as_slice()).unwrap_or(&[]), slots));
    }
    for (id, name) in &mine.corporations {
        let p = Party::Corporation(*id);
        if let Some(fs) = by.get(&p) {
            rows.push(mk(p, format!("{name} (corporation)"), fs, None));
        }
    }
    rows.sort_by(|a, b| (b.bought + b.sold + b.hauled + b.shipped).total_cmp(&(a.bought + a.sold + a.hauled + a.shipped)));
    let (ctx2, link2) = (ctx.clone(), link.clone());
    let ctx3 = ctx.clone();
    let to_link = |r: &PartyRow| Link { scope: Scope::Of(r.party), ..Default::default() };
    // the per-party links keep their own scope, not the page's (which is everyone here)
    let link_keep = move |l: Link| {
        let scope = l.scope;
        let mut l = link2(l);
        l.scope = scope;
        l
    };
    let link_keep: LinkFn = Rc::new(link_keep);
    let link_keep2 = link_keep.clone();
    let t = Table::new(rows)
        .widget_sorted(
            "Character",
            |r| match r.party {
                Party::Character(id) => character_cell(id, &r.name),
                Party::Corporation(id) => {
                    let row = w::hbox(10);
                    row.append(&w::eve_image(&config::corporation_logo(id, 64), 24));
                    row.append(&w::line(&r.name, &[]));
                    row.upcast()
                }
            },
            |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        )
        .expand()
        .num("Bought", |r| fmt::isk_short(r.bought), |r| r.bought)
        .num("Sold", |r| fmt::isk_short(r.sold), |r| r.sold)
        .num("Hauled", |r| fmt::isk_short(r.hauled), |r| r.hauled)
        .num("Shipped", |r| fmt::isk_short(r.shipped), |r| r.shipped)
        .widget_sorted("Net", |r| money_cell(r.net), |a, b| a.net.total_cmp(&b.net))
        .num("Contracts", |r| fmt::int(r.count as i64), |r| r.count as f64)
        .widget_sorted(
            "Slots",
            |r| {
                let l = match r.slots {
                    Some((used, max)) => {
                        let l = w::line(&format!("{used} / {max}"), &["numeric"]);
                        if used >= max {
                            l.add_css_class("warning-text");
                            l.set_tooltip_text(Some("Every contract slot is in use"));
                        }
                        l
                    }
                    None => w::line("—", &["dim"]),
                };
                l.set_xalign(1.0);
                l.upcast()
            },
            |a, b| a.slots.map(|s| s.0).cmp(&b.slots.map(|s| s.0)),
        )
        .widget("", move |r| open_button(&ctx2, &link_keep, to_link(r)))
        .on_activate(move |r| follow(&ctx3, &link_keep2, to_link(r)))
        .natural_height()
        .build();
    let sub = format!(
        "{} · each one's figures include contracts between your own characters · slots are open personal contracts",
        chart_subtitle(period)
    );
    w::card_with("Per character and corporation", Some(&sub), &[], &t)
}
