//! Planetary Industry.
//!
//! STUB: replaced by its feature work. Keep the public signatures.

use crate::ctx::{Ctx, Page};

pub fn build(_ctx: &Ctx) -> Page {
    Page::todo("Planetary Industry")
}

/// Count shown on the navigation drawer item (None hides the badge).
pub fn nav_badge(_core: &podstack_core::Core) -> Option<u32> {
    None
}
