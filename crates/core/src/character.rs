//! A character's cached ESI data and the calculations the UI shows from it.
//!
//! Each kind of data lives in its own field, filled by the matching module in [`crate::domains`]. Fields are
//! `Option` until their first refresh succeeds. Feature state that isn't ESI data (mails marked read, notes, …)
//! goes in [`Character::local`] under a key named for the feature.

use crate::domains::*;
use crate::sde::{self, Attribute};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RefreshState {
    pub last: Option<DateTime<Utc>>,
    /// When it's due again; `None` = never (failed for good, e.g. scope missing)
    pub next: Option<DateTime<Utc>>,
    /// "scope", "token", "client" or "error"
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Character {
    pub id: i64,
    #[serde(default)]
    pub refreshes: BTreeMap<String, RefreshState>,
    #[serde(default)]
    pub last_manual_refresh: Option<DateTime<Utc>>,
    /// Per-feature local state, e.g. `local["mail.seen"]`.
    #[serde(default)]
    pub local: BTreeMap<String, serde_json::Value>,

    // foundation domains
    #[serde(default)]
    pub info: Option<info::Info>,
    #[serde(default)]
    pub skills: Option<skills::SkillsData>,
    #[serde(default)]
    pub skill_queue: Option<Vec<skills::QueueEntry>>,
    #[serde(default)]
    pub attributes: Option<skills::Attributes>,
    #[serde(default)]
    pub implants: Option<Vec<clones::Implant>>,
    #[serde(default)]
    pub clones: Option<clones::Clones>,
    #[serde(default)]
    pub location: Option<location::Location>,
    #[serde(default)]
    pub ship: Option<location::Ship>,
    #[serde(default)]
    pub fatigue: Option<location::Fatigue>,
    #[serde(default)]
    pub wallet: Option<f64>,

    // character pages
    #[serde(default)]
    pub loyalty: Option<social::LoyaltyData>,
    #[serde(default)]
    pub standings: Option<social::StandingsData>,
    #[serde(default)]
    pub research: Option<social::ResearchData>,
    #[serde(default)]
    pub notifications: Option<social::NotificationsData>,
    #[serde(default)]
    pub calendar: Option<social::CalendarData>,
    #[serde(default)]
    pub killmails: Option<social::KillmailsData>,
    #[serde(default)]
    pub medals: Option<social::MedalsData>,
    #[serde(default)]
    pub contacts: Option<social::ContactsData>,
    #[serde(default)]
    pub fw: Option<social::FwData>,

    // comms and money
    #[serde(default)]
    pub mail: Option<mail::MailData>,
    #[serde(default)]
    pub contracts: Option<contracts::ContractsData>,
    #[serde(default)]
    pub wallet_journal: Option<wallet::JournalData>,
    #[serde(default)]
    pub wallet_transactions: Option<wallet::TransactionsData>,
    #[serde(default)]
    pub market_orders: Option<wallet::MarketOrdersData>,

    // assets and industry
    #[serde(default)]
    pub assets: Option<assets::AssetsData>,
    #[serde(default)]
    pub industry: Option<industry::IndustryData>,
    #[serde(default)]
    pub planets: Option<industry::PlanetsData>,
    #[serde(default)]
    pub blueprints: Option<industry::BlueprintsData>,
}

/// Every injector extracts 500k SP.
pub const INJECTOR_SP: i64 = 500_000;
pub const DEFAULT_FARM_BASE_SP: i64 = 5_000_000;

impl Character {
    pub fn new(id: i64) -> Self {
        Character { id, ..Default::default() }
    }

    pub fn name(&self) -> String {
        self.info.as_ref().map(|i| i.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Character #{}", self.id))
    }

    pub fn portrait_url(&self, size: u32) -> String {
        crate::config::character_portrait(self.id, size)
    }

    pub fn corporation_name(&self) -> Option<&str> {
        self.info.as_ref().and_then(|i| i.corporation.as_ref()).map(|c| c.name.as_str())
    }

    pub fn alliance_name(&self) -> Option<&str> {
        self.info.as_ref().and_then(|i| i.alliance.as_ref()).map(|a| a.name.as_str())
    }

    pub fn has_basic_info(&self) -> bool {
        self.info.is_some()
    }

    pub fn queue(&self) -> &[skills::QueueEntry] {
        self.skill_queue.as_deref().unwrap_or(&[])
    }

    pub fn skill_list(&self) -> &[skills::Skill] {
        self.skills.as_ref().map(|s| s.skills.as_slice()).unwrap_or(&[])
    }

    pub fn skill(&self, id: i64) -> Option<&skills::Skill> {
        self.skill_list().iter().find(|s| s.skill_id == id)
    }

    pub fn skill_level(&self, id: i64) -> u8 {
        self.skill(id).map(|s| s.active_skill_level).unwrap_or(0)
    }

    pub fn trained_skill_level(&self, id: i64) -> u8 {
        self.skill(id).map(|s| s.trained_skill_level).unwrap_or(0)
    }

    fn skill_level_by_name(&self, name: &str) -> u8 {
        sde::SKILLS.by_name(name).map(|s| self.skill_level(s.type_id)).unwrap_or(0)
    }

    /// The skill training right now (first queue entry that finishes in the future).
    pub fn current_skill(&self) -> Option<&skills::QueueEntry> {
        let now = Utc::now();
        self.queue().iter().find(|q| q.finish_date.map(|f| f > now).unwrap_or(false))
    }

    pub fn finished_in_queue(&self) -> Vec<&skills::QueueEntry> {
        let now = Utc::now();
        self.queue().iter().filter(|q| q.finish_date.map(|f| f < now).unwrap_or(false)).collect()
    }

    /// When the queue runs out, if it's training.
    pub fn queue_end(&self) -> Option<DateTime<Utc>> {
        let now = Utc::now();
        self.queue().iter().filter_map(|q| q.finish_date).filter(|f| *f > now).max()
    }

    pub fn queue_time_left(&self) -> Option<Duration> {
        self.queue_end().map(|e| e - Utc::now())
    }

    pub fn is_training(&self) -> bool {
        self.current_skill().is_some()
    }

    pub fn sp_per_ms(&self) -> f64 {
        let Some(q) = self.current_skill() else { return 0.0 };
        let (Some(start), Some(end), Some(s0), Some(s1)) = (q.start_date, q.finish_date, q.training_start_sp, q.level_end_sp) else {
            return 0.0;
        };
        let ms = (end - start).num_milliseconds() as f64;
        if ms <= 0.0 {
            0.0
        } else {
            (s1 - s0) as f64 / ms
        }
    }

    pub fn sp_per_hour(&self) -> i64 {
        (self.sp_per_ms() * 3_600_000.0).round() as i64
    }

    /// Total SP projected to now: the training skill's progress since it started, plus finished queue entries
    /// ESI's skill list hasn't caught up with. Not exact while a cerebral accelerator is active.
    pub fn total_sp(&self) -> i64 {
        let base = self.skills.as_ref().map(|s| s.total_sp).unwrap_or(0);
        let Some(current) = self.current_skill() else { return base };
        let finished = self.finished_in_queue();
        let now = Utc::now();
        let mut total = 0f64;
        for skill in self.skill_list() {
            if skill.skill_id == current.skill_id {
                let start = current.start_date.unwrap_or(now);
                let passed = (now - start).num_milliseconds().max(0) as f64;
                total += current.training_start_sp.unwrap_or(skill.skillpoints_in_skill) as f64 + passed * self.sp_per_ms();
            } else {
                total += skill.skillpoints_in_skill as f64;
                for q in finished.iter().filter(|q| q.skill_id == skill.skill_id) {
                    total += (q.level_end_sp.unwrap_or(0) - q.training_start_sp.unwrap_or(0)) as f64;
                }
            }
        }
        total.floor() as i64
    }

    pub fn unallocated_sp(&self) -> i64 {
        self.skills.as_ref().map(|s| s.unallocated_sp).unwrap_or(0)
    }

    pub fn injectors_ready(&self, base_sp: i64) -> i64 {
        ((self.total_sp() - base_sp) / INJECTOR_SP).max(0)
    }

    /// When the next injector's worth of SP will be trained (None when not training).
    pub fn next_injector_date(&self, base_sp: i64) -> Option<DateTime<Utc>> {
        let rate = self.sp_per_ms();
        if rate <= 0.0 {
            return None;
        }
        let target = base_sp + (self.injectors_ready(base_sp) + 1) * INJECTOR_SP;
        let needed = (target - self.total_sp()) as f64;
        Some(Utc::now() + Duration::milliseconds((needed / rate) as i64))
    }

    pub fn attribute(&self, a: Attribute) -> f64 {
        self.attributes.as_ref().map(|x| x.get(a) as f64).unwrap_or(0.0)
    }

    /// Training speed of the current skill relative to full Omega speed for the character's attributes:
    /// ~1 Omega, ~0.5 Alpha, more with an accelerator the attributes don't include.
    pub fn training_speed_ratio(&self) -> Option<f64> {
        let q = self.current_skill()?;
        let info = sde::SKILLS.get(q.skill_id)?;
        let attrs = self.attributes.as_ref()?;
        let hours = (q.finish_date? - q.start_date?).num_seconds() as f64 / 3600.0;
        let full = (attrs.get(info.primary_attribute) as f64 + attrs.get(info.secondary_attribute) as f64 / 2.0) * 60.0;
        let sp = (q.level_end_sp? - q.training_start_sp?) as f64;
        if hours <= 0.05 || full <= 0.0 || sp <= 0.0 {
            return None;
        }
        Some(sp / hours / full)
    }

    /// Omega (Some(true)), Alpha (Some(false)) or unknown.
    pub fn is_omega(&self) -> Option<bool> {
        if let Some(r) = self.training_speed_ratio() {
            if r >= 0.9 {
                return Some(true);
            }
            if (0.45..=0.56).contains(&r) {
                return Some(false);
            }
        }
        let total = self.skills.as_ref().map(|s| s.total_sp).unwrap_or(0);
        if total > 5_000_000 && self.is_training() {
            return Some(true);
        }
        let skills = self.skill_list();
        if skills.iter().any(|s| s.trained_skill_level > s.active_skill_level) {
            return Some(false);
        }
        if skills.iter().any(|s| s.active_skill_level > 0 && s.active_skill_level > sde::alpha_cap(&s.skill_name)) {
            return Some(true);
        }
        let day = Utc::now() + Duration::hours(24);
        if self.queue().iter().any(|q| q.finish_date.is_some() && q.start_date.map(|s| s > day).unwrap_or(false)) {
            return Some(true);
        }
        None
    }

    /// When the next jump clone jump is possible (24h minus 1h per Infomorph Synchronizing level).
    pub fn next_clone_jump(&self) -> Option<DateTime<Utc>> {
        let last = self.clones.as_ref()?.last_clone_jump_date?;
        let reduction = self.skill_level_by_name("Infomorph Synchronizing") as i64;
        Some(last + Duration::hours(24 - reduction))
    }

    pub fn max_jump_clones(&self) -> u8 {
        self.skill_level_by_name("Infomorph Psychology") + self.skill_level_by_name("Advanced Infomorph Psychology")
    }

    pub fn max_contracts(&self) -> u32 {
        1 + 4 * self.skill_level_by_name("Contracting") as u32
    }

    /// Next yearly-cooldown remap date, or None when one is available now.
    pub fn next_remap_date(&self) -> Option<DateTime<Utc>> {
        self.attributes.as_ref()?.accrued_remap_cooldown_date.filter(|d| *d > Utc::now())
    }

    /// Skills grouped by skill group (sorted by name), with each group's SP total.
    pub fn skill_tree(&self) -> Vec<(String, Vec<&skills::Skill>, i64)> {
        let mut groups: BTreeMap<String, Vec<&skills::Skill>> = BTreeMap::new();
        for s in self.skill_list() {
            groups.entry(s.group_name.clone()).or_default().push(s);
        }
        groups
            .into_iter()
            .map(|(name, mut list)| {
                list.sort_by(|a, b| a.skill_name.cmp(&b.skill_name));
                let total = list.iter().map(|s| s.skillpoints_in_skill).sum();
                (name, list, total)
            })
            .collect()
    }

    /// Most recent successful refresh of any data.
    pub fn last_updated(&self) -> Option<DateTime<Utc>> {
        self.refreshes.values().filter(|r| r.error.is_none()).filter_map(|r| r.last).max()
    }

    pub fn next_auto_refresh(&self) -> Option<DateTime<Utc>> {
        self.refreshes.values().filter_map(|r| r.next).min()
    }

    /// Time until the "Refresh from ESI" button can be used again.
    pub fn manual_refresh_wait(&self) -> Option<Duration> {
        let last = self.last_manual_refresh?;
        let ready = last + Duration::seconds(crate::config::MANUAL_REFRESH_COOLDOWN);
        let now = Utc::now();
        (ready > now).then(|| ready - now)
    }

    /// Per-feature local state.
    pub fn local<T: serde::de::DeserializeOwned + Default>(&self, key: &str) -> T {
        self.local.get(key).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
    }

    pub fn set_local<T: Serialize>(&mut self, key: &str, value: &T) {
        if let Ok(v) = serde_json::to_value(value) {
            self.local.insert(key.to_string(), v);
        }
    }
}
