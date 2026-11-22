//! Wallet journal, wallet transactions and market orders.
//!
//! STUB: filled in by its feature work. Keep the type names below (Character's fields use them).

use super::DomainSpec;
use serde::{Deserialize, Serialize};

pub const DOMAINS: &[DomainSpec] = &[];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JournalData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TransactionsData {}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MarketOrdersData {}
