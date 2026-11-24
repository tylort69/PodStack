//! Doctrines › Stock: every pilot's stock of the doctrine fits it can fly. For each character (by
//! account) and doctrine: how many ships of each fit it should keep (its stock target), how many it has and where,
//! and a shopping list for the rest priced at Jita, for each character and for all of them together, to copy into
//! the game's Multibuy or save as a file. Targets are set in the dialog in `targets`; the calculations are in
//! `features::doctrine_inventory`.

mod targets;

use crate::ctx::{Ctx, Page, Route};
use super::{tab_bar, Tab};
use crate::pages::industry::{char_cell, icon_url, place_cell, scope_failed, type_cell};
use crate::pages::skill_browser::shared as browser;
use crate::widgets::{self as w, Btn, Status, Table};
use gtk::gio;
use gtk::prelude::*;
use podstack_core::domains::assets::SCOPE;
use podstack_core::features::accounts;
use podstack_core::features::contracts::csv;
use podstack_core::features::doctrine_inventory::{self as inv, Basis, CharacterInventory, Cost, Holding, Options, OwnedShip, Purchase, Source, Targets};
use podstack_core::features::doctrines::{self as dc, Readiness};
use podstack_core::{config, fmt};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

const GROUP_KEY: &str = "doctrines.stock.group";
const HULL_ONLY_KEY: &str = "doctrines.stock.hull_only";

const CSS: &str = "
.dsi-account-head { margin-top: 8px; }
.dsi-char-head { padding-bottom: 4px; }
";

thread_local! {
    /// Jita prices fetched this session, by type (only types someone sells).
    static PRICES: RefCell<HashMap<i64, f64>> = RefCell::new(HashMap::new());
    /// Types asked for (priced or not), so a rebuild doesn't ask again for types nobody sells.
    static ASKED: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
    /// Types being fetched right now.
    static PENDING: RefCell<HashSet<i64>> = RefCell::new(HashSet::new());
    /// Whether PODSTACK_STOCK_TARGETS has opened the dialog already.
    static TARGETS_OPENED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn build(ctx: &Ctx) -> Page {
    crate::theme::add_css(CSS);
    let body = w::page_body();
    let doc = dc::load();
    let group = ctx.recall(GROUP_KEY).filter(|g| doc.group(g).is_some());
    let hull_only = ctx.recall(HULL_ONLY_KEY).as_deref() == Some("1");
    body.append(&toolbar(ctx, &doc, group.as_deref(), hull_only));

    if doc.fit_count() == 0 {
        let empty = w::empty_state(
            "military_tech",
            "No doctrines yet",
            "Add your groups' doctrines and their fits, and this shows each pilot's stock of the doctrine ships it can fly, and what to buy to fill it.",
        );
        let go = w::button_icon("add", "Go to Doctrines", Btn::Filled);
        go.set_halign(gtk::Align::Center);
        go.set_margin_top(8);
        let c = ctx.clone();
        go.connect_clicked(move |_| super::open(&c, Tab::Doctrines));
        empty.append(&go);
        body.append(&empty);
        return Page::new("Doctrines", body);
    }
    let chars = ctx.core().characters();
    if chars.is_empty() {
        body.append(&w::empty_state("group_add", "No characters", "Add a character to see its stock of doctrine ships."));
        return Page::new("Doctrines", body);
    }

    // PODSTACK_STOCK_TARGETS=everyone or =<character id> opens the targets dialog once (for UI testing)
    if let Ok(v) = std::env::var("PODSTACK_STOCK_TARGETS") {
        if !TARGETS_OPENED.with(|o| o.replace(true)) {
            let (c, g) = (ctx.clone(), group.clone());
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || targets::open(&c, v.parse().ok(), g.as_deref()));
        }
    }

    let doctrines = dc::doctrines_in(&doc, group.as_deref());
    let opts = Options { hull_only };
    let accounts = accounts::export_all();
    let targets = inv::load_targets();
    let prices = PRICES.with(|p| p.borrow().clone());
    let groups = inv::inventory(&chars, &accounts, &doctrines, &targets, &prices, opts);

    // prices for what's to buy that this session hasn't asked for yet
    let wanted = inv::types_to_price(groups.iter().flat_map(|g| &g.characters).flat_map(|c| &c.to_buy).map(|p| p.fit));
    let ask: Vec<i64> = ASKED.with(|a| wanted.iter().copied().filter(|t| !a.borrow().contains(t)).collect());
    let loading = !ask.is_empty() || PENDING.with(|p| wanted.iter().any(|t| p.borrow().contains(t)));
    if !ask.is_empty() {
        fetch_prices(ctx, &body, ask, false);
    }

    let t = inv::totals(&groups);
    let mut priced_as = Vec::new();
    if t.fit_priced > 0 {
        priced_as.push(format!("{} as full fits", t.fit_priced));
    }
    if t.hull_priced > 0 {
        priced_as.push(format!("{} as hulls", t.hull_priced));
    }
    let cost_tile = w::stat_tile(
        "Cost to fill",
        &if loading { "…".to_string() } else { format!("{} ISK", fmt::isk_short(t.cost)) },
        Some(&if priced_as.is_empty() { "Jita sell".to_string() } else { format!("Jita sell · {}", priced_as.join(", ")) }),
    );
    cost_tile.add_css_class("accent");
    body.append(&w::tile_row(vec![
        w::stat_tile("Stock target", &fmt::int(t.target as i64), Some(&format!("{} flyable fits · {} characters", t.flyable, t.characters))),
        w::stat_tile("In stock", &fmt::int(t.stock as i64), Some(&format!("{} fitted as the doctrine", t.fitted))),
        w::stat_tile("Short", &fmt::int(t.short as i64), Some(&format!("{} ships to buy", t.to_buy))),
        cost_tile,
    ]));

    if loading {
        body.append(&w::banner("hourglass_empty", "Loading Jita prices for the ships to buy…", w::BannerKind::Info));
    } else if t.partly_priced > 0 {
        let mut unpriced: Vec<&str> =
            groups.iter().flat_map(|g| &g.characters).flat_map(|c| &c.to_buy).flat_map(|p| &p.cost.unpriced).map(String::as_str).collect();
        unpriced.sort();
        unpriced.dedup();
        body.append(&w::banner(
            "info",
            &format!(
                "{} fit(s) to buy include items nobody sells at Jita 4-4 ({}): their cost leaves those items out.",
                t.partly_priced,
                unpriced.join(", ")
            ),
            w::BannerKind::Notice,
        ));
    }
    let unloaded = chars.iter().filter(|c| c.skills.is_none()).count();
    if unloaded > 0 {
        body.append(&w::banner(
            "hourglass_empty",
            &format!("{unloaded} character(s) have no skills loaded yet and are left out until they have."),
            w::BannerKind::Info,
        ));
    }
    if doctrines.is_empty() {
        body.append(&w::empty_state("military_tech", "No doctrines in this group", "Pick another group, or all groups."));
        return Page::new("Doctrines", body);
    }

    body.append(&w::text(
        "Each pilot keeps a stock of every doctrine fit it can fly: one of each unless Stock targets says otherwise. Inside \
         a doctrine each ship counts for one fit, but a ship counts for every doctrine it's in, so a hull several \
         doctrines are short of is bought once, as many as the doctrine short of the most. Fits that list modules are \
         bought as the whole fit (hull, modules, charges and drones), the rest as the hull.",
        &["body-small", "dim"],
    ));

    for g in &groups {
        let head = w::hbox(12);
        head.add_css_class("dsi-account-head");
        head.append(&w::icon(if g.account.is_some() { "group" } else { "person_off" }));
        head.append(&w::line(&g.name, &["title-large"]));
        let n = g.characters.len();
        head.append(&w::line(
            &format!("{n} character{} · {} ISK to fill", if n == 1 { "" } else { "s" }, fmt::isk_short(g.cost())),
            &["body-medium", "dim"],
        ));
        body.append(&head);
        for c in &g.characters {
            body.append(&character_card(ctx, c, &targets, group.as_deref(), loading));
        }
    }

    let all: Vec<(&CharacterInventory, &Purchase)> =
        groups.iter().flat_map(|g| &g.characters).flat_map(|c| c.to_buy.iter().map(move |p| (c, p))).collect();
    if !all.is_empty() {
        let rows: Vec<BuyRow> = all.iter().map(|(c, p)| BuyRow::new(c, p)).collect();
        let actions = list_actions(ctx, &all, "all characters");
        body.append(&w::card_with(
            "Shopping list: all characters",
            Some(&format!(
                "{} ships for {} ISK at Jita: what every character needs to reach its stock targets",
                t.to_buy,
                fmt::isk_short(t.cost)
            )),
            &actions,
            &buy_table(rows, true),
        ));
    }
    Page::new("Doctrines", body)
}

fn toolbar(ctx: &Ctx, doc: &dc::DoctrineDoc, group: Option<&str>, hull_only: bool) -> gtk::Widget {
    let bar = w::hbox(12);
    bar.append(&tab_bar(ctx, Tab::Stock));

    let groups: Vec<(String, String)> = doc.all_groups().iter().map(|(p, g)| (g.id.clone(), p.join(" \u{203a} "))).collect();
    let mut names = vec!["All doctrine groups".to_string()];
    names.extend(groups.iter().map(|(_, n)| n.clone()));
    let index = group.and_then(|id| groups.iter().position(|(g, _)| g == id)).map(|i| i as u32 + 1).unwrap_or(0);
    let dd = browser::dropdown(&names, index);
    dd.set_tooltip_text(Some("Only the doctrines of this group and its subgroups"));
    let c = ctx.clone();
    dd.connect_selected_notify(move |d| {
        let id = (d.selected() as usize).checked_sub(1).and_then(|i| groups.get(i)).map(|(g, _)| g.clone());
        c.remember(GROUP_KEY, id.unwrap_or_default());
        c.rebuild();
    });
    bar.append(&dd);

    let chip = w::chip("Count hull-only fits", hull_only);
    chip.set_tooltip_text(Some("Also count fits where the character can fly the hull but not every module yet"));
    let c = ctx.clone();
    chip.connect_toggled(move |b| {
        c.remember(HULL_ONLY_KEY, if b.is_active() { "1" } else { "0" });
        c.rebuild();
    });
    bar.append(&chip);

    let spacer = w::hbox(0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let set = w::button_icon("tune", "Stock targets", Btn::Tonal);
    set.set_tooltip_text(Some("How many ships of each fit to keep: overall, per doctrine and per fit, for everyone or one pilot"));
    let (c, g) = (ctx.clone(), group.map(str::to_string));
    set.connect_clicked(move |_| targets::open(&c, None, g.as_deref()));
    bar.append(&set);
    let refresh = w::icon_button("refresh", "Reload Jita prices");
    let c = ctx.clone();
    let b2 = bar.clone();
    refresh.connect_clicked(move |_| {
        let types: Vec<i64> = ASKED.with(|a| a.borrow().iter().copied().collect());
        if !types.is_empty() {
            fetch_prices(&c, &b2, types, true);
        }
    });
    bar.append(&refresh);
    bar.upcast()
}

/// Fetches Jita prices for `types`, then rebuilds the page if it's still showing.
fn fetch_prices(ctx: &Ctx, anchor: &impl IsA<gtk::Widget>, types: Vec<i64>, force: bool) {
    ASKED.with(|a| a.borrow_mut().extend(types.iter().copied()));
    PENDING.with(|p| p.borrow_mut().extend(types.iter().copied()));
    let core = ctx.core().clone();
    let ctx = ctx.clone();
    let anchor = anchor.as_ref().downgrade();
    let asked = types.clone();
    crate::rt::run(async move { core.market.jita_sell(&core.esi, &asked, force).await.into_iter().collect::<Vec<_>>() }, move |found| {
        let got: HashSet<i64> = found.iter().map(|(t, _)| *t).collect();
        PRICES.with(|p| {
            let mut p = p.borrow_mut();
            for (t, price) in found {
                match price {
                    Some(v) => p.insert(t, v),
                    None => p.remove(&t),
                };
            }
        });
        PENDING.with(|p| p.borrow_mut().retain(|t| !types.contains(t)));
        // types that failed to load are asked for again on a later rebuild
        ASKED.with(|a| a.borrow_mut().retain(|t| got.contains(t) || !types.contains(t)));
        if anchor.upgrade().is_some_and(|w| w.root().is_some()) && ctx.route() == Route::Doctrines {
            ctx.rebuild();
        }
    });
}

fn character_card(ctx: &Ctx, c: &CharacterInventory, targets: &Targets, group: Option<&str>, loading: bool) -> gtk::Widget {
    let card = w::vbox(12);
    card.add_css_class("card");
    let head = w::hbox(12);
    head.add_css_class("dsi-char-head");
    head.append(&w::avatar(&config::character_portrait(c.character_id, 64), 40));
    let titles = w::vbox(2);
    titles.set_hexpand(true);
    titles.set_valign(gtk::Align::Center);
    titles.append(&w::text(&c.name, &["title-medium"]));
    let summary = if !c.assets_loaded {
        format!("Can fly {} doctrine fits in {} doctrines · stock target {} · assets unknown", c.flyable(), c.doctrines_flyable(), c.target())
    } else {
        format!(
            "Can fly {} doctrine fits in {} doctrines · {} of {} in stock · short {}",
            c.flyable(),
            c.doctrines_flyable(),
            c.stock(),
            c.target(),
            c.short()
        )
    };
    titles.append(&w::text(&summary, &["body-medium", "dim"]));
    head.append(&titles);
    if c.assets_loaded && !c.to_buy.is_empty() {
        let cost = w::vbox(0);
        cost.set_valign(gtk::Align::Center);
        let v = w::line(&if loading { "…".into() } else { format!("{} ISK", fmt::isk_short(c.cost())) }, &["title-medium", "numeric"]);
        v.set_xalign(1.0);
        cost.append(&v);
        let s = w::line(&format!("{} to buy{}", c.ships_to_buy(), if c.partly_priced() { ", partly priced" } else { "" }), &["body-small", "dim"]);
        s.set_xalign(1.0);
        cost.append(&s);
        head.append(&cost);
    } else if c.assets_loaded && c.target() > 0 {
        head.append(&w::status("Fully stocked", Status::Good));
    }
    let set = w::icon_button("tune", "This pilot's stock targets");
    set.set_valign(gtk::Align::Center);
    let (cx, id, g) = (ctx.clone(), c.character_id, group.map(str::to_string));
    set.connect_clicked(move |_| targets::open(&cx, Some(id), g.as_deref()));
    head.append(&set);
    card.append(&head);

    if !c.assets_loaded {
        let char = ctx.core().character(c.character_id);
        if char.as_ref().is_some_and(|ch| scope_failed(ch, "assets")) {
            card.append(&w::scope_notice(SCOPE));
        } else {
            card.append(&w::banner("hourglass_empty", "Assets not loaded yet: stock shows after the next refresh.", w::BannerKind::Info));
        }
    }

    let rows: Vec<DoctrineRow> = c.doctrines.iter().map(DoctrineRow::new).collect();
    card.append(&doctrine_table(rows, c.assets_loaded));
    let fits: Vec<FitRow> =
        c.doctrines.iter().flat_map(|d| d.flyable.iter().map(move |l| FitRow::new(d, l, targets.resolve(Some(c.character_id), Some(&d.doctrine.id), Some(&l.fit.id)).1))).collect();
    if !fits.is_empty() {
        let key = format!("doctrines.stock.open.{}", c.character_id);
        // open by default when something's short, so the gaps are in view
        let open = ctx.recall(&key).map(|v| v == "1").unwrap_or(c.assets_loaded && c.short() > 0);
        card.append(&expander(ctx, &format!("Stock by fit ({} fits)", fits.len()), key, open, &fit_table(fits, c.assets_loaded)));
    }
    if c.assets_loaded && !c.to_buy.is_empty() {
        let all: Vec<(&CharacterInventory, &Purchase)> = c.to_buy.iter().map(|p| (c, p)).collect();
        let rows: Vec<BuyRow> = all.iter().map(|(c, p)| BuyRow::new(c, p)).collect();
        let list = w::vbox(8);
        let bar = w::hbox(8);
        for a in list_actions(ctx, &all, &c.name) {
            bar.append(&a);
        }
        list.append(&bar);
        list.append(&buy_table(rows, false));
        let key = format!("doctrines.stock.list.{}", c.character_id);
        let open = ctx.recall(&key).as_deref() == Some("1");
        let title = format!("Shopping list ({} ships, {} ISK)", c.ships_to_buy(), if loading { "…".into() } else { fmt::isk_short(c.cost()) });
        card.append(&expander(ctx, &title, key, open, &list));
    }
    card.upcast()
}

/// An expander whose open state survives rebuilds.
fn expander(ctx: &Ctx, title: &str, key: String, open: bool, child: &impl IsA<gtk::Widget>) -> gtk::Expander {
    let exp = gtk::Expander::new(None);
    exp.set_label_widget(Some(&w::line(title, &["label-large"])));
    exp.set_child(Some(child));
    exp.set_expanded(open);
    let c = ctx.clone();
    exp.connect_expanded_notify(move |e| c.remember(&key, if e.is_expanded() { "1" } else { "0" }));
    exp
}

/// Copy for Multibuy and Save as a file, for a shopping list of `who`.
fn list_actions(ctx: &Ctx, list: &[(&CharacterInventory, &Purchase)], who: &str) -> Vec<gtk::Widget> {
    let items = inv::items(list.iter().map(|(_, p)| *p));
    let text = inv::multibuy(&items);
    let n = items.len();
    let copy = w::button_icon("content_copy", "Copy for Multibuy", Btn::Tonal);
    copy.set_tooltip_text(Some("Every hull, module, charge and drone to buy, as the game's Multibuy window takes them"));
    let c = ctx.clone();
    copy.connect_clicked(move |_| {
        c.window().clipboard().set_text(&text);
        c.toast(&format!("Copied {n} item{} for Multibuy", if n == 1 { "" } else { "s" }));
    });
    let csv_text = list_csv(list);
    let save = w::button_icon("download", "Save", Btn::Text);
    save.set_tooltip_text(Some("Save the shopping list as a CSV file: each character's items to buy, with Jita prices"));
    let c = ctx.clone();
    let name = format!("doctrine-shopping-{}.csv", file_slug(who));
    save.connect_clicked(move |_| save_csv(&c, csv_text.clone(), &name));
    vec![copy.upcast(), save.upcast()]
}

/// The list per character and item: what each character buys, so a list for several still says who needs what.
fn list_csv(list: &[(&CharacterInventory, &Purchase)]) -> String {
    let prices = PRICES.with(|p| p.borrow().clone());
    let mut rows = vec![["Character", "Item", "Quantity", "Jita sell each", "Jita sell total"].map(String::from).to_vec()];
    let mut chars: Vec<&CharacterInventory> = Vec::new();
    for (c, _) in list {
        if !chars.iter().any(|x| x.character_id == c.character_id) {
            chars.push(c);
        }
    }
    for c in chars {
        let mine = list.iter().filter(|(x, _)| x.character_id == c.character_id).map(|(_, p)| *p);
        for i in inv::items(mine) {
            let each = prices.get(&i.type_id).copied();
            rows.push(vec![
                c.name.clone(),
                i.name,
                i.quantity.to_string(),
                each.map(|v| format!("{v:.2}")).unwrap_or_default(),
                each.map(|v| format!("{:.2}", v * i.quantity as f64)).unwrap_or_default(),
            ]);
        }
    }
    csv(&rows)
}

fn file_slug(s: &str) -> String {
    let slug: String = s.chars().map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    slug.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

fn save_csv(ctx: &Ctx, text: String, name: &str) {
    let filter = gtk::FileFilter::new();
    filter.set_name(Some("CSV files"));
    filter.add_pattern("*.csv");
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let d = gtk::FileDialog::new();
    d.set_title("Save shopping list");
    d.set_modal(true);
    d.set_initial_name(Some(name));
    d.set_filters(Some(&filters));
    let ctx = ctx.clone();
    d.save(Some(&ctx.window()), gio::Cancellable::NONE, move |r| {
        let Ok(file) = r else { return };
        let Some(path) = file.path() else { return };
        match std::fs::write(&path, text.as_bytes()) {
            Ok(()) => ctx.toast("Shopping list saved"),
            Err(e) => ctx.toast(&format!("Couldn't save the file: {e}")),
        }
    });
}

#[derive(Clone)]
struct DoctrineRow {
    path: String,
    name: String,
    fits: usize,
    flyable: usize,
    target: u32,
    stock: u32,
    short: u32,
    cost: f64,
}

impl DoctrineRow {
    fn new(d: &inv::DoctrineLine) -> Self {
        DoctrineRow {
            path: d.path.join(" \u{203a} "),
            name: d.doctrine.name.clone(),
            fits: d.fits(),
            flyable: d.flyable.len(),
            target: d.target(),
            stock: d.stock(),
            short: d.short(),
            cost: d.cost(),
        }
    }
}

fn doctrine_table(rows: Vec<DoctrineRow>, assets: bool) -> gtk::Widget {
    let dash = |n: u32| if n == 0 { "—".to_string() } else { fmt::int(n as i64) };
    let t = Table::new(rows)
        .widget_sorted("Doctrine", |r| place_cell(r.name.clone(), Some(r.path.clone())), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .num("Can fly", |r| format!("{} of {}", r.flyable, r.fits), |r| r.flyable as f64)
        .num("Target", move |r| dash(r.target), |r| r.target as f64);
    let t = if assets {
        t.num("In stock", move |r| if r.target == 0 { "—".into() } else { format!("{} of {}", r.stock, r.target) }, |r| r.stock as f64)
            .num("Short", move |r| dash(r.short), |r| r.short as f64)
            .num("Short costs (this doctrine)", |r| if r.short == 0 { "—".into() } else { format!("{} ISK", fmt::isk_short(r.cost)) }, |r| r.cost)
    } else {
        t
    };
    t.natural_height().build()
}

#[derive(Clone)]
struct FitRow {
    doctrine: String,
    fit: String,
    role: String,
    ship_id: i64,
    ship_name: String,
    readiness: Readiness,
    target: u32,
    source: Source,
    fitted: u32,
    hulls: u32,
    short: u32,
    holding: Holding,
    ships: Vec<OwnedShip>,
    cost: Cost,
}

impl FitRow {
    fn new(d: &inv::DoctrineLine, l: &inv::FitLine, source: Source) -> Self {
        FitRow {
            doctrine: d.doctrine.name.clone(),
            fit: l.fit.name.clone(),
            role: l.fit.role.clone(),
            ship_id: l.fit.ship_id,
            ship_name: l.fit.ship_name.clone(),
            readiness: l.readiness,
            target: l.target,
            source,
            fitted: l.fitted,
            hulls: l.hulls,
            short: l.short(),
            holding: l.holding(),
            ships: l.ships.clone(),
            cost: l.cost.clone(),
        }
    }
}

fn holding_pill(r: &FitRow) -> gtk::Widget {
    let label = match r.holding {
        Holding::Short => format!("Short {}", r.short),
        h => h.label().to_string(),
    };
    w::status(&label, match r.holding {
        Holding::Fitted => Status::Good,
        Holding::Hulls => Status::Info,
        Holding::Short => Status::Warn,
        Holding::NotStocked => Status::Neutral,
    })
    .upcast()
}

/// A number over a small line under it.
fn two_lines(top: &str, sub: &str, sub_class: &str) -> gtk::Widget {
    let b = w::vbox(0);
    b.set_valign(gtk::Align::Center);
    b.append(&w::line(top, &["numeric"]));
    b.append(&w::line(sub, &["body-small", sub_class]));
    b.upcast()
}

/// "1.2B ISK" over "3 × full fit" (and what has no price).
fn cost_cell(cost: &Cost, quantity: u32) -> gtk::Widget {
    let mut sub = format!("{quantity} × {}", cost.basis.label().to_lowercase());
    if !cost.complete() {
        sub = format!("{sub} · {} unpriced", cost.unpriced.len());
    }
    let cell = two_lines(&format!("{} ISK", fmt::isk_short(cost.isk * quantity as f64)), &sub, if cost.basis == Basis::Hull { "warning-text" } else { "dim" });
    if !cost.complete() {
        cell.set_tooltip_text(Some(&format!("No Jita sell price: {}", cost.unpriced.join(", "))));
    }
    cell
}

/// Where the ships are: the first place, and how many more there are (all of them in the tooltip).
fn ships_cell(ships: &[OwnedShip]) -> gtk::Widget {
    let place = |s: &OwnedShip| match &s.inside {
        Some(p) => format!("{}, in {p}", s.location),
        None => s.location.clone(),
    };
    let Some(first) = ships.first() else { return w::line("—", &["dim"]).upcast() };
    let mut places: Vec<String> = ships.iter().map(place).collect();
    places.dedup();
    let label = if first.count > 1 { format!("{} × {}", first.count, first.label) } else { first.label.clone() };
    let sub = if places.len() > 1 { format!("{} · +{} more places", places[0], places.len() - 1) } else { places[0].clone() };
    let cell = place_cell(if ships.len() > 1 { format!("{label} +{} more", ships.len() - 1) } else { label }, Some(sub));
    let all: Vec<String> = ships.iter().map(|s| format!("{} × {} — {}", s.count, s.label, place(s))).collect();
    cell.set_tooltip_text(Some(&all.join("\n")));
    cell
}

fn fit_table(rows: Vec<FitRow>, assets: bool) -> gtk::Widget {
    let t = Table::new(rows)
        .widget_sorted(
            "Fit",
            |r| {
                let pill = (r.readiness == Readiness::HullOnly).then(|| w::status("Hull skills only", Status::Neutral));
                let sub = if r.role.is_empty() { format!("{} · {}", r.ship_name, r.doctrine) } else { format!("{} · {} · {}", r.ship_name, r.role, r.doctrine) };
                type_cell(&icon_url(r.ship_id, &r.ship_name, false), &r.fit, Some(&sub), pill)
            },
            |a, b| a.doctrine.cmp(&b.doctrine).then(a.fit.cmp(&b.fit)),
        )
        .expand()
        .widget_sorted("Target", |r| two_lines(&r.target.to_string(), &targets::source_label(r.source), "dim"), |a, b| a.target.cmp(&b.target));
    if !assets {
        return t.natural_height().build();
    }
    t.widget_sorted(
        "In stock",
        |r| {
            let mut parts = Vec::new();
            if r.fitted > 0 {
                parts.push(format!("{} fitted", r.fitted));
            }
            if r.hulls > 0 {
                parts.push(format!("{} hull{}", r.hulls, if r.hulls == 1 { "" } else { "s" }));
            }
            two_lines(&(r.fitted + r.hulls).to_string(), &if parts.is_empty() { "none".into() } else { parts.join(", ") }, "dim")
        },
        |a, b| (a.fitted + a.hulls).cmp(&(b.fitted + b.hulls)),
    )
    .widget_sorted("Status", holding_pill, |a, b| a.holding.cmp(&b.holding))
    .widget_sorted("Ships and where", |r| ships_cell(&r.ships), |a, b| a.ships.first().map(|s| &s.location).cmp(&b.ships.first().map(|s| &s.location)))
    .expand()
    .widget_sorted(
        "Short costs",
        |r| if r.short == 0 { w::line("—", &["dim"]).upcast() } else { cost_cell(&r.cost, r.short) },
        |a, b| (a.cost.isk * a.short as f64).total_cmp(&(b.cost.isk * b.short as f64)),
    )
    .natural_height()
    .build()
}

#[derive(Clone)]
struct BuyRow {
    character_id: i64,
    character: String,
    doctrine: String,
    wanted_by: usize,
    fit: String,
    ship_id: i64,
    ship_name: String,
    quantity: u32,
    cost: Cost,
}

impl BuyRow {
    fn new(c: &CharacterInventory, p: &Purchase) -> Self {
        BuyRow {
            character_id: c.character_id,
            character: c.name.clone(),
            doctrine: p.doctrine.name.clone(),
            wanted_by: p.wanted_by,
            fit: p.fit.name.clone(),
            ship_id: p.fit.ship_id,
            ship_name: p.fit.ship_name.clone(),
            quantity: p.quantity,
            cost: p.cost.clone(),
        }
    }
}

fn buy_table(rows: Vec<BuyRow>, with_character: bool) -> gtk::Widget {
    let t = Table::new(rows);
    let t = if with_character {
        t.widget_sorted("Character", |r| char_cell(r.character_id, &r.character), |a, b| a.character.cmp(&b.character))
    } else {
        t
    };
    let cost_col = if with_character { 4 } else { 3 };
    t.widget_sorted(
        "Fit",
        |r| type_cell(&icon_url(r.ship_id, &r.ship_name, false), &r.fit, Some(&r.ship_name), None),
        |a, b| a.fit.cmp(&b.fit),
    )
    .expand()
    .text("Doctrine", |r| if r.wanted_by > 1 { format!("{} (+{} more)", r.doctrine, r.wanted_by - 1) } else { r.doctrine.clone() })
    .num("Quantity", |r| fmt::int(r.quantity as i64), |r| r.quantity as f64)
    .widget_sorted("Cost", |r| cost_cell(&r.cost, r.quantity), |a, b| (a.cost.isk * a.quantity as f64).total_cmp(&(b.cost.isk * b.quantity as f64)))
    .sort_by(cost_col, true)
    .max_height(480)
    .build()
}
