//! The Overview tab.

use super::Tab;
use crate::ctx::{Ctx, Page};

pub fn build(ctx: &Ctx) -> Page {
    super::placeholder(ctx, Tab::Overview)
}
