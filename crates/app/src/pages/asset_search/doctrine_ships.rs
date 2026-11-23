//! Asset Search › Doctrine ships: for every character (by account) and doctrine, the fits it can fly, which of those
//! it owns a ship for and where that ship is, and what the missing ones cost at Jita, up to one total for buying
//! every doctrine ship each character can fly. The calculations are in `features::doctrine_inventory`.

use crate::ctx::{Ctx, Page, Route};
use crate::pages::doctrines::{self, Tab};
use crate::pages::industry::{char_cell, icon_url, place_cell, scope_failed, type_cell};
use crate::pages::skill_browser::shared as browser;
use crate::widgets::{self as w, Btn, Status, Table};
use gtk::prelude::*;
use podstack_core::domains::assets::SCOPE;
use podstack_core::features::accounts;
use podstack_core::features::doctrine_inventory::{self as inv, Basis, CharacterInventory, Cost, Holding, Options};
use podstack_core::features::doctrines::{self as dc, Readiness};
use podstack_core::{config, fmt};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

const GROUP_KEY: &str = "asset_search.doctrines.group";
const HULL_ONLY_KEY: &str = "asset_search.doctrines.hull_only";

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
            "Add your groups' doctrines and their fits, and this shows which doctrine ships each character can fly, owns and still needs to buy.",
        );
        let go = w::button_icon("add", "Go to Doctrines", Btn::Filled);
        go.set_halign(gtk::Align::Center);
        go.set_margin_top(8);
        let c = ctx.clone();
        go.connect_clicked(move |_| doctrines::open(&c, Tab::Doctrines));
        empty.append(&go);
        body.append(&empty);
        return Page::new("Asset Search", body);
    }
    let chars = ctx.core().characters();
    if chars.is_empty() {
        body.append(&w::empty_state("group_add", "No characters", "Add a character to see which doctrine ships it owns."));
        return Page::new("Asset Search", body);
    }

    let doctrines = dc::doctrines_in(&doc, group.as_deref());
    let opts = Options { hull_only };
    let accounts = accounts::export_all();
    let prices = PRICES.with(|p| p.borrow().clone());
    let groups = inv::inventory(&chars, &accounts, &doctrines, &prices, opts);

    // prices for what's missing that this session hasn't asked for yet
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
        "Cost to complete",
        &if loading { "…".to_string() } else { format!("{} ISK", fmt::isk_short(t.cost)) },
        Some(&if priced_as.is_empty() { "Jita sell".to_string() } else { format!("Jita sell · {}", priced_as.join(", ")) }),
    );
    cost_tile.add_css_class("accent");
    body.append(&w::tile_row(vec![
        w::stat_tile("Flyable fits", &fmt::int(t.flyable as i64), Some(&format!("{} characters with assets", t.characters))),
        w::stat_tile("Owned", &fmt::int(t.owned as i64), Some(&format!("{} fitted as the doctrine", t.fitted))),
        w::stat_tile("Missing", &fmt::int(t.missing as i64), Some(&format!("{} ships to buy", groups.iter().flat_map(|g| &g.characters).map(|c| c.to_buy.len()).sum::<usize>()))),
        cost_tile,
    ]));

    if loading {
        body.append(&w::banner("hourglass_empty", "Loading Jita prices for the missing ships…", w::BannerKind::Info));
    } else if t.partly_priced > 0 {
        let mut unpriced: Vec<&str> =
            groups.iter().flat_map(|g| &g.characters).flat_map(|c| &c.to_buy).flat_map(|p| &p.cost.unpriced).map(String::as_str).collect();
        unpriced.sort();
        unpriced.dedup();
        body.append(&w::banner(
            "info",
            &format!(
                "{} ship(s) include items nobody sells at Jita 4-4 ({}): their cost leaves those items out.",
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
        return Page::new("Asset Search", body);
    }

    body.append(&w::text(
        "A doctrine's fits each need their own ship, but one ship counts for every doctrine it's in: a hull several doctrines \
         are missing is bought once, as many as the doctrine needing the most. Missing fits that list modules are priced as \
         the whole fit (hull, modules, charges and drones), the rest as the hull.",
        &["body-small", "dim"],
    ));

    for g in &groups {
        let head = w::hbox(12);
        head.add_css_class("dsi-account-head");
        head.append(&w::icon(if g.account.is_some() { "group" } else { "person_off" }));
        head.append(&w::line(&g.name, &["title-large"]));
        let n = g.characters.len();
        head.append(&w::line(
            &format!("{n} character{} · {} ISK to complete", if n == 1 { "" } else { "s" }, fmt::isk_short(g.cost())),
            &["body-medium", "dim"],
        ));
        body.append(&head);
        for c in &g.characters {
            body.append(&character_card(ctx, c, loading));
        }
    }

    let buys: Vec<BuyRow> = groups
        .iter()
        .flat_map(|g| &g.characters)
        .flat_map(|c| c.to_buy.iter().map(move |p| BuyRow::new(c, p)))
        .collect();
    if !buys.is_empty() {
        body.append(&w::card_with(
            "Shopping list",
            Some(&format!(
                "{} ships for {} ISK at Jita: everything needed for every character to own every doctrine ship it can fly",
                buys.len(),
                fmt::isk_short(t.cost)
            )),
            &[],
            &buy_table(buys),
        ));
    }
    Page::new("Asset Search", body)
}

fn toolbar(ctx: &Ctx, doc: &dc::DoctrineDoc, group: Option<&str>, hull_only: bool) -> gtk::Widget {
    let bar = w::hbox(12);
    bar.append(&super::view_switch(ctx, 1));

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
        if anchor.upgrade().is_some_and(|w| w.root().is_some()) && ctx.route() == Route::AssetSearch {
            ctx.rebuild();
        }
    });
}

fn character_card(ctx: &Ctx, c: &CharacterInventory, loading: bool) -> gtk::Widget {
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
        format!("Can fly {} doctrine fits in {} doctrines · assets unknown", c.flyable(), c.doctrines_flyable())
    } else {
        format!(
            "Can fly {} doctrine fits in {} doctrines · owns {} · missing {}",
            c.flyable(),
            c.doctrines_flyable(),
            c.owned(),
            c.missing()
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
        let s = w::line(&format!("{} to buy{}", c.to_buy.len(), if c.partly_priced() { ", partly priced" } else { "" }), &["body-small", "dim"]);
        s.set_xalign(1.0);
        cost.append(&s);
        head.append(&cost);
    } else if c.assets_loaded && c.flyable() > 0 {
        head.append(&w::status("Owns every flyable fit", Status::Good));
    }
    card.append(&head);

    if !c.assets_loaded {
        let char = ctx.core().character(c.character_id);
        if char.as_ref().is_some_and(|ch| scope_failed(ch, "assets")) {
            card.append(&w::scope_notice(SCOPE));
        } else {
            card.append(&w::banner("hourglass_empty", "Assets not loaded yet: ownership shows after the next refresh.", w::BannerKind::Info));
        }
    }

    let rows: Vec<DoctrineRow> = c.doctrines.iter().map(DoctrineRow::new).collect();
    card.append(&doctrine_table(rows, c.assets_loaded));
    let fits: Vec<FitRow> = c.doctrines.iter().flat_map(|d| d.flyable.iter().map(move |l| FitRow::new(d, l))).collect();
    if !fits.is_empty() {
        let exp = gtk::Expander::new(None);
        exp.set_label_widget(Some(&w::line(&format!("{} flyable fits", fits.len()), &["label-large"])));
        exp.set_child(Some(&fit_table(fits, c.assets_loaded)));
        // open by default when there's something to buy, so the gaps are in view
        let key = format!("asset_search.doctrines.open.{}", c.character_id);
        exp.set_expanded(ctx.recall(&key).map(|v| v == "1").unwrap_or(c.assets_loaded && c.missing() > 0));
        let c2 = ctx.clone();
        exp.connect_expanded_notify(move |e| c2.remember(&key, if e.is_expanded() { "1" } else { "0" }));
        card.append(&exp);
    }
    card.upcast()
}

#[derive(Clone)]
struct DoctrineRow {
    path: String,
    name: String,
    fits: usize,
    flyable: usize,
    owned: usize,
    missing: usize,
    cost: f64,
}

impl DoctrineRow {
    fn new(d: &inv::DoctrineLine) -> Self {
        DoctrineRow {
            path: d.path.join(" \u{203a} "),
            name: d.doctrine.name.clone(),
            fits: d.fits(),
            flyable: d.flyable.len(),
            owned: d.owned(),
            missing: d.missing().count(),
            cost: d.cost(),
        }
    }
}

fn doctrine_table(rows: Vec<DoctrineRow>, assets: bool) -> gtk::Widget {
    let t = Table::new(rows)
        .widget_sorted("Doctrine", |r| place_cell(r.name.clone(), Some(r.path.clone())), |a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .expand()
        .num("Can fly", |r| format!("{} of {}", r.flyable, r.fits), |r| r.flyable as f64);
    let t = if assets {
        t.num("Owns", |r| if r.flyable == 0 { "—".into() } else { format!("{} of {}", r.owned, r.flyable) }, |r| r.owned as f64)
            .num("Missing", |r| if r.missing == 0 { "—".into() } else { fmt::int(r.missing as i64) }, |r| r.missing as f64)
            .num("To buy (this doctrine)", |r| if r.missing == 0 { "—".into() } else { format!("{} ISK", fmt::isk_short(r.cost)) }, |r| r.cost)
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
    holding: Holding,
    ship: Option<String>,
    location: Option<String>,
    inside: Option<String>,
    cost: Option<Cost>,
}

impl FitRow {
    fn new(d: &inv::DoctrineLine, l: &inv::FitLine) -> Self {
        FitRow {
            doctrine: d.doctrine.name.clone(),
            fit: l.fit.name.clone(),
            role: l.fit.role.clone(),
            ship_id: l.fit.ship_id,
            ship_name: l.fit.ship_name.clone(),
            readiness: l.readiness,
            holding: l.holding,
            ship: l.ship.as_ref().map(|s| s.label.clone()),
            location: l.ship.as_ref().map(|s| s.location.clone()),
            inside: l.ship.as_ref().and_then(|s| s.inside.clone()),
            cost: l.cost.clone(),
        }
    }
}

fn holding_pill(h: Holding) -> gtk::Widget {
    w::status(h.label(), match h {
        Holding::Fitted => Status::Good,
        Holding::Hull => Status::Info,
        Holding::Missing => Status::Warn,
    })
    .upcast()
}

/// "1.2B ISK" over "Full fit" or "Hull only" (and what has no price).
fn cost_cell(cost: &Cost) -> gtk::Widget {
    let b = w::vbox(0);
    b.set_valign(gtk::Align::Center);
    b.append(&w::line(&format!("{} ISK", fmt::isk_short(cost.isk)), &["numeric"]));
    let mut sub = cost.basis.label().to_string();
    if !cost.complete() {
        sub = format!("{sub} · {} unpriced", cost.unpriced.len());
    }
    let s = w::line(&sub, &["body-small", if cost.basis == Basis::Hull { "warning-text" } else { "dim" }]);
    if !cost.complete() {
        s.set_tooltip_text(Some(&format!("No Jita sell price: {}", cost.unpriced.join(", "))));
    }
    b.append(&s);
    b.upcast()
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
        .expand();
    if !assets {
        return t.natural_height().build();
    }
    t.widget_sorted("Status", |r| holding_pill(r.holding), |a, b| a.holding.cmp(&b.holding))
        .widget_sorted(
            "Ship and where",
            |r| match (&r.ship, &r.location) {
                (Some(s), Some(l)) => place_cell(s.clone(), Some(match &r.inside {
                    Some(p) => format!("{l}, in {p}"),
                    None => l.clone(),
                })),
                _ => w::line("—", &["dim"]).upcast(),
            },
            |a, b| a.location.cmp(&b.location),
        )
        .expand()
        .widget_sorted(
            "Cost to buy",
            |r| match &r.cost {
                Some(c) => cost_cell(c),
                None => w::line("—", &["dim"]).upcast(),
            },
            |a, b| a.cost.as_ref().map(|c| c.isk).unwrap_or(-1.0).total_cmp(&b.cost.as_ref().map(|c| c.isk).unwrap_or(-1.0)),
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
    cost: Cost,
}

impl BuyRow {
    fn new(c: &CharacterInventory, p: &inv::Purchase) -> Self {
        BuyRow {
            character_id: c.character_id,
            character: c.name.clone(),
            doctrine: p.doctrine.name.clone(),
            wanted_by: p.wanted_by,
            fit: p.fit.name.clone(),
            ship_id: p.fit.ship_id,
            ship_name: p.fit.ship_name.clone(),
            cost: p.cost.clone(),
        }
    }
}

fn buy_table(rows: Vec<BuyRow>) -> gtk::Widget {
    Table::new(rows)
        .widget_sorted("Character", |r| char_cell(r.character_id, &r.character), |a, b| a.character.cmp(&b.character))
        .widget_sorted(
            "Fit",
            |r| type_cell(&icon_url(r.ship_id, &r.ship_name, false), &r.fit, Some(&r.ship_name), None),
            |a, b| a.fit.cmp(&b.fit),
        )
        .expand()
        .text("Doctrine", |r| if r.wanted_by > 1 { format!("{} (+{} more)", r.doctrine, r.wanted_by - 1) } else { r.doctrine.clone() })
        .widget_sorted("Cost", |r| cost_cell(&r.cost), |a, b| a.cost.isk.total_cmp(&b.cost.isk))
        .sort_by(3, true)
        .max_height(480)
        .build()
}
