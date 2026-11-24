//! Page builders, one module per destination. `build` dispatches a route to its page.

pub mod about;
pub mod asset_search;
pub mod character;
pub mod contracts;
pub mod doctrines;
pub mod fit_planner;
pub mod industry;
pub mod market;
pub mod market_orders;
pub mod overview;
pub mod planets;
pub mod queue_health;
pub mod settings;
pub mod ship_browser;
pub mod skill_browser;
pub mod sp_farming;
pub mod wallet;

use crate::ctx::{Ctx, Page, Route};

pub fn build(ctx: &Ctx, route: Route) -> Page {
    match route {
        Route::Overview => overview::build(ctx),
        Route::SkillBrowser => skill_browser::build(ctx),
        Route::ShipBrowser => ship_browser::build(ctx),
        Route::QueueHealth => queue_health::build(ctx),
        Route::SpFarming => sp_farming::build(ctx),
        Route::Contracts => contracts::build(ctx),
        Route::Doctrines => doctrines::build(ctx),
        Route::AssetSearch => asset_search::build(ctx),
        Route::Market => market::build(ctx),
        Route::Wallet => wallet::build(ctx),
        Route::Industry => industry::build(ctx),
        Route::Planets => planets::build(ctx),
        Route::Settings => settings::build(ctx),
        Route::About => about::build(ctx),
        Route::Character(id, tab) => character::build(ctx, id, tab),
    }
}

/// Navigation drawer badge for a destination.
pub fn nav_badge(core: &podstack_core::Core, route: Route) -> Option<u32> {
    match route {
        Route::QueueHealth => queue_health::nav_badge(core),
        Route::Market => market::nav_badge(core),
        Route::Industry => industry::nav_badge(core),
        Route::Planets => planets::nav_badge(core),
        Route::Contracts => contracts::overview::nav_badge(core),
        _ => None,
    }
    .filter(|n| *n > 0)
}
