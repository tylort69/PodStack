//! Plan simulation: a character's skills and attributes as a plan trains them, level by level.
//!
//! [`Planner`] is the port of the old app's `PlanCharacter`. Adding a skill adds its missing prerequisites first and
//! every missing level in turn, each timed with the attributes in force at that point (the character's own, then
//! each remap's), an active cerebral accelerator's bonus while it lasts, and half speed for Alpha clones.
//! Editing a plan (moving, removing, editing a remap) rebuilds it from its items, so times always follow the order.

use super::accelerator;
use crate::character::Character;
use crate::features::plans::PlanEntry;
use crate::sde::{self, Attribute, RequiredSkill};
use crate::training::AttributeSet;
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashMap};

/// The five attributes in the order the old app and EVE's remap screen list them.
pub const ATTRIBUTE_ORDER: [Attribute; 5] =
    [Attribute::Perception, Attribute::Memory, Attribute::Willpower, Attribute::Intelligence, Attribute::Charisma];

/// SP for a level as EVE counts it: 250 × rank × √32^(level−1), rounded up (e.g. 1,415 and 45,255 for rank 1).
pub fn sp_for_level(rank: u32, level: u8) -> f64 {
    if level == 0 {
        return 0.0;
    }
    (250.0 * rank as f64 * 32f64.powf((level as f64 - 1.0) / 2.0)).ceil()
}

/// A skill level in a plan, with its training numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct SkillItem {
    pub id: i64,
    pub level: u8,
    pub name: String,
    /// SP still to train for this level
    pub sp: f64,
    /// SP the skill has once this level is trained
    pub sp_total: f64,
    pub sp_hour: f64,
    /// training time in ms
    pub time: f64,
    /// ms since the last remap when this level finishes
    pub last_remap: f64,
    pub primary: Attribute,
    pub secondary: Attribute,
    pub required: Vec<(i64, u8)>,
    /// The plan it came from, in a combined training queue.
    pub plan: Option<(String, String)>,
}

impl SkillItem {
    /// "Caldari Cruiser IV"
    pub fn title(&self) -> String {
        format!("{} {}", self.name, crate::training::roman(self.level))
    }

    /// "Per / Wil"
    pub fn attribute_title(&self) -> String {
        format!("{} / {}", &self.primary.name()[..3], &self.secondary.name()[..3])
    }
}

/// One entry of a simulated plan.
#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Skill(SkillItem),
    /// Remap to these base attributes; `implants` is added to each (a uniform implant set).
    Remap { attributes: AttributeSet, implants: i32, plan: Option<(String, String)> },
    Note { title: String, details: String, plan: Option<(String, String)> },
}

impl Item {
    pub fn skill(&self) -> Option<&SkillItem> {
        match self {
            Item::Skill(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_skill(&self) -> bool {
        matches!(self, Item::Skill(_))
    }

    pub fn remap(attributes: AttributeSet, implants: i32) -> Item {
        Item::Remap { attributes, implants, plan: None }
    }

    pub fn note(title: &str, details: &str) -> Item {
        Item::Note { title: title.to_string(), details: details.to_string(), plan: None }
    }

    /// A skill item to plan, carrying only what [`Planner::add_item`] reads (id and level).
    pub fn wanted(id: i64, level: u8) -> Item {
        let info = sde::SKILLS.get(id);
        Item::Skill(SkillItem {
            id,
            level,
            name: info.map(|s| s.name.clone()).unwrap_or_default(),
            sp: 0.0,
            sp_total: 0.0,
            sp_hour: 0.0,
            time: 0.0,
            last_remap: 0.0,
            primary: info.map(|s| s.primary_attribute).unwrap_or(Attribute::Intelligence),
            secondary: info.map(|s| s.secondary_attribute).unwrap_or(Attribute::Memory),
            required: info.map(|s| required(&s.required_skills)).unwrap_or_default(),
            plan: None,
        })
    }

    /// The plan an item of a combined training queue came from: (id, name).
    pub fn plan(&self) -> Option<&(String, String)> {
        match self {
            Item::Skill(s) => s.plan.as_ref(),
            Item::Remap { plan, .. } | Item::Note { plan, .. } => plan.as_ref(),
        }
    }

    fn set_plan(&mut self, p: (String, String)) {
        match self {
            Item::Skill(s) => s.plan = Some(p),
            Item::Remap { plan, .. } | Item::Note { plan, .. } => *plan = Some(p),
        }
    }

    /// The row title, e.g. "Remap - P27 M21 W17 I17 C17" (attributes with implants) or "Note - Fit".
    pub fn title(&self) -> String {
        match self {
            Item::Skill(s) => s.title(),
            Item::Remap { attributes, implants, .. } => format!("Remap - {}", remap_label(&attributes.plus_all(*implants as f64))),
            Item::Note { title, .. } => format!("Note - {title}"),
        }
    }

    /// The stored form.
    pub fn to_entry(&self) -> PlanEntry {
        match self {
            Item::Skill(s) => PlanEntry::Skill { skill_id: s.id, level: s.level },
            Item::Remap { attributes, implants, .. } => PlanEntry::Remap {
                attributes: Attribute::ALL.iter().map(|a| (a.key().to_string(), attributes.get(*a).round() as i32)).collect(),
                implants: Some(serde_json::Value::from(*implants)),
            },
            Item::Note { title, details, .. } => PlanEntry::Note { text: note_text(title, details) },
        }
    }

    /// From the stored form (skills carry only id and level until planned).
    pub fn from_entry(e: &PlanEntry) -> Item {
        match e {
            PlanEntry::Skill { skill_id, level } => Item::wanted(*skill_id, *level),
            PlanEntry::Remap { attributes, implants } => {
                let mut a = AttributeSet::default();
                for attr in Attribute::ALL {
                    a.set(attr, attributes.get(attr.key()).copied().unwrap_or(17) as f64);
                }
                let implants = implants.as_ref().and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
                Item::Remap { attributes: a, implants, plan: None }
            }
            PlanEntry::Note { text } => {
                let (title, details) = split_note(text);
                Item::Note { title, details, plan: None }
            }
        }
    }
}

/// "P27 M21 W17 I17 C17"
pub fn remap_label(a: &AttributeSet) -> String {
    format!(
        "P{} M{} W{} I{} C{}",
        a.perception.round(),
        a.memory.round(),
        a.willpower.round(),
        a.intelligence.round(),
        a.charisma.round()
    )
}

/// Notes are stored as one text: the title on the first line, the details after it.
pub fn note_text(title: &str, details: &str) -> String {
    let title = title.replace('\n', " ");
    if details.trim().is_empty() {
        title
    } else {
        format!("{title}\n{details}")
    }
}

pub fn split_note(text: &str) -> (String, String) {
    match text.split_once('\n') {
        Some((t, d)) => (t.to_string(), d.to_string()),
        None => (text.to_string(), String::new()),
    }
}

fn required(r: &[RequiredSkill]) -> Vec<(i64, u8)> {
    r.iter().map(|r| (r.id, r.level)).collect()
}

/// An active accelerator as plans count it: its bonus for the first `remaining` ms of the plan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoostWindow {
    pub bonus: f64,
    pub end: DateTime<Utc>,
    pub remaining: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct SkillState {
    trained: u8,
    sp: f64,
    planned: u8,
    planned_sp: f64,
}

/// What a plan starts from: the character's skills (with training progress up to now), attributes without an
/// active accelerator, clone state and the accelerator's window.
#[derive(Clone, Debug, Default)]
pub struct PlanBase {
    skills: HashMap<i64, (u8, f64)>,
    pub attributes: AttributeSet,
    /// Some(false) = Alpha (half speed); unknown is planned at Omega speed.
    pub omega: Option<bool>,
    pub accelerator: Option<BoostWindow>,
    /// ms since the last remap, now
    pub last_remap: f64,
}

impl PlanBase {
    /// From a character as of `now`. `with_accelerator: false` plans without an active accelerator (for
    /// comparisons that apply one themselves); the character's own "plan without it" switch is honoured too.
    pub fn of(c: &Character, now: DateTime<Utc>, with_accelerator: bool) -> PlanBase {
        let booster = accelerator::attribute_bonus(c) as f64;
        let mut attributes = AttributeSet::of(c);
        for a in Attribute::ALL {
            attributes.set(a, attributes.get(a) - booster);
        }
        let accelerator = if with_accelerator && !accelerator::ignored_in_plans(c) { accelerator::plan_window(c, now) } else { None };
        let last_remap = c.attributes.as_ref().and_then(|a| a.last_remap_date).map(|d| (now - d).num_milliseconds() as f64).unwrap_or(0.0);

        let current = c.queue().iter().find(|q| q.finish_date.map(|f| f > now).unwrap_or(false));
        let finished: Vec<_> = c.queue().iter().filter(|q| q.finish_date.map(|f| f < now).unwrap_or(false)).collect();
        let mut skills = HashMap::new();
        for s in c.skill_list() {
            let mut level = s.trained_skill_level;
            let mut sp = s.skillpoints_in_skill as f64;
            if let Some(q) = current.filter(|q| q.skill_id == s.skill_id) {
                // the skill training now: its SP so far
                let start = q.start_date.unwrap_or(now);
                let passed = (now - start).num_milliseconds().max(0) as f64;
                sp = q.training_start_sp.unwrap_or(s.skillpoints_in_skill) as f64 + passed * rate_of(q);
            } else if let Some(q) = finished.iter().filter(|q| q.skill_id == s.skill_id).max_by_key(|q| q.finished_level) {
                // finished in the queue since ESI's skill list was loaded
                level = q.finished_level;
                sp = q.level_end_sp.unwrap_or(s.skillpoints_in_skill) as f64;
            }
            skills.insert(s.skill_id, (level, sp));
        }
        PlanBase { skills, attributes, omega: c.is_omega(), accelerator, last_remap }
    }

    /// A base for tests and previews: no skills, the given attributes.
    pub fn with_attributes(attributes: AttributeSet) -> PlanBase {
        PlanBase { attributes, ..Default::default() }
    }

    /// Adds a trained skill (tests and previews).
    pub fn set_skill(&mut self, id: i64, level: u8, sp: f64) {
        self.skills.insert(id, (level, sp));
    }

    pub fn trained_level(&self, id: i64) -> u8 {
        self.skills.get(&id).map(|s| s.0).unwrap_or(0)
    }

    pub fn is_injected(&self, id: i64) -> bool {
        self.skills.contains_key(&id)
    }
}

/// SP per ms of a queue entry.
fn rate_of(q: &crate::domains::skills::QueueEntry) -> f64 {
    match (q.start_date, q.finish_date, q.training_start_sp, q.level_end_sp) {
        (Some(s), Some(f), Some(a), Some(b)) => {
            let ms = (f - s).num_milliseconds() as f64;
            if ms > 0.0 {
                (b - a) as f64 / ms
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}

/// A plan being simulated (the old app's PlanCharacter).
#[derive(Clone, Debug)]
pub struct Planner {
    base: PlanBase,
    skills: HashMap<i64, SkillState>,
    pub attributes: AttributeSet,
    pub queue: Vec<Item>,
    /// total training time, ms
    pub time: f64,
    /// ms since the last remap at the end of the plan so far
    pub last_remap: f64,
    banned_skills: Option<Vec<i64>>,
    banned_skill: Option<(i64, u8)>,
}

impl Planner {
    pub fn new(base: &PlanBase) -> Planner {
        Planner {
            base: base.clone(),
            skills: HashMap::new(),
            attributes: base.attributes,
            queue: vec![],
            time: 0.0,
            last_remap: base.last_remap,
            banned_skills: None,
            banned_skill: None,
        }
    }

    /// A planner with the plan's items added.
    pub fn with_items(base: &PlanBase, items: &[Item]) -> Planner {
        let mut p = Planner::new(base);
        p.add_items(items);
        p
    }

    pub fn base(&self) -> &PlanBase {
        &self.base
    }

    pub fn omega(&self) -> Option<bool> {
        self.base.omega
    }

    pub fn accelerator(&self) -> Option<BoostWindow> {
        self.base.accelerator
    }

    /// ms since the last remap when the plan starts.
    pub fn last_remap_at_start(&self) -> f64 {
        self.base.last_remap
    }

    /// Forgets every planned item.
    pub fn reset(&mut self) {
        self.queue.clear();
        self.time = 0.0;
        self.attributes = self.base.attributes;
        self.last_remap = self.base.last_remap;
        self.skills.clear();
    }

    /// Empties the plan and adds `items` again (after reordering or editing).
    pub fn rebuild(&mut self, items: &[Item]) {
        self.reset();
        self.add_items(items);
    }

    pub fn add_items(&mut self, items: &[Item]) {
        for i in items {
            self.add_item(i);
        }
    }

    pub fn add_item(&mut self, item: &Item) {
        match item {
            Item::Skill(s) => self.plan_skill(s.id, s.level, None),
            Item::Remap { attributes, implants, .. } => self.add_remap(*attributes, *implants),
            Item::Note { title, details, .. } => self.add_note(title, details),
        }
    }

    fn state(&mut self, id: i64) -> &mut SkillState {
        let base = &self.base;
        self.skills.entry(id).or_insert_with(|| {
            let (trained, sp) = base.skills.get(&id).copied().unwrap_or((0, 0.0));
            SkillState { trained, sp, planned: 0, planned_sp: 0.0 }
        })
    }

    /// The highest level the character has or the plan reaches so far.
    pub fn level_of(&mut self, id: i64) -> u8 {
        let s = *self.state(id);
        s.trained.max(s.planned)
    }

    pub fn skill_count(&self) -> usize {
        self.queue.iter().filter(|i| i.is_skill()).count()
    }

    /// Adds a remap: from here on the attributes are `attributes` plus `implants` on each.
    pub fn add_remap(&mut self, attributes: AttributeSet, implants: i32) {
        self.attributes = attributes.plus_all(implants as f64);
        self.last_remap = 0.0;
        self.queue.push(Item::remap(attributes, implants));
    }

    pub fn add_note(&mut self, title: &str, details: &str) {
        self.queue.push(Item::note(title, details));
    }

    /// Changes the note at `index` (times don't change).
    pub fn edit_note(&mut self, index: usize, title: &str, details: &str) {
        if let Some(Item::Note { title: t, details: d, .. }) = self.queue.get_mut(index) {
            *t = title.to_string();
            *d = details.to_string();
        }
    }

    /// Replaces the remap at `index` and re-times the plan.
    pub fn edit_remap(&mut self, index: usize, attributes: AttributeSet, implants: i32) {
        if !matches!(self.queue.get(index), Some(Item::Remap { .. })) {
            return;
        }
        let mut items = self.queue.clone();
        items[index] = Item::remap(attributes, implants);
        self.rebuild(&items);
    }

    /// Plans a skill to `level`, its missing prerequisites first (at least to `prereq_level` when given), each
    /// missing level as its own item. Level 0 with `prereq_level` plans only the prerequisites.
    pub fn plan_skill(&mut self, id: i64, level: u8, prereq_level: Option<u8>) {
        let Some(info) = sde::SKILLS.get(id) else { return };
        let st = *self.state(id);
        if !((st.trained < level && st.planned < level) || level == 0) {
            return;
        }
        let current = st.trained.max(st.planned);

        for r in &info.required_skills {
            let target = match prereq_level {
                Some(p) if p > r.level => p,
                _ => r.level,
            };
            let banned_list = self.banned_skills.as_ref().map(|b| b.contains(&r.id)).unwrap_or(false);
            let banned_direct = matches!(self.banned_skill, Some((bid, blvl)) if bid == r.id && blvl <= r.level);
            if banned_list || banned_direct {
                self.banned_skills.get_or_insert_with(Vec::new).push(id);
            } else {
                self.plan_skill(r.id, target, prereq_level);
            }
        }

        let a = &self.attributes;
        let mut sp_per_hour = (a.get(info.primary_attribute) + a.get(info.secondary_attribute) / 2.0) * 60.0;
        // with an active accelerator: its bonus on both attributes
        let mut boosted = match self.base.accelerator {
            Some(w) => sp_per_hour + w.bonus * 1.5 * 60.0,
            None => sp_per_hour,
        };
        // Alpha clones train at half speed; a character not known to be Alpha is planned at Omega speed
        if self.base.omega == Some(false) {
            sp_per_hour *= 0.5;
            boosted *= 0.5;
        }

        for lvl in (current + 1)..=level {
            let banned_list = self.banned_skills.as_ref().map(|b| b.contains(&id)).unwrap_or(false);
            let banned_direct = matches!(self.banned_skill, Some((bid, blvl)) if bid == id && blvl <= lvl);
            if banned_list || banned_direct {
                continue;
            }
            let st = *self.state(id);
            let current_sp = st.sp.max(st.planned_sp);
            let sp_total = sp_for_level(info.training_time_multiplier, lvl);
            let missing = sp_total - current_sp;

            // seconds: at the boosted speed while the accelerator lasts, the normal speed after
            let boost_left = self.base.accelerator.map(|w| ((w.remaining - self.time) / 1000.0).max(0.0)).unwrap_or(0.0);
            let mut secs = if boosted > 0.0 { missing * (3600.0 / boosted) } else { 0.0 };
            if secs > boost_left && sp_per_hour > 0.0 {
                let sp_boosted = boost_left * boosted / 3600.0;
                secs = boost_left + (missing - sp_boosted) * (3600.0 / sp_per_hour);
            }

            self.time += secs * 1000.0;
            self.last_remap += secs * 1000.0;
            let s = self.state(id);
            s.planned = lvl;
            s.planned_sp = sp_total;

            self.queue.push(Item::Skill(SkillItem {
                id,
                level: lvl,
                name: info.name.clone(),
                sp: missing,
                sp_total,
                sp_hour: if secs > 0.0 { (missing / (secs / 3600.0)).round() } else { sp_per_hour.round() },
                time: secs * 1000.0,
                last_remap: self.last_remap,
                primary: info.primary_attribute,
                secondary: info.secondary_attribute,
                required: required(&info.required_skills),
                plan: None,
            }));
        }
    }

    /// Whether the skill at `old` can move to `new` without passing a remap/note, a higher level of itself
    /// or a skill needing it (moving down), or a lower level or one of its prerequisites (moving up).
    pub fn can_move(&self, old: usize, new: usize) -> bool {
        let Some(Item::Skill(m)) = self.queue.get(old) else { return false };
        let new = new.min(self.queue.len().saturating_sub(1));
        if old < new {
            !self.queue[old + 1..=new].iter().any(|i| match i {
                Item::Skill(s) => (s.id == m.id && s.level > m.level) || s.required.iter().any(|(rid, rl)| *rid == m.id && *rl >= m.level),
                _ => true,
            })
        } else {
            !self.queue[new..old].iter().any(|i| match i {
                Item::Skill(s) => (m.id == s.id && s.level < m.level) || m.required.iter().any(|(rid, rl)| *rid == s.id && s.level <= *rl),
                _ => true,
            })
        }
    }

    /// Moves the item at `old` to position `new` (as in "remove it, then insert it at `new`") and re-times the
    /// plan. A skill moved ahead of its prerequisites pulls them along, so prerequisites always come first.
    pub fn move_item(&mut self, old: usize, new: usize) {
        if old >= self.queue.len() {
            return;
        }
        let mut items = self.queue.clone();
        let it = items.remove(old);
        let new = new.min(items.len());
        items.insert(new, it);
        self.rebuild(&items);
    }

    /// Moves a set of items (`selected`, positions) together so the dragged item `old` lands at `new`, keeping
    /// their order, and re-times the plan.
    pub fn move_items(&mut self, old: usize, new: usize, selected: &[usize]) {
        if selected.is_empty() {
            return;
        }
        let mut selected: Vec<usize> = selected.iter().copied().filter(|i| *i < self.queue.len()).collect();
        selected.sort_unstable();
        selected.dedup();
        let mut items = self.queue.clone();
        let mut n = new;
        let mut moving = Vec::new();
        for &i in selected.iter().rev() {
            if i < n && i != old {
                n = n.saturating_sub(1);
            }
            moving.insert(0, items.remove(i));
        }
        let n = n.min(items.len());
        for (k, it) in moving.into_iter().enumerate() {
            items.insert(n + k, it);
        }
        self.rebuild(&items);
    }

    /// Removes the item at `index`. Skills that need it plan it again where they are; with `force` the level is
    /// removed with every level above it and every skill that needs them.
    pub fn remove_item(&mut self, index: usize, force: bool) {
        if index >= self.queue.len() {
            return;
        }
        let mut items = self.queue.clone();
        if force {
            if let Item::Skill(s) = &self.queue[index] {
                self.banned_skills = Some(vec![]);
                self.banned_skill = Some((s.id, s.level));
                self.rebuild(&items);
                self.banned_skills = None;
                self.banned_skill = None;
                return;
            }
        }
        items.remove(index);
        self.rebuild(&items);
    }

    /// Removes several items at once (positions), re-timing the plan once.
    pub fn remove_items(&mut self, indexes: &[usize]) {
        let items: Vec<Item> = self.queue.iter().enumerate().filter(|(i, _)| !indexes.contains(i)).map(|(_, it)| it.clone()).collect();
        self.rebuild(&items);
    }

    /// The stored form of the plan.
    pub fn entries(&self) -> Vec<PlanEntry> {
        self.queue.iter().map(Item::to_entry).collect()
    }
}

/// A plan's stored entries as items.
pub fn items_of(entries: &[PlanEntry]) -> Vec<Item> {
    entries.iter().map(Item::from_entry).collect()
}

/// The character's combined training queue: every switched-on plan, in priority order. A level an earlier plan
/// already trains isn't repeated. Each item says which plan it came from.
pub fn training_queue(base: &PlanBase, plans: &[crate::features::plans::SkillPlan]) -> Planner {
    let mut p = Planner::new(base);
    for plan in plans.iter().filter(|p| p.enabled) {
        let start = p.queue.len();
        p.add_items(&items_of(&plan.queue));
        for it in &mut p.queue[start..] {
            it.set_plan((plan.id.clone(), plan.name.clone()));
        }
    }
    p
}

/// Plans combined in the given order into one queue (for a merged plan): each plan's items headed by a note
/// naming it; levels an earlier plan already covers aren't repeated.
pub fn merge(base: &PlanBase, plans: &[crate::features::plans::SkillPlan]) -> Planner {
    let mut p = Planner::new(base);
    for plan in plans {
        p.add_note(&plan.name, "Merged plan");
        p.add_items(&items_of(&plan.queue));
    }
    p
}

/// Number of skill levels and training time (ms) of a stored plan for a character.
pub fn plan_summary(base: &PlanBase, entries: &[PlanEntry]) -> (usize, f64) {
    let p = Planner::with_items(base, &items_of(entries));
    (p.skill_count(), p.time)
}

/// Attributes from a stored remap's map (missing ones 17).
pub fn attributes_from_map(m: &BTreeMap<String, i32>) -> AttributeSet {
    let mut a = AttributeSet::default();
    for attr in Attribute::ALL {
        a.set(attr, m.get(attr.key()).copied().unwrap_or(17) as f64);
    }
    a
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn id(name: &str) -> i64 {
        sde::SKILLS.by_name(name).unwrap_or_else(|| panic!("{name}")).type_id
    }

    pub fn attrs(p: f64, m: f64, w: f64, i: f64, c: f64) -> AttributeSet {
        AttributeSet { perception: p, memory: m, willpower: w, intelligence: i, charisma: c }
    }

    pub fn base() -> PlanBase {
        PlanBase::with_attributes(attrs(20.0, 20.0, 20.0, 20.0, 19.0))
    }

    fn titles(p: &Planner) -> Vec<String> {
        p.queue.iter().map(|i| i.title()).collect()
    }

    #[test]
    fn sp_rounds_up() {
        assert_eq!(sp_for_level(1, 2), 1415.0);
        assert_eq!(sp_for_level(1, 4), 45255.0);
        assert_eq!(sp_for_level(1, 5), 256000.0);
    }

    #[test]
    fn plans_prerequisites_first() {
        let mut p = Planner::new(&base());
        p.plan_skill(id("Caldari Cruiser"), 1, None);
        let t = titles(&p);
        assert_eq!(t.last().unwrap(), "Caldari Cruiser I");
        let frig = t.iter().position(|x| x == "Caldari Frigate III").unwrap();
        assert!(frig < t.len() - 1);
        // each level once
        let mut sorted = t.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), t.len());
    }

    #[test]
    fn skips_trained_levels_and_counts_partial_sp() {
        let mut b = base();
        let sc = id("Spaceship Command");
        b.set_skill(sc, 2, 1415.0 + 3000.0);
        let mut p = Planner::new(&b);
        p.plan_skill(sc, 3, None);
        assert_eq!(p.queue.len(), 1);
        let s = p.queue[0].skill().unwrap();
        assert_eq!(s.level, 3);
        assert_eq!(s.sp, sp_for_level(1, 3) - 4415.0);
        // Spaceship Command is Per/Wil: (20 + 10) × 60 = 1800 SP/h
        assert!((s.time - s.sp / 1800.0 * 3_600_000.0).abs() < 1.0);
        assert_eq!(s.sp_hour, 1800.0);
    }

    #[test]
    fn alpha_trains_at_half_speed() {
        let mut b = base();
        let sc = id("Spaceship Command");
        let omega = Planner::with_items(&b, &[Item::wanted(sc, 3)]).time;
        b.omega = Some(false);
        let alpha = Planner::with_items(&b, &[Item::wanted(sc, 3)]).time;
        assert!((alpha - 2.0 * omega).abs() < 1.0);
    }

    #[test]
    fn remap_changes_speed_after_it() {
        let b = base();
        let sc = id("Spaceship Command");
        let mut p = Planner::new(&b);
        p.add_remap(attrs(27.0, 17.0, 21.0, 17.0, 17.0), 0);
        p.plan_skill(sc, 1, None);
        let s = p.queue[1].skill().unwrap();
        assert_eq!(s.sp_hour, ((27.0 + 21.0 / 2.0) * 60.0f64).round());
        assert_eq!(p.queue[0].title(), "Remap - P27 M17 W21 I17 C17");
        assert!(s.last_remap > 0.0 && s.last_remap == s.time);
    }

    #[test]
    fn accelerator_window_splits_a_level() {
        let mut b = base();
        let sc = id("Spaceship Command");
        // 1800 SP/h normally, +10 → 1800 + 900 = 2700 SP/h for the first hour
        b.accelerator = Some(BoostWindow { bonus: 10.0, end: Utc::now(), remaining: 3_600_000.0 });
        let p = Planner::with_items(&b, &[Item::wanted(sc, 4)]);
        let total_sp: f64 = p.queue.iter().filter_map(|i| i.skill()).map(|s| s.sp).sum();
        // 2700 SP in the first hour, the rest at 1800
        let expected = 3_600_000.0 + (total_sp - 2700.0) / 1800.0 * 3_600_000.0;
        assert!((p.time - expected).abs() < 2.0, "{} vs {}", p.time, expected);
    }

    #[test]
    fn removing_a_prerequisite_replans_it_unless_forced() {
        let b = base();
        let cruiser = id("Caldari Cruiser");
        let mut p = Planner::with_items(&b, &[Item::wanted(cruiser, 2)]);
        let n = p.queue.len();
        let frig3 = p.queue.iter().position(|i| i.title() == "Caldari Frigate III").unwrap();
        p.remove_item(frig3, false);
        // it comes back, as Caldari Cruiser needs it
        assert_eq!(p.queue.len(), n);
        let frig3 = p.queue.iter().position(|i| i.title() == "Caldari Frigate III").unwrap();
        p.remove_item(frig3, true);
        let t = titles(&p);
        assert!(!t.iter().any(|x| x.starts_with("Caldari Cruiser") || x == "Caldari Frigate III"), "{t:?}");
        assert!(t.iter().any(|x| x == "Caldari Frigate II"));
    }

    #[test]
    fn moving_respects_prerequisites() {
        let b = base();
        let mut p = Planner::with_items(&b, &[Item::wanted(id("Caldari Frigate"), 3), Item::wanted(id("Mechanics"), 2)]);
        let last = p.queue.len() - 1;
        let cf3 = p.queue.iter().position(|i| i.title() == "Caldari Frigate III").unwrap();
        let cf1 = p.queue.iter().position(|i| i.title() == "Caldari Frigate I").unwrap();
        assert!(p.can_move(cf3, last));
        assert!(!p.can_move(cf3, cf1));
        assert!(!p.can_move(cf1, cf3));
        // forcing level III before level I keeps the order valid
        p.move_item(cf3, 0);
        let t = titles(&p);
        let i1 = t.iter().position(|x| x == "Caldari Frigate I").unwrap();
        let i3 = t.iter().position(|x| x == "Caldari Frigate III").unwrap();
        assert!(i1 < i3);
        assert_eq!(t.len(), p.queue.len());
    }

    #[test]
    fn moving_a_group() {
        let b = base();
        let mut p = Planner::new(&b);
        p.add_note("a", "");
        p.add_note("b", "");
        p.add_note("c", "");
        p.add_note("d", "");
        // drag a and b (dragging a) to the end
        p.move_items(0, 3, &[0, 1]);
        let t = titles(&p);
        assert_eq!(t, vec!["Note - c", "Note - d", "Note - a", "Note - b"]);
        p.move_items(3, 0, &[2, 3]);
        assert_eq!(titles(&p), vec!["Note - a", "Note - b", "Note - c", "Note - d"]);
    }

    #[test]
    fn entries_round_trip() {
        let b = base();
        let mut p = Planner::new(&b);
        p.add_note("Fit", "for the Caracal\nsecond line");
        p.add_remap(attrs(27.0, 21.0, 17.0, 17.0, 17.0), 3);
        p.plan_skill(id("Caldari Frigate"), 2, None);
        let entries = p.entries();
        let json = serde_json::to_string(&entries).unwrap();
        let back: Vec<PlanEntry> = serde_json::from_str(&json).unwrap();
        let q = Planner::with_items(&b, &items_of(&back));
        assert_eq!(q.queue, p.queue);
    }

    #[test]
    fn training_queue_skips_levels_earlier_plans_cover() {
        use crate::features::plans::SkillPlan;
        let b = base();
        let cf = id("Caldari Frigate");
        let plan = |id: &str, prio: i32, enabled: bool, lvl: u8| SkillPlan {
            id: id.into(),
            name: id.into(),
            priority: prio,
            enabled,
            queue: vec![PlanEntry::Skill { skill_id: cf, level: lvl }],
            last_saved: Utc::now(),
        };
        let plans = vec![plan("a", 1, true, 2), plan("b", 2, true, 3), plan("c", 3, false, 5)];
        let q = training_queue(&b, &plans);
        let t = titles(&q);
        assert_eq!(t.iter().filter(|x| x.starts_with("Caldari Frigate")).count(), 3);
        assert_eq!(q.queue.last().unwrap().plan().unwrap().1, "b");
        let m = merge(&b, &plans[..2]);
        assert_eq!(m.queue[0].title(), "Note - a");
        assert!(m.queue.iter().any(|i| i.title() == "Note - b"));
    }
}
