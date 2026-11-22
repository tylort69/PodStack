//! Loyalty points, standings, research agents, EVE notifications, calendar, killmails, medals, contacts and factional warfare.
//!
//! STUB: filled in by its feature work. Keep the type names below (Character's fields use them).

use super::DomainSpec;
use serde::{Deserialize, Serialize};

pub const DOMAINS: &[DomainSpec] = &[];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LoyaltyData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StandingsData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ResearchData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct NotificationsData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CalendarData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct KillmailsData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MedalsData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ContactsData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FwData {}
