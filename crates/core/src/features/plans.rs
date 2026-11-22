//! Skill plans per character, stored in `skill_plans.json` in the data directory.
//!
//! This module holds the stored data and the basic operations other pages use (the ship browser's "add to plan",
//! queue health). The skill plan feature builds its editing and simulation on top; keep these signatures stable.

use crate::store;
use crate::training::with_prerequisites;
use crate::{Core, Event};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::LazyLock;

pub const DOC: &str = "skill_plans";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PlanEntry {
    Skill {
        skill_id: i64,
        level: u8,
    },
    /// Remap attributes here: target values for intelligence, memory, perception, willpower, charisma.
    Remap {
        attributes: BTreeMap<String, i32>,
        #[serde(default)]
        implants: Option<serde_json::Value>,
    },
    Note {
        text: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillPlan {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub queue: Vec<PlanEntry>,
    #[serde(default = "Utc::now")]
    pub last_saved: DateTime<Utc>,
}

fn yes() -> bool {
    true
}

/// character id -> plan id -> plan
pub type PlanDoc = BTreeMap<i64, BTreeMap<String, SkillPlan>>;

static PLANS: LazyLock<Mutex<PlanDoc>> = LazyLock::new(|| Mutex::new(store::load_doc(DOC)));

fn save(doc: &PlanDoc) {
    store::save_doc(DOC, doc);
}

fn changed(core: &Core) {
    core.emit(Event::Doc(DOC.into()));
}

/// The character's plans, highest priority (lowest number) first.
pub fn plans_for(character_id: i64) -> Vec<SkillPlan> {
    let doc = PLANS.lock();
    let mut v: Vec<SkillPlan> = doc.get(&character_id).map(|m| m.values().cloned().collect()).unwrap_or_default();
    v.sort_by_key(|p| p.priority);
    v
}

pub fn get(character_id: i64, plan_id: &str) -> Option<SkillPlan> {
    PLANS.lock().get(&character_id).and_then(|m| m.get(plan_id)).cloned()
}

/// Creates an empty plan (lowest priority, enabled) and returns its id.
pub fn create(core: &Core, character_id: i64, name: &str) -> String {
    let id = format!("{:x}", Utc::now().timestamp_nanos_opt().unwrap_or_default());
    let mut doc = PLANS.lock();
    let plans = doc.entry(character_id).or_default();
    let priority = plans.values().map(|p| p.priority).max().unwrap_or(0) + 1;
    plans.insert(id.clone(), SkillPlan { id: id.clone(), name: name.to_string(), priority, enabled: true, queue: vec![], last_saved: Utc::now() });
    save(&doc);
    drop(doc);
    changed(core);
    id
}

/// Saves a plan (insert or replace).
pub fn save_plan(core: &Core, character_id: i64, mut plan: SkillPlan) {
    plan.last_saved = Utc::now();
    let mut doc = PLANS.lock();
    doc.entry(character_id).or_default().insert(plan.id.clone(), plan);
    save(&doc);
    drop(doc);
    changed(core);
}

pub fn delete(core: &Core, character_id: i64, plan_id: &str) {
    let mut doc = PLANS.lock();
    if let Some(m) = doc.get_mut(&character_id) {
        m.remove(plan_id);
    }
    save(&doc);
    drop(doc);
    changed(core);
}

pub fn delete_all_for(core: &Core, character_id: i64) {
    let mut doc = PLANS.lock();
    doc.remove(&character_id);
    save(&doc);
    drop(doc);
    changed(core);
}

/// Sets priorities from an ordered list of plan ids.
pub fn set_order(core: &Core, character_id: i64, plan_ids: &[String]) {
    let mut doc = PLANS.lock();
    if let Some(m) = doc.get_mut(&character_id) {
        for (i, id) in plan_ids.iter().enumerate() {
            if let Some(p) = m.get_mut(id) {
                p.priority = i as i32 + 1;
            }
        }
    }
    save(&doc);
    drop(doc);
    changed(core);
}

pub fn set_enabled(core: &Core, character_id: i64, plan_id: &str, enabled: bool) {
    let mut doc = PLANS.lock();
    if let Some(p) = doc.get_mut(&character_id).and_then(|m| m.get_mut(plan_id)) {
        p.enabled = enabled;
    }
    save(&doc);
    drop(doc);
    changed(core);
}

/// Appends skills (with prerequisites, prerequisites first) to a plan, skipping levels it already has.
/// Returns how many levels were added.
pub fn add_skills(core: &Core, character_id: i64, plan_id: &str, skills: &[(i64, u8)]) -> usize {
    let Some(mut plan) = get(character_id, plan_id) else { return 0 };
    let mut added = 0;
    for &(id, level) in skills {
        for (sid, lvl) in with_prerequisites(id, level) {
            for l in 1..=lvl {
                let exists = plan.queue.iter().any(|e| matches!(e, PlanEntry::Skill { skill_id, level } if *skill_id == sid && *level == l));
                if !exists {
                    plan.queue.push(PlanEntry::Skill { skill_id: sid, level: l });
                    added += 1;
                }
            }
        }
    }
    save_plan(core, character_id, plan);
    added
}

/// The whole document (backups).
pub fn export_all() -> PlanDoc {
    PLANS.lock().clone()
}

/// Replaces the whole document (backup restore).
pub fn import_all(core: &Core, doc: PlanDoc) {
    let mut d = PLANS.lock();
    *d = doc;
    save(&d);
    drop(d);
    changed(core);
}
