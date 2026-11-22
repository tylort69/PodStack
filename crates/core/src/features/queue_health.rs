//! Queue health: training status of every character, flagging idle or paused queues, queues about to run out,
//! failing logins and SP farms with injectors to extract.

use crate::character::Character;
use crate::features::farming;
use crate::{fmt, Core};
use chrono::{DateTime, Duration, Utc};

/// Choices for "flag queues ending within", in hours.
pub const LOW_QUEUE_OPTIONS: [i64; 5] = [12, 24, 48, 72, 168];
pub const DEFAULT_LOW_QUEUE_HOURS: i64 = 24;
/// App setting holding the low-queue window in hours.
pub const SETTING: &str = "queue_low_hours";

/// Flag levels, most severe first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Critical,
    Warning,
    Info,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Flag {
    pub level: Level,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    pub training: bool,
    /// When the queue runs out (only while training).
    pub queue_ends: Option<DateTime<Utc>>,
    pub flags: Vec<Flag>,
}

impl Health {
    /// A critical or warning flag (what the navigation badge counts).
    pub fn has_problem(&self) -> bool {
        self.flags.iter().any(|f| f.level != Level::Info)
    }

    pub fn has(&self, level: Level) -> bool {
        self.flags.iter().any(|f| f.level == level)
    }
}

/// "12 hours", "1 day", "7 days"
pub fn hours_label(h: i64) -> String {
    if h % 24 == 0 {
        let d = h / 24;
        format!("{d} day{}", if d == 1 { "" } else { "s" })
    } else {
        format!("{h} hours")
    }
}

pub fn low_queue_hours(core: &Core) -> i64 {
    core.setting::<Option<i64>>(SETTING).filter(|h| *h > 0).unwrap_or(DEFAULT_LOW_QUEUE_HOURS)
}

pub fn set_low_queue_hours(core: &Core, hours: i64) {
    core.set_setting(SETTING, &hours);
}

fn countdown(d: Duration) -> String {
    fmt::duration(d, 4)
}

/// Health of one character's training. `token_failing`: its last token refresh failed; `farm_base_sp`: set when
/// the character is an SP farm; `window`: how soon a queue end (or next injector) gets flagged.
pub fn check(c: &Character, token_failing: bool, farm_base_sp: Option<i64>, window: Duration, now: DateTime<Utc>) -> Health {
    let mut flags = vec![];
    let queue = c.queue();
    let training = c.is_training();

    if token_failing {
        flags.push(Flag { level: Level::Critical, text: "Token failing, re-authorize (data may be stale)".into() });
    }

    let mut queue_ends = None;
    if !training {
        // EVE lists a paused queue's skills without start and finish dates
        let text = if !queue.is_empty() && queue.iter().all(|q| q.finish_date.is_none()) {
            "Training paused"
        } else if queue.is_empty() {
            "Not training: queue empty"
        } else {
            "Not training: queue finished"
        };
        flags.push(Flag { level: Level::Critical, text: text.into() });
    } else {
        queue_ends = c.queue_end();
        if let Some(end) = queue_ends {
            if end - now < window {
                flags.push(Flag { level: Level::Warning, text: format!("Queue ends in {}", countdown(end - now)) });
            }
        }
    }

    if let Some(base) = farm_base_sp {
        let ready = c.injectors_ready(base);
        if ready > 0 {
            flags.push(Flag { level: Level::Info, text: format!("{ready} injector{} ready to extract", if ready == 1 { "" } else { "s" }) });
        } else if training {
            if let Some(next) = c.next_injector_date(base) {
                if next - now < window {
                    flags.push(Flag { level: Level::Info, text: format!("Injector ready in {}", countdown(next - now)) });
                }
            }
        }
    }

    Health { training, queue_ends, flags }
}

/// [`check`] with the character's login, farm and the low-queue setting from the app.
pub fn check_in(core: &Core, c: &Character) -> Health {
    let token_failing = core.auth.get(c.id).and_then(|a| a.last_refresh).map(|r| !r.success).unwrap_or(false);
    let farm = farming::get(c.id).map(|f| f.base_sp);
    check(c, token_failing, farm, Duration::hours(low_queue_hours(core)), Utc::now())
}

/// Characters with a critical or warning flag, for the navigation badge.
pub fn count_problems(core: &Core) -> u32 {
    core.characters().iter().filter(|c| check_in(core, c).has_problem()).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domains::skills::{QueueEntry, Skill, SkillsData};

    fn training_char(queue_hours_left: i64) -> Character {
        let now = Utc::now();
        let mut c = Character::new(1);
        c.skills = Some(SkillsData {
            skills: vec![Skill { skill_id: 3300, skillpoints_in_skill: 5_900_000, trained_skill_level: 3, active_skill_level: 3, ..Default::default() }],
            total_sp: 5_900_000,
            unallocated_sp: 0,
        });
        // 1,000,000 SP over 400 hours = 2,500 SP/h; started 40 hours ago (+100,000 SP)
        c.skill_queue = Some(vec![QueueEntry {
            skill_id: 3300,
            finished_level: 4,
            queue_position: 0,
            start_date: Some(now - Duration::hours(40)),
            finish_date: Some(now + Duration::hours(queue_hours_left)),
            training_start_sp: Some(5_900_000),
            level_start_sp: Some(5_900_000),
            level_end_sp: Some(5_900_000 + 2_500 * (40 + queue_hours_left)),
            ..Default::default()
        }]);
        c
    }

    #[test]
    fn labels() {
        assert_eq!(hours_label(12), "12 hours");
        assert_eq!(hours_label(24), "1 day");
        assert_eq!(hours_label(168), "7 days");
    }

    #[test]
    fn idle_queues() {
        let mut c = Character::new(1);
        let h = check(&c, false, None, Duration::hours(24), Utc::now());
        assert!(!h.training);
        assert_eq!(h.flags, vec![Flag { level: Level::Critical, text: "Not training: queue empty".into() }]);

        c.skill_queue = Some(vec![QueueEntry { skill_id: 1, finished_level: 1, ..Default::default() }]);
        let h = check(&c, true, None, Duration::hours(24), Utc::now());
        assert_eq!(h.flags.len(), 2);
        assert_eq!(h.flags[1].text, "Training paused");
        assert!(h.has_problem());

        c.skill_queue = Some(vec![QueueEntry { skill_id: 1, finished_level: 1, finish_date: Some(Utc::now() - Duration::hours(1)), ..Default::default() }]);
        let h = check(&c, false, None, Duration::hours(24), Utc::now());
        assert_eq!(h.flags[0].text, "Not training: queue finished");
    }

    #[test]
    fn low_queue_and_farm() {
        let c = training_char(10);
        let h = check(&c, false, None, Duration::hours(24), Utc::now());
        assert!(h.training);
        assert!(h.queue_ends.is_some());
        assert_eq!(h.flags.len(), 1);
        assert_eq!(h.flags[0].level, Level::Warning);
        assert!(h.flags[0].text.starts_with("Queue ends in 9h") || h.flags[0].text.starts_with("Queue ends in 10h"));

        let c = training_char(100);
        let h = check(&c, false, None, Duration::hours(24), Utc::now());
        assert!(h.flags.is_empty());
        assert!(!h.has_problem());

        // ~6,000,000 SP on a 5,000,000 base: two injectors ready
        let h = check(&c, false, Some(5_000_000), Duration::hours(24), Utc::now());
        assert_eq!(h.flags, vec![Flag { level: Level::Info, text: "2 injectors ready to extract".into() }]);
        assert!(!h.has_problem());

        // 6,000,000 base: next injector 500,000 SP away at 2,500 SP/h = 200 hours, outside a day but inside 10 days
        assert!(check(&c, false, Some(6_000_000), Duration::hours(24), Utc::now()).flags.is_empty());
        let h = check(&c, false, Some(6_000_000), Duration::hours(240), Utc::now());
        assert!(h.has(Level::Info));
        assert!(h.flags.iter().any(|f| f.text.starts_with("Injector ready in 8d")));
    }
}
