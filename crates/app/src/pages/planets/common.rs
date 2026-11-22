//! Pieces the Planetary Industry tabs share: PI settings (customs tax, alert thresholds, price hub), loading hub
//! prices for commodities off the GTK thread, tier badges and commodity cells.

#![allow(dead_code)] // shared by the PI tabs, which use different parts

use crate::ctx::Ctx;
use crate::pages::industry::form::{fields, labelled, number_entry, parse_amount, percent_text};
use crate::rt;
use crate::widgets::{self as w, Btn};
use gtk::prelude::*;
use podstack_core::features::planetary::{self as pl, PiSettings};
use podstack_core::market::BestPrices;
use podstack_core::pi::{Tier, PI};
use podstack_core::sde::TRADE_HUBS;
use std::collections::HashMap;

pub const CSS: &str = "
.pi-tier { border-radius: 6px; padding: 1px 6px; font-size: 11px; font-weight: 600; }
.pi-tier-0 { background-color: @surface_container_highest; color: @on_surface_variant; }
.pi-tier-1 { background-color: @primary_container; color: @on_primary_container; }
.pi-tier-2 { background-color: @secondary_container; color: @on_secondary_container; }
.pi-tier-3 { background-color: @tertiary_container; color: @on_tertiary_container; }
.pi-tier-4 { background-color: @error_container; color: @on_error_container; }
.stat-tile .stat-value.success-text { color: @success; }
.stat-tile .stat-value.error-text { color: @error; }
";

/// "P0" … "P4" as a colored badge.
pub fn tier_badge(tier: Tier) -> gtk::Label {
    let l = gtk::Label::new(Some(&format!("P{tier}")));
    l.add_css_class("pi-tier");
    l.add_css_class(&format!("pi-tier-{}", tier.min(4)));
    l.set_valign(gtk::Align::Center);
    l
}

pub fn icon_url(type_id: i64) -> String {
    format!("https://images.evetech.net/types/{type_id}/icon?size=32")
}

/// Icon, name and tier badge of a commodity.
pub fn commodity_cell(type_id: i64) -> gtk::Widget {
    let row = w::hbox(8);
    row.append(&w::eve_image(&icon_url(type_id), 24));
    let name = gtk::Label::new(Some(&PI.name(type_id)));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&name);
    if let Some(t) = PI.tier(type_id) {
        row.append(&tier_badge(t));
    }
    row.upcast()
}

pub fn settings() -> PiSettings {
    pl::load().settings
}

/// The hub prices come from: (name, station, system, region).
pub fn hub(s: &PiSettings) -> (&'static str, i64, i64, i64) {
    s.price_hub.as_deref().and_then(|n| TRADE_HUBS.iter().find(|h| h.0 == n)).copied().unwrap_or(TRADE_HUBS[0])
}

/// Best buy and sell orders at the hub for commodities.
#[derive(Clone, Default)]
pub struct Prices {
    pub hub: HashMap<i64, BestPrices>,
    pub hub_name: String,
}

impl Prices {
    /// Lowest sell order: what buying costs and what a sell order would get.
    pub fn sell(&self) -> HashMap<i64, f64> {
        self.hub.iter().filter_map(|(t, p)| p.sell.map(|v| (*t, v))).collect()
    }
    /// Highest buy order: what selling right away gets.
    pub fn buy(&self) -> HashMap<i64, f64> {
        self.hub.iter().filter_map(|(t, p)| p.buy.map(|v| (*t, v))).collect()
    }
}

/// Loads hub prices for `types` (every commodity when empty) off the GTK thread.
pub fn load_prices(ctx: &Ctx, types: Vec<i64>, force: bool, done: impl FnOnce(Prices) + 'static) {
    let core = ctx.core().clone();
    let (name, station, _, region) = hub(&settings());
    let mut types = if types.is_empty() { PI.commodities.keys().copied().collect() } else { types };
    types.sort();
    types.dedup();
    rt::run(
        async move {
            let hub = core.market.best_prices(&core.esi, region, Some(station), &types, force).await;
            Prices { hub, hub_name: name.to_string() }
        },
        done,
    )
}

/// The gear button that opens the PI settings.
pub fn settings_button(ctx: &Ctx) -> gtk::Button {
    let b = w::icon_button("settings", "PI settings");
    let ctx = ctx.clone();
    b.connect_clicked(move |_| settings_dialog(&ctx));
    b
}

fn settings_dialog(ctx: &Ctx) {
    let s = settings();
    let (win, content, actions) = w::dialog(ctx, "Planetary industry settings", 520);
    let tax = number_entry(&percent_text(s.customs_tax), 7);
    let warn = number_entry(&format!("{}", s.warning_hours), 6);
    let crit = number_entry(&format!("{}", s.critical_hours), 6);
    let swarn = number_entry(&percent_text(s.storage_warning), 6);
    let scrit = number_entry(&percent_text(s.storage_critical), 6);
    let hubs: Vec<&str> = TRADE_HUBS.iter().map(|h| h.0).collect();
    let hub_dd = w::dropdown(&hubs, hubs.iter().position(|h| *h == hub(&s).0).unwrap_or(0) as u32);
    content.append(&fields(vec![labelled("Customs office tax %", &tax).upcast(), labelled("Prices from", &hub_dd).upcast()]));
    content.append(&fields(vec![labelled("Extractor warning (hours)", &warn).upcast(), labelled("Critical (hours)", &crit).upcast()]));
    content.append(&fields(vec![labelled("Storage warning %", &swarn).upcast(), labelled("Critical %", &scrit).upcast()]));
    let note = w::line("NPC customs offices in high-sec charge 10%; player-owned offices set their own rate.", &["body-small", "dim"]);
    note.set_wrap(true);
    note.set_xalign(0.0);
    content.append(&note);
    let cancel = w::button("Cancel", Btn::Text);
    let save = w::button("Save", Btn::Filled);
    {
        let win = win.clone();
        cancel.connect_clicked(move |_| win.close());
    }
    {
        let (win, ctx) = (win.clone(), ctx.clone());
        save.connect_clicked(move |_| {
            let mut doc = pl::load();
            let old = doc.settings.clone();
            let pct = |e: &gtk::Entry, o: f64| parse_amount(&e.text()).map(|v| (v / 100.0).clamp(0.0, 1.0)).unwrap_or(o);
            let num = |e: &gtk::Entry, o: f64| parse_amount(&e.text()).map(|v| v.max(0.0)).unwrap_or(o);
            doc.settings = PiSettings {
                customs_tax: pct(&tax, old.customs_tax),
                warning_hours: num(&warn, old.warning_hours),
                critical_hours: num(&crit, old.critical_hours),
                storage_warning: pct(&swarn, old.storage_warning),
                storage_critical: pct(&scrit, old.storage_critical),
                price_hub: TRADE_HUBS.get(hub_dd.selected() as usize).map(|h| h.0.to_string()),
            };
            pl::save(ctx.core(), &doc);
            win.close();
            ctx.rebuild();
        });
    }
    actions.append(&cancel);
    actions.append(&save);
    win.present();
}
