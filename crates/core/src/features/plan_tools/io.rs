//! Importing and exporting plans: PodStack's own JSON (`.podstack_plan`, also the old `.cerebral_plan`), EVE's
//! skill plan text (the in-game Skills window's clipboard format), pasted skill lists (EVEMon, forums) and EVEMon's
//! XML plans.

use super::sim::Item;
use crate::sde;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonSkill {
    type_id: i64,
    level: u8,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JsonPlan {
    format_version: u32,
    #[serde(default)]
    source: String,
    #[serde(default)]
    created: i64,
    skills: Vec<JsonSkill>,
}

/// A plan as PodStack JSON: its skill levels only.
pub fn export_json(items: &[Item]) -> String {
    let plan = JsonPlan {
        format_version: 1,
        source: format!("podstack/{}", crate::config::VERSION),
        created: chrono::Utc::now().timestamp_millis(),
        skills: items.iter().filter_map(|i| i.skill()).map(|s| JsonSkill { type_id: s.id, level: s.level }).collect(),
    };
    serde_json::to_string(&plan).unwrap_or_default()
}

/// Skill levels from PodStack JSON (empty when it isn't a format-1 plan).
pub fn import_json(text: &str) -> Vec<(i64, u8)> {
    match serde_json::from_str::<JsonPlan>(text) {
        Ok(p) if p.format_version == 1 => p.skills.into_iter().map(|s| (s.type_id, s.level)).collect(),
        _ => vec![],
    }
}

/// A plan as the EVE client writes skill plans ("Copy to clipboard" in the Skills window), which its skill plan
/// import reads back: one line per level, English name and level number, e.g. "Spaceship Command 1".
pub fn eve_text(items: &[Item]) -> String {
    items.iter().filter_map(|i| i.skill()).map(|s| format!("{} {}", s.name, s.level)).collect::<Vec<_>>().join("\r\n")
}

/// Skill levels in an EVE skill plan text.
pub fn eve_text_count(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.split("\r\n").count()
    }
}

static LOCALIZED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)<localized\s+hint="([^"]*)"\s*>(.*?)</localized>"#).unwrap());
static BRACKETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(.*?\)|\[.*?\]").unwrap());
static BULLET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*(\d+[.)]|[-*•])\s+").unwrap());
static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static SKILL_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(.+?)\*?\s+(?:level\s+)?(I{1,3}|IV|V|[1-5])$").unwrap());

/// Result of reading a pasted skill list.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub skills: Vec<(i64, u8)>,
    /// lines that aren't a skill and level
    pub unknown: Vec<String>,
}

fn level_of(s: &str) -> u8 {
    match s.to_uppercase().as_str() {
        "I" => 1,
        "II" => 2,
        "III" => 3,
        "IV" => 4,
        "V" => 5,
        n => n.parse().unwrap_or(0),
    }
}

/// Reads a pasted skill list: one skill per line, as the EVE client, EVEMon and forums write them, e.g.
/// "Caldari Cruiser IV", "Caldari Cruiser 4", "1. Caldari Cruiser Level 4", "Caldari Cruiser\tIV (2d 3h)".
///
/// The EVE client's skill plan export writes one line per level, e.g. "Spaceship Command 1", and in non-English
/// clients wraps the name in a localisation tag with the English name marked by an asterisk:
/// `<localized hint="宇宙船操作">Spaceship Command*</localized> 1`.
pub fn parse_skill_text(text: &str) -> Parsed {
    let mut out = Parsed::default();
    for raw in text.lines() {
        // the English name is the tag's text or, in some clients, its hint
        let mut names = vec![];
        let line = LOCALIZED.replace_all(raw, |c: &regex::Captures| {
            names.push(c[1].trim_end_matches('*').trim().to_string());
            c[2].to_string()
        });
        let line = BRACKETS.replace_all(&line, " ");
        let line = BULLET.replace(&line, "");
        let line = SPACES.replace_all(&line, " ");
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let caps = SKILL_LINE.captures(line);
        let mut id = caps.as_ref().and_then(|c| sde::SKILLS.by_name(c[1].trim())).map(|s| s.type_id);
        if caps.is_some() && id.is_none() {
            id = names.iter().find_map(|n| sde::SKILLS.by_name(n)).map(|s| s.type_id);
        }
        match (id, caps) {
            (Some(id), Some(c)) => out.skills.push((id, level_of(&c[2]))),
            _ => out.unknown.push(line.to_string()),
        }
    }
    out
}

static ENTRY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)<entry\b([^>]*)>").unwrap());
static ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i)\b([a-z_]+)\s*=\s*"([^"]*)""#).unwrap());

/// Skill levels in an EVEMon plan (`<plan><entry skillID="3327" skill="Spaceship Command" level="3" …>`).
pub fn import_evemon_xml(text: &str) -> Vec<(i64, u8)> {
    let mut out = vec![];
    for e in ENTRY.captures_iter(text) {
        let mut id = None;
        let mut level = None;
        for a in ATTR.captures_iter(&e[1]) {
            match &a[1] {
                "skillID" => id = a[2].trim().parse::<i64>().ok(),
                "level" => level = a[2].trim().parse::<u8>().ok(),
                _ => {}
            }
        }
        if let (Some(id), Some(level)) = (id, level) {
            out.push((id, level));
        }
    }
    out
}

/// Reads any supported plan file by its content: PodStack JSON, EVEMon XML, or EVE/pasted text. Returns the skill
/// levels and the format's name.
pub fn import_any(text: &str) -> (Vec<(i64, u8)>, &'static str) {
    let t = text.trim_start_matches('\u{feff}').trim_start();
    if t.starts_with('{') {
        (import_json(t), "PodStack JSON")
    } else if t.starts_with('<') && t.contains("<entry") {
        (import_evemon_xml(t), "EVEMon XML")
    } else {
        (parse_skill_text(t).skills, "a skill list")
    }
}

#[cfg(test)]
mod tests {
    use super::super::sim::tests::{base, id};
    use super::super::sim::{Item, Planner};
    use super::*;

    #[test]
    fn parses_the_usual_formats() {
        let sc = id("Spaceship Command");
        let cc = id("Caldari Cruiser");
        let text = "Caldari Cruiser IV\nCaldari Cruiser 4\n1. Caldari Cruiser Level 4\nCaldari Cruiser\tIV (2d 3h)\n\
                    - Spaceship Command III\n<localized hint=\"宇宙船操作\">Spaceship Command*</localized> 1\n\
                    <localized hint=\"Spaceship Command*\">宇宙船操作</localized> 2\n\nNot a skill V\nCaldari Cruiser\n";
        let p = parse_skill_text(text);
        assert_eq!(p.skills, vec![(cc, 4), (cc, 4), (cc, 4), (cc, 4), (sc, 3), (sc, 1), (sc, 2)]);
        assert_eq!(p.unknown, vec!["Not a skill V".to_string(), "Caldari Cruiser".to_string()]);
    }

    #[test]
    fn json_round_trip() {
        let p = Planner::with_items(&base(), &[Item::wanted(id("Caldari Frigate"), 2)]);
        let json = export_json(&p.queue);
        let back = import_json(&json);
        assert_eq!(back, vec![(id("Spaceship Command"), 1), (id("Caldari Frigate"), 1), (id("Caldari Frigate"), 2)]);
        assert_eq!(import_any(&json).0, back);
        assert!(import_json("{\"formatVersion\":2,\"skills\":[]}").is_empty());
        assert!(import_json("nonsense").is_empty());
    }

    #[test]
    fn eve_text_round_trip() {
        let mut p = Planner::new(&base());
        p.add_note("x", "");
        p.plan_skill(id("Caldari Frigate"), 2, None);
        let t = eve_text(&p.queue);
        assert_eq!(t, "Spaceship Command 1\r\nCaldari Frigate 1\r\nCaldari Frigate 2");
        assert_eq!(eve_text_count(&t), 3);
        assert_eq!(eve_text_count(""), 0);
        assert_eq!(parse_skill_text(&t).skills, vec![(id("Spaceship Command"), 1), (id("Caldari Frigate"), 1), (id("Caldari Frigate"), 2)]);
    }

    #[test]
    fn evemon_xml() {
        let xml = r#"<?xml version="1.0"?>
<plan xmlns:xsd="http://www.w3.org/2001/XMLSchema" name="Test" revision="4">
  <sorting criteria="None" order="None" groupByPriority="false" />
  <entry skillID="3327" skill="Spaceship Command" level="1" priority="3" type="Prerequisite">
    <notes>Spaceship Command</notes>
  </entry>
  <entry level="2" skill="Spaceship Command" skillID="3327" priority="3" type="Planned" />
</plan>"#;
        assert_eq!(import_evemon_xml(xml), vec![(3327, 1), (3327, 2)]);
        assert_eq!(import_any(xml).1, "EVEMon XML");
    }
}
