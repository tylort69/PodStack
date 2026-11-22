//! Character tab: the character's unfinished industry jobs (the Industry Jobs page's view for one character).

use crate::ctx::{Ctx, Page};
use crate::pages::industry::{jobs_view, scope_failed};
use crate::widgets as w;
use gtk::prelude::*;
use podstack_core::domains::industry::JOBS_SCOPE;

pub fn build(ctx: &Ctx, id: i64) -> Page {
    let body = w::vbox(16);
    let Some(c) = ctx.core().character(id) else { return Page::new("Industry", body) };
    if scope_failed(&c, "industry_jobs") {
        body.append(&w::scope_notice(JOBS_SCOPE));
        return Page::new("Industry", body);
    }
    if c.industry.is_none() {
        body.append(&w::empty_state("hourglass_empty", "Industry jobs not loaded yet", "They'll appear after the next refresh from ESI."));
        return Page::new("Industry", body);
    }
    body.set_vexpand(true);
    body.append(&jobs_view(ctx, &[c], false, &format!("industry.{id}")));
    Page::new("Industry", body).no_scroll()
}
