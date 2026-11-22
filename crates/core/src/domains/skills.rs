//! Trained skills, the skill queue and attributes.

use super::{Cx, DomainResult, DomainSpec};
use crate::domain;
use crate::sde::{self, Attribute};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const DOMAINS: &[DomainSpec] = &[
    domain!("skills", "Skills", 3600, refresh_skills),
    domain!("skill_queue", "Skill Queue", 3600, refresh_queue),
    domain!("attributes", "Attributes and Remaps", 3600, refresh_attributes),
];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Skill {
    pub skill_id: i64,
    pub skillpoints_in_skill: i64,
    pub trained_skill_level: u8,
    pub active_skill_level: u8,
    #[serde(default)]
    pub skill_name: String,
    #[serde(default)]
    pub group_name: String,
    /// SP isn't exactly a level boundary (partly trained toward the next level)
    #[serde(default)]
    pub half_trained: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SkillsData {
    pub skills: Vec<Skill>,
    pub total_sp: i64,
    #[serde(default)]
    pub unallocated_sp: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QueueEntry {
    pub skill_id: i64,
    pub finished_level: u8,
    pub queue_position: i32,
    #[serde(default)]
    pub start_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub finish_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub training_start_sp: Option<i64>,
    #[serde(default)]
    pub level_start_sp: Option<i64>,
    #[serde(default)]
    pub level_end_sp: Option<i64>,
    #[serde(default)]
    pub skill_name: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Attributes {
    pub charisma: i32,
    pub intelligence: i32,
    pub memory: i32,
    pub perception: i32,
    pub willpower: i32,
    #[serde(default)]
    pub bonus_remaps: Option<i32>,
    #[serde(default)]
    pub last_remap_date: Option<DateTime<Utc>>,
    #[serde(default)]
    pub accrued_remap_cooldown_date: Option<DateTime<Utc>>,
}

impl Attributes {
    pub fn get(&self, a: Attribute) -> i32 {
        match a {
            Attribute::Charisma => self.charisma,
            Attribute::Intelligence => self.intelligence,
            Attribute::Memory => self.memory,
            Attribute::Perception => self.perception,
            Attribute::Willpower => self.willpower,
        }
    }
}

fn is_level_boundary(rank: u32, level: u8, sp: i64) -> bool {
    let need = sde::sp_for_level(rank, level);
    // CCP's rounding sometimes leaves one SP over
    sp == need || sp == need + 1
}

async fn refresh_skills(cx: &Cx) -> DomainResult {
    let mut data: SkillsData = cx.get(cx.char_path("skills")).scope("esi-skills.read_skills.v1").json().await?;
    let unknown: Vec<i64> = data.skills.iter().filter(|s| sde::SKILLS.get(s.skill_id).is_none()).map(|s| s.skill_id).collect();
    let fetched = cx.universe().types(cx.esi(), unknown).await;
    for s in &mut data.skills {
        if let Some(info) = sde::SKILLS.get(s.skill_id) {
            s.skill_name = info.name.clone();
            s.group_name = info.market_group_name.clone();
            if s.group_name.is_empty() {
                s.group_name = sde::SKILLS.group_of(s.skill_id).map(|g| g.name.clone()).unwrap_or_default();
            }
            s.half_trained = !is_level_boundary(info.training_time_multiplier, s.trained_skill_level, s.skillpoints_in_skill);
        } else if let Some(t) = fetched.get(&s.skill_id) {
            s.skill_name = t.name.clone();
            s.group_name = t.group_name.clone();
        }
    }
    Ok(Box::new(move |c| c.skills = Some(data)))
}

async fn refresh_queue(cx: &Cx) -> DomainResult {
    let mut queue: Vec<QueueEntry> = cx.get(cx.char_path("skillqueue")).scope("esi-skills.read_skillqueue.v1").json().await?;
    for q in &mut queue {
        q.skill_name = match sde::SKILLS.get(q.skill_id) {
            Some(s) => s.name.clone(),
            None => cx.universe().name(cx.esi(), q.skill_id).await.unwrap_or_else(|| format!("Skill #{}", q.skill_id)),
        };
    }
    queue.sort_by_key(|q| q.queue_position);
    Ok(Box::new(move |c| c.skill_queue = Some(queue)))
}

async fn refresh_attributes(cx: &Cx) -> DomainResult {
    let attrs: Attributes = cx.get(cx.char_path("attributes")).scope("esi-skills.read_skills.v1").json().await?;
    Ok(Box::new(move |c| c.attributes = Some(attrs)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_boundaries() {
        assert!(is_level_boundary(1, 5, 256_000));
        assert!(is_level_boundary(1, 5, 256_001));
        assert!(!is_level_boundary(1, 4, 50_000));
    }
}
