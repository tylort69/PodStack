//! What the tray icon shows on every platform: a line per character with its training and queue, sorted by skill
//! points.

use podstack_core::{fmt, Core};

const LEVELS: [&str; 6] = ["", "I", "II", "III", "IV", "V"];

#[derive(Clone, Default)]
pub struct Entry {
    pub id: i64,
    /// "Name: 3d 4h" for the tooltip.
    pub short: String,
    /// The menu line.
    pub label: String,
}

fn short(d: chrono::Duration) -> String {
    fmt::duration(d, 2)
}

pub fn entries(core: &Core) -> Vec<Entry> {
    let now = chrono::Utc::now();
    let mut chars: Vec<_> = core.characters().into_iter().filter(|c| c.has_basic_info()).collect();
    chars.sort_by_key(|c| std::cmp::Reverse(c.total_sp()));
    chars
        .iter()
        .map(|c| {
            let name = c.name();
            match (c.current_skill(), c.queue_end()) {
                (Some(cur), Some(end)) => {
                    let skill = if cur.skill_name.is_empty() { "Training".to_string() } else { cur.skill_name.clone() };
                    let level = LEVELS.get(cur.finished_level as usize).copied().unwrap_or("");
                    let skill_left = cur.finish_date.map(|f| short(f - now)).unwrap_or_default();
                    Entry {
                        id: c.id,
                        short: format!("{name}: {}", short(end - now)),
                        label: format!("{name} — {skill} {level} · {skill_left} (queue {})", short(end - now)),
                    }
                }
                _ => Entry { id: c.id, short: format!("{name}: idle"), label: format!("{name} — not training") },
            }
        })
        .collect()
}
