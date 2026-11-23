//! Calculations and stored data behind individual pages. Each module belongs to one feature area.

/// Skill plan storage (shared API, see module docs).
pub mod plans;

// Fleet pages: SP farming, farm profit, accounts, queue health
pub mod farming;
pub mod accounts;
pub mod queue_health;

// Skill plans: plan simulation, import/export, remaps, accelerators, implant profiles, skillbooks
pub mod plan_tools;

// Ship browser and fit planner
pub mod fitting;
// Doctrines: groups, doctrines and their fits, who can fly what (`doctrines`)
pub mod doctrines;
// Doctrine ships owned and missing per character, and what the missing ones cost (`doctrine_inventory`)
pub mod doctrine_inventory;

// Mail bodies, contracts and wallet helpers
pub mod comms;
// Alerts to Discord: webhook, bot channel or bot DM (`discord`)
pub mod discord;
// Contract history, money flows and totals (`contracts`)
pub mod contracts;
// Couriers: active hauls, hauling history, rate cards and quotes, public courier finder (`courier`)
pub mod courier;
// Buyback programs, offers and verdicts (`buyback`); public contracts against the market (`deals`)
pub mod buyback;
pub mod deals;

// Industry and planetary interaction helpers
pub mod industry;

// Loyalty store helper and other character-page calculations
pub mod character_extras;

// Desktop alerts and password-protected backups
pub mod alerts;
pub mod backup;

// Market browser, order competition, trade profits and appraisal (`trade`).
pub mod manufacturing;
pub mod planetary;
pub mod trade;
// new PodStack releases on GitHub
pub mod updates;
