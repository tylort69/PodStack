//! Fit planner: paste an EFT/in-game fitting and see the skills it needs.
//!
//! STUB: replaced by its feature work. Keep the public signatures (the Plans tab embeds the panel).

use crate::ctx::Ctx;
use std::rc::Rc;

/// The fit planner panel for a character. `add_to_plan` is called with the (skill, level) list the user chose to
/// add to the open plan.
pub fn panel(_ctx: &Ctx, _character_id: i64, _add_to_plan: Rc<dyn Fn(Vec<(i64, u8)>)>) -> gtk::Widget {
    crate::widgets::empty_state("construction", "Fit planner", "Not built yet.").into()
}
