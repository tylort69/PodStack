//! Character tab: the character's planetary colonies (the Planetary Industry page's view for one character).

use crate::ctx::{Ctx, Page};
use crate::pages::industry::scope_failed;
use crate::pages::planets::colonies_view;
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::domains::industry::PLANETS_SCOPE;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("PI", body) };
    if scope_failed(&c, "planets") {
        body.append(&w::scope_notice(PLANETS_SCOPE));
    } else if c.planets.is_none() {
        body.append(&w::empty_state("hourglass_empty", "Colonies not loaded yet", "They'll appear after the next refresh from ESI."));
    } else {
        body.append(&colonies_view(ctx, &[c], false));
    }
    Page::new("PI", body)
}
