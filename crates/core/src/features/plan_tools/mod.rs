//! The skill plan feature's calculations, on top of the stored plans in [`super::plans`]:
//!
//! - [`sim`]: plan simulation ([`sim::Planner`]: prerequisites, per-level times, remaps, notes, reordering), the
//!   combined training queue and merging;
//! - [`order`]: sorting by training time and finding the fastest order;
//! - [`remap`]: optimal attribute remaps (and a second one for plans over a year);
//! - [`accelerator`]: detecting an active cerebral accelerator and modelling how long plans count it;
//! - [`profile`]: plan times under other implant sets and accelerators, and the skillbooks a plan needs;
//! - [`io`]: import and export formats.

pub mod accelerator;
pub mod io;
pub mod order;
pub mod profile;
pub mod remap;
pub mod sim;

pub use sim::{Item, PlanBase, Planner, SkillItem};
