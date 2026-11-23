//! The Contracts tab: every contract PodStack has seen (the archive keeps them past ESI's 30 days, plus whatever the
//! characters list that it hasn't caught yet), filtered by state, type, character or corporation, the user's side,
//! availability, period, tag and text, with what each meant for the user and its price against the market. The
//! Overview's breakdowns link here with a filter of their own (category, counterparty, item), shown as chips. The
//! filtered rows export to CSV.

use super::common::{date_cell, open_contract, status_kind};
use super::Tab;
use crate::ctx::{Ctx, Page};
use crate::pages::industry::form::labelled;
use crate::pages::market_orders::set_child;
use crate::widgets::{self as w, Btn, Table};
use chrono::Utc;
use gtk::gio;
use gtk::prelude::*;
use podstack_core::features::comms;
use podstack_core::features::contracts::{self as ct, ArchivedContract, Category, Filter, Flow, Mine, Party, Period, Role, Scope};
use podstack_core::fmt;
use std::rc::Rc;

const STATE_KEY: &str = "contracts.browse.state";
const KIND_KEY: &str = "contracts.browse.kind";
const SCOPE_KEY: &str = "contracts.browse.scope";
const ROLE_KEY: &str = "contracts.browse.role";
const AVAILABILITY_KEY: &str = "contracts.browse.availability";
const PERIOD_KEY: &str = "contracts.browse.period";
const TAG_KEY: &str = "contracts.browse.tag";
const CATEGORY_KEY: &str = "contracts.browse.category";
/// "<id>|<name>"
const COUNTERPARTY_KEY: &str = "contracts.browse.counterparty";
const ITEM_CATEGORY_KEY: &str = "contracts.browse.item_category";
/// "<type id>|<name>"
const TYPE_KEY: &str = "contracts.browse.type";
const SEARCH_KEY: &str = "contracts.browse.search";

const STATES: [(&str, &str); 3] = [("all", "Open and completed"), ("open", "Open"), ("completed", "Completed")];
const KINDS: [(&str, &str); 5] = [("", "All types"), ("item_exchange", "Item exchange"), ("auction", "Auction"), ("courier", "Courier"), ("loan", "Loan")];
const AVAILABILITIES: [(&str, &str); 5] =
    [("", "Any availability"), ("alliance", "Alliance"), ("corporation", "Corporation"), ("personal", "Personal"), ("public", "Public")];

/// The choices for "whose": everyone, each character, each corporation.
pub(crate) fn scope_options(mine: &Mine) -> Vec<(String, Scope)> {
    let mut v = vec![("All characters".to_string(), Scope::All)];
    v.extend(mine.characters.iter().map(|(id, n)| (n.clone(), Scope::Of(Party::Character(*id)))));
    v.extend(mine.corporations.iter().map(|(id, n)| (format!("{n} (corporation)"), Scope::Of(Party::Corporation(*id)))));
    v
}

/// A filter to open the tab with (from the Overview's breakdowns). Everything else is reset.
#[derive(Clone, Debug, Default)]
pub struct Link {
    pub scope: Scope,
    pub period: Option<Period>,
    pub category: Option<Category>,
    pub availability: Option<String>,
    pub counterparty: Option<(i64, String)>,
    pub item_category: Option<String>,
    pub type_id: Option<(i64, String)>,
    pub tag: Option<String>,
}

/// Opens the Contracts tab filtered by `link`.
pub fn show(ctx: &Ctx, link: Link) {
    ctx.remember(STATE_KEY, "all");
    ctx.remember(KIND_KEY, "");
    ctx.remember(ROLE_KEY, "");
    ctx.remember(SEARCH_KEY, "");
    ctx.remember(SCOPE_KEY, link.scope.key());
    ctx.remember(PERIOD_KEY, link.period.map(|p| p.key()).unwrap_or(""));
    ctx.remember(CATEGORY_KEY, link.category.map(|c| c.label()).unwrap_or(""));
    ctx.remember(AVAILABILITY_KEY, link.availability.unwrap_or_default());
    ctx.remember(COUNTERPARTY_KEY, link.counterparty.map(|(id, n)| format!("{id}|{n}")).unwrap_or_default());
    ctx.remember(ITEM_CATEGORY_KEY, link.item_category.unwrap_or_default());
    ctx.remember(TYPE_KEY, link.type_id.map(|(id, n)| format!("{id}|{n}")).unwrap_or_default());
    ctx.remember(TAG_KEY, link.tag.unwrap_or_default());
    // once the click handler has returned: opening the tab replaces the widget being clicked
    let ctx = ctx.clone();
    gtk::glib::idle_add_local_once(move || super::open(&ctx, Tab::Contracts));
}

/// A remembered "<id>|<name>".
fn id_name(ctx: &Ctx, key: &str) -> Option<(i64, String)> {
    let v = ctx.recall(key)?;
    let (id, name) = v.split_once('|')?;
    Some((id.parse().ok()?, name.to_string()))
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.is_empty())
}

/// The filter as remembered.
fn current_filter(ctx: &Ctx, mine: &Mine) -> Filter {
    let scope = Scope::from_key(&ctx.recall(SCOPE_KEY).unwrap_or_default());
    let party = match scope {
        Scope::Of(p) if mine.is_mine(p.id()) => Some(p),
        _ => None,
    };
    Filter {
        completed: match ctx.recall(STATE_KEY).as_deref() {
            Some("open") => Some(false),
            Some("completed") => Some(true),
            _ => None,
        },
        kind: non_empty(ctx.recall(KIND_KEY)),
        party,
        role: non_empty(ctx.recall(ROLE_KEY)).and_then(|r| Role::ALL.into_iter().find(|x| x.label() == r)),
        availability: non_empty(ctx.recall(AVAILABILITY_KEY)),
        period: non_empty(ctx.recall(PERIOD_KEY)).map(|k| Period::from_key(&k)).filter(|p| *p != Period::All),
        tag: non_empty(ctx.recall(TAG_KEY)),
        category: non_empty(ctx.recall(CATEGORY_KEY)).and_then(|l| Category::ALL.into_iter().find(|c| c.label() == l)),
        counterparty: id_name(ctx, COUNTERPARTY_KEY).map(|x| x.0),
        item_category: non_empty(ctx.recall(ITEM_CATEGORY_KEY)),
        type_id: id_name(ctx, TYPE_KEY).map(|x| x.0),
        query: ctx.recall(SEARCH_KEY).unwrap_or_default(),
    }
}

/// A contract with what it meant for the user, worked out once per build.
pub(crate) struct Row {
    pub e: ArchivedContract,
    pub flows: Vec<Flow>,
    pub roles: Vec<(Party, Role)>,
    pub tags: Vec<String>,
    pub note: String,
    /// The user's parties on it, e.g. "Alice (accepted)".
    pub mine: String,
}

impl Row {
    fn contract(&self) -> &podstack_core::domains::contracts::Contract {
        &self.e.contract
    }
    /// What it was for the user ("Bought", "Sold, Bought"), or the open contract's state for them.
    fn what(&self) -> String {
        let mut cats: Vec<Category> = self.flows.iter().map(|f| f.category).collect();
        cats.sort();
        cats.dedup();
        if cats.is_empty() {
            String::new()
        } else {
            cats.iter().map(|c| c.label()).collect::<Vec<_>>().join(", ")
        }
    }
    /// The user's ISK result: of `party` alone when given, else of every side (None for an internal move, which
    /// nets to nothing, and for open or failed-to-finish contracts).
    fn net(&self, party: Option<Party>) -> Option<f64> {
        let fs: Vec<&Flow> = self.flows.iter().filter(|f| party.is_none_or(|p| f.party == p)).collect();
        if fs.is_empty() || (party.is_none() && fs.iter().all(|f| f.internal)) {
            return None;
        }
        Some(fs.iter().map(|f| f.net()).sum())
    }
    /// The items' worth for the acceptor at the settings' prices, when valued.
    fn deal(&self, at_sell: bool) -> Option<ct::Deal> {
        let v = self.e.valuation.as_ref()?;
        ct::deal(self.contract(), v.included(at_sell), v.asked(at_sell)).filter(|d| d.worth != 0.0)
    }
}

pub(crate) fn rows(ctx: &Ctx) -> (Vec<Rc<Row>>, Mine) {
    let chars = ctx.core().characters();
    let archive = ct::archive_snapshot();
    let doc = ct::load_doc();
    let mine = Mine::of(&chars);
    let at_sell = doc.settings.value_at_sell;
    let mut known = ct::all_known(&chars, &archive);
    // already in the table's order, so its initial sort doesn't move rows (the view would follow the first one)
    known.sort_by_key(|e| std::cmp::Reverse(e.contract.last_date()));
    let rows = known
        .into_iter()
        .map(|e| {
            let flows = ct::flows(&e.contract, &mine, e.valuation.as_ref(), at_sell);
            let roles = ct::roles(&e.contract, &mine);
            let mut parties: Vec<String> = Vec::new();
            for (p, r) in &roles {
                let s = format!("{} ({})", mine.name(*p), r.label().to_lowercase());
                if !parties.contains(&s) {
                    parties.push(s);
                }
            }
            let id = e.contract.contract_id;
            Rc::new(Row {
                flows,
                roles,
                tags: doc.tags.get(&id).cloned().unwrap_or_default(),
                note: doc.notes.get(&id).cloned().unwrap_or_default(),
                mine: parties.join(", "),
                e,
            })
        })
        .collect();
    (rows, mine)
}

pub fn build(ctx: &Ctx) -> Page {
    let body = w::page_body();
    body.set_vexpand(true);
    body.append(&super::tab_bar(ctx, Tab::Contracts));
    let (all, mine) = rows(ctx);
    if all.is_empty() {
        body.append(&w::empty_state(
            "assignment",
            "No contracts yet",
            "Contracts your characters and their corporations issue, accept or are assigned show up here after the next refresh, and stay after ESI forgets them.",
        ));
        return Page::new("Contracts", body).no_scroll();
    }
    let all = Rc::new(all);
    let doc = ct::load_doc();
    let at_sell = doc.settings.value_at_sell;

    let summary = w::line("", &["body-medium", "dim"]);
    let holder = w::vbox(0);
    holder.set_vexpand(true);
    let shown: Rc<std::cell::RefCell<Vec<Rc<Row>>>> = Rc::default();
    let refill: Rc<dyn Fn()> = {
        let (ctx, holder, all, summary, shown, mine) = (ctx.clone(), holder.clone(), all.clone(), summary.clone(), shown.clone(), mine.clone());
        Rc::new(move || {
            let f = current_filter(&ctx, &mine);
            let now = Utc::now();
            let rows: Vec<Rc<Row>> = all.iter().filter(|r| f.matches(r.contract(), &r.flows, &r.roles, &r.tags, now)).cloned().collect();
            summary.set_text(&summary_text(&rows, f.party, ct::settings().count_internal));
            let empty = if all.is_empty() { "No contracts yet." } else { "No contracts match these filters." };
            set_child(&holder, &table(&ctx, rows.clone(), f.party, at_sell, empty));
            *shown.borrow_mut() = rows;
        })
    };

    body.append(&filter_bar(ctx, &mine, &doc.all_tags(), refill.clone()));
    let chips = drill_chips(ctx, refill.clone());
    let actions = w::hbox(8);
    summary.set_hexpand(true);
    summary.set_valign(gtk::Align::Center);
    actions.append(&chips);
    actions.append(&summary);
    let copy = w::button_icon("content_copy", "Copy as CSV", Btn::Text);
    {
        let (ctx, shown) = (ctx.clone(), shown.clone());
        copy.connect_clicked(move |_| {
            let rows = shown.borrow();
            w::copy_to_clipboard(&ctx, &csv(&rows, at_sell));
        });
    }
    let export = w::button_icon("download", "Export CSV", Btn::Tonal);
    {
        let (ctx, shown) = (ctx.clone(), shown.clone());
        export.connect_clicked(move |_| {
            let rows = shown.borrow();
            save_csv(&ctx, csv(&rows, at_sell), rows.len());
        });
    }
    actions.append(&copy);
    actions.append(&export);
    body.append(&actions);
    body.append(&holder);
    refill();
    Page::new("Contracts", body).no_scroll()
}

/// "123 contracts · 4.5b in · 3.2b out · +1.3b net", over the finished ones among `rows`.
fn summary_text(rows: &[Rc<Row>], party: Option<Party>, count_internal: bool) -> String {
    let scope = party.map(Scope::Of).unwrap_or_default();
    let t = ct::Totals::of(rows.iter().flat_map(|r| r.flows.iter()).filter(|f| scope.includes(f, count_internal)));
    let n = rows.len();
    let mut s = format!("{} contract{}", fmt::int(n as i64), if n == 1 { "" } else { "s" });
    if t.count > 0 {
        s.push_str(&format!(" · {} in · {} out · {} net", fmt::isk_short(t.isk_in), fmt::isk_short(t.isk_out), signed(t.net())));
    }
    s
}

pub(crate) fn signed(v: f64) -> String {
    if v > 0.0 {
        format!("+{}", fmt::isk_short(v))
    } else {
        fmt::isk_short(v)
    }
}

/// The dropdowns, the search and the reset button.
fn filter_bar(ctx: &Ctx, mine: &Mine, tags: &[String], refill: Rc<dyn Fn()>) -> gtk::Box {
    let bar = w::vbox(8);
    let fields = gtk::FlowBox::new();
    fields.set_selection_mode(gtk::SelectionMode::None);
    fields.set_column_spacing(12);
    fields.set_row_spacing(8);
    fields.set_max_children_per_line(8);
    fields.set_homogeneous(false);

    // a dropdown remembering one of `options` (key, label) under `key`
    let add = |label: &str, key: &'static str, options: Vec<(String, String)>| {
        let current = ctx.recall(key).unwrap_or_default();
        let labels: Vec<&str> = options.iter().map(|o| o.1.as_str()).collect();
        let selected = options.iter().position(|o| o.0 == current).unwrap_or(0);
        let dd = w::dropdown(&labels, selected as u32);
        let (ctx, refill) = (ctx.clone(), refill.clone());
        dd.connect_selected_notify(move |d| {
            if let Some(o) = options.get(d.selected() as usize) {
                ctx.remember(key, o.0.clone());
                refill();
            }
        });
        fields.insert(&labelled(label, &dd), -1);
    };
    let owned = |v: &[(&str, &str)]| v.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect::<Vec<_>>();
    add("Show", STATE_KEY, owned(&STATES));
    add("Type", KIND_KEY, owned(&KINDS));
    add("Whose", SCOPE_KEY, scope_options(mine).into_iter().map(|(l, s)| (s.key(), l)).collect());
    let mut roles = vec![(String::new(), "Any side".to_string())];
    roles.extend(Role::ALL.iter().map(|r| (r.label().to_string(), if *r == Role::Assigned { "Assigned to me".into() } else { format!("{} by me", r.label()) })));
    add("My side", ROLE_KEY, roles);
    add("Availability", AVAILABILITY_KEY, owned(&AVAILABILITIES));
    let mut periods = vec![(String::new(), "Any time".to_string())];
    periods.extend(Period::ALL.iter().filter(|p| **p != Period::All).map(|p| (p.key().to_string(), format!("Last {}", p.label()))));
    add("Period", PERIOD_KEY, periods);
    if !tags.is_empty() {
        let mut t = vec![(String::new(), "Any tag".to_string())];
        t.extend(tags.iter().map(|x| (x.clone(), x.clone())));
        add("Tag", TAG_KEY, t);
    }
    bar.append(&fields);

    let row = w::hbox(12);
    let search = w::search_entry("Search type, title, parties, items or locations");
    search.set_text(&ctx.recall(SEARCH_KEY).unwrap_or_default());
    {
        let (ctx, refill) = (ctx.clone(), refill.clone());
        search.connect_search_changed(move |s| {
            ctx.remember(SEARCH_KEY, s.text().as_str());
            refill();
        });
    }
    row.append(&search);
    let reset = w::button_icon("filter_alt_off", "Clear filters", Btn::Text);
    {
        let ctx = ctx.clone();
        reset.connect_clicked(move |_| {
            for k in [STATE_KEY, KIND_KEY, SCOPE_KEY, ROLE_KEY, AVAILABILITY_KEY, PERIOD_KEY, TAG_KEY, CATEGORY_KEY, COUNTERPARTY_KEY, ITEM_CATEGORY_KEY, TYPE_KEY, SEARCH_KEY] {
                ctx.remember(k, "");
            }
            let ctx = ctx.clone();
            gtk::glib::idle_add_local_once(move || ctx.rebuild());
        });
    }
    row.append(&reset);
    bar.append(&row);
    bar
}

/// Chips for the filters only the Overview sets (category, counterparty, item category, item); clicking one drops it.
fn drill_chips(ctx: &Ctx, refill: Rc<dyn Fn()>) -> gtk::Box {
    let b = w::hbox(8);
    b.set_valign(gtk::Align::Center);
    let mut chips: Vec<(&'static str, String)> = Vec::new();
    if let Some(c) = non_empty(ctx.recall(CATEGORY_KEY)) {
        chips.push((CATEGORY_KEY, c));
    }
    if let Some((_, n)) = id_name(ctx, COUNTERPARTY_KEY) {
        chips.push((COUNTERPARTY_KEY, format!("With {n}")));
    }
    if let Some(c) = non_empty(ctx.recall(ITEM_CATEGORY_KEY)) {
        chips.push((ITEM_CATEGORY_KEY, c));
    }
    if let Some((_, n)) = id_name(ctx, TYPE_KEY) {
        chips.push((TYPE_KEY, n));
    }
    for (key, label) in chips {
        let chip = w::button_icon("close", &label, Btn::Tonal);
        chip.set_tooltip_text(Some("Remove this filter"));
        let (ctx, refill) = (ctx.clone(), refill.clone());
        chip.connect_clicked(move |c| {
            ctx.remember(key, "");
            c.set_visible(false);
            refill();
        });
        b.append(&chip);
    }
    b
}

fn money_cell(v: Option<f64>, internal: bool) -> gtk::Widget {
    let l = match v {
        Some(v) => {
            let l = w::line(&signed(v), &["numeric"]);
            if v != 0.0 {
                l.add_css_class(if v > 0.0 { "comms-isk-in" } else { "comms-isk-out" });
            }
            l.set_tooltip_text(Some(&fmt::isk(v)));
            l
        }
        None if internal => {
            let l = w::line("Internal", &["dim"]);
            l.set_tooltip_text(Some("Between your own characters or corporations"));
            l
        }
        None => w::line("", &[]),
    };
    l.set_xalign(1.0);
    l.upcast()
}

fn percent_cell(d: Option<ct::Deal>) -> gtk::Widget {
    let Some(r) = d.and_then(|d| d.ratio()) else { return w::line("", &[]).upcast() };
    let l = w::line(&format!("{:.0}%", r * 100.0), &["numeric"]);
    l.set_xalign(1.0);
    // a price far off the market is worth a second look either way
    if !(0.5..=1.5).contains(&r) {
        l.add_css_class("warning-text");
    }
    l.set_tooltip_text(Some("Price as a share of what the items were worth at the hub when PodStack first saw the contract finished"));
    l.upcast()
}

fn table(ctx: &Ctx, rows: Vec<Rc<Row>>, party: Option<Party>, at_sell: bool, empty: &str) -> gtk::Widget {
    let ctx2 = ctx.clone();
    let ctx3 = ctx.clone();
    let internal = |r: &Rc<Row>| r.flows.iter().any(|f| f.internal);
    Table::new(rows)
        .text("Type", |r| comms::title_case(&r.contract().kind))
        .widget_sorted(
            "Status",
            |r| w::status(&comms::title_case(&r.contract().status), status_kind(comms::contract_status_tone(&r.contract().status))).upcast(),
            |a, b| a.contract().status.cmp(&b.contract().status),
        )
        .widget_sorted(
            "Details",
            |r| {
                let row = w::hbox(6);
                let l = w::line(&comms::contract_details(r.contract()), &[]);
                l.set_hexpand(true);
                row.append(&l);
                for t in r.tags.iter().take(3) {
                    row.append(&w::status(t, w::Status::Neutral));
                }
                if !r.note.is_empty() {
                    let n = w::icon_sized("sticky_note_2", 18);
                    n.add_css_class("dim");
                    n.set_tooltip_text(Some(&r.note));
                    row.append(&n);
                }
                row.upcast()
            },
            |a, b| comms::contract_details(a.contract()).to_lowercase().cmp(&comms::contract_details(b.contract()).to_lowercase()),
        )
        .expand()
        .text("Mine", |r| r.mine.clone())
        .width(200)
        .text("What", |r| r.what())
        .text("From", |r| comms::contract_from(r.contract()).to_string())
        .text("To", |r| comms::contract_to(r.contract()).to_string())
        .num("ISK", |r| comms::contract_value(r.contract()).map(fmt::isk_short).unwrap_or_default(), |r| comms::contract_value(r.contract()).unwrap_or(0.0))
        .widget_sorted("Net", move |r| money_cell(r.net(party), internal(r)), move |a, b| a.net(party).unwrap_or(0.0).total_cmp(&b.net(party).unwrap_or(0.0)))
        .num(
            "Worth",
            move |r| r.deal(at_sell).map(|d| fmt::isk_short(d.worth)).unwrap_or_default(),
            move |r| r.deal(at_sell).map(|d| d.worth).unwrap_or(0.0),
        )
        .widget_sorted(
            "Of market",
            move |r| percent_cell(r.deal(at_sell)),
            move |a, b| {
                let k = |r: &Rc<Row>| r.deal(at_sell).and_then(|d| d.ratio()).unwrap_or(0.0);
                k(a).total_cmp(&k(b))
            },
        )
        .widget_sorted("Date", |r| date_cell(r.contract().last_date()), |a, b| a.contract().last_date().cmp(&b.contract().last_date()))
        .sort_by(11, true)
        .widget("", move |r| {
            let b = w::icon_button("chevron_right", "Contract details");
            let (ctx, c) = (ctx2.clone(), r.contract().clone());
            b.connect_clicked(move |_| open_contract(&ctx, &c));
            b.upcast()
        })
        .on_activate(move |r| open_contract(&ctx3, r.contract()))
        .empty_text(empty)
        .build()
}

/// The rows as CSV, with a header.
fn csv(rows: &[Rc<Row>], at_sell: bool) -> String {
    let mut out = vec![[
        "Contract ID", "Type", "Status", "Availability", "Title", "Details", "Issuer", "Issuer corporation", "On behalf of corporation", "Assignee",
        "Acceptor", "Mine", "What", "Issued", "Accepted", "Completed", "Expires", "Price", "Reward", "Collateral", "Buyout", "Winning bid", "Volume (m3)",
        "From", "To", "Net ISK", "Items worth", "Price of market", "Items", "Tags", "Note",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect::<Vec<_>>()];
    let num = |v: Option<f64>| v.map(|v| format!("{v:.2}")).unwrap_or_default();
    let date = |t: Option<chrono::DateTime<Utc>>| t.map(|t| t.to_rfc3339()).unwrap_or_default();
    for r in rows {
        let c = r.contract();
        let place = |p: &Option<podstack_core::universe::Place>| p.as_ref().map(|p| p.name.clone()).unwrap_or_default();
        let deal = r.deal(at_sell);
        let items = c.items.iter().map(|i| format!("{}{} x{}", if i.is_included { "" } else { "wanted: " }, i.name, i.quantity)).collect::<Vec<_>>().join("; ");
        out.push(vec![
            c.contract_id.to_string(),
            comms::title_case(&c.kind),
            comms::title_case(&c.status),
            comms::title_case(&c.availability),
            c.title.clone(),
            comms::contract_details(c),
            c.issuer_name.clone(),
            c.issuer_corporation_name.clone(),
            if c.for_corporation { "Yes" } else { "No" }.into(),
            c.assignee_name.clone().unwrap_or_default(),
            c.acceptor_name.clone().unwrap_or_default(),
            r.mine.clone(),
            r.what(),
            date(Some(c.date_issued)),
            date(c.date_accepted),
            date(c.date_completed),
            date(Some(c.date_expired)),
            num(c.price),
            num(c.reward),
            num(c.collateral),
            num(c.buyout),
            num((c.kind == "auction").then(|| c.bids.first().map(|b| b.amount)).flatten()),
            num(c.volume),
            place(&c.start_location),
            place(&c.end_location),
            num(r.net(None)),
            num(deal.map(|d| d.worth)),
            deal.and_then(|d| d.ratio()).map(|x| format!("{:.1}%", x * 100.0)).unwrap_or_default(),
            items,
            r.tags.join("; "),
            r.note.clone(),
        ]);
    }
    ct::csv(&out)
}

fn save_csv(ctx: &Ctx, text: String, count: usize) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("CSV files"));
    filter.add_pattern("*.csv");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let d = gtk::FileDialog::new();
    d.set_title("Export contracts");
    d.set_modal(true);
    d.set_initial_name(Some(&format!("contracts-{}.csv", Utc::now().format("%Y-%m-%d"))));
    d.set_filters(Some(&filters));
    let ctx = ctx.clone();
    d.save(Some(&ctx.window()), gio::Cancellable::NONE, move |r| {
        let Ok(file) = r else { return };
        let Some(path) = file.path() else { return };
        match std::fs::write(&path, text.as_bytes()) {
            Ok(()) => ctx.toast(&format!("Exported {count} contract{}", if count == 1 { "" } else { "s" })),
            Err(e) => ctx.toast(&format!("Couldn't save the file: {e}")),
        }
    });
}
