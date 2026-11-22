//! Industry calculations: material amounts, job times and job costs, facilities with structure and rig bonuses,
//! invention chances, and the build planner that turns a list of things to make into every job, invention and
//! purchase needed, down to raw materials.
//!
//! Every number the game publishes comes from CCP's static data (`crate::blueprints::INDUSTRY`) or live ESI data
//! (cost indices and adjusted prices). The few rules that live only in CCP's patch notes are in [`Rules`], which
//! users can edit in the app, so a balance change never needs a PodStack update:
//!
//! - materials per job: `max(runs, ceil(round(base × runs × (1 − ME/100) × structure × rigs, 2)))`;
//! - job time: `base × runs × (1 − TE/100) × structure × rigs × skills`, with each skill's per-level bonus from the
//!   static data (Industry −4%/level, Advanced Industry −3%/level, the T2 science skills −1%/level, …);
//! - estimated item value (EIV): the sum of adjusted price × base quantity of the materials, per run;
//! - job cost: `EIV × system cost index × structure/rig cost bonus + EIV × facility tax + EIV × SCC surcharge`;
//!   invention and copying use a share of the product's EIV;
//! - invention chance: `base × (1 + (science skill levels)/30 + encryption level/40) × decryptor`.

use crate::blueprints::{Activity, Blueprint, BonusKind, IndustryData, INDUSTRY};
use crate::market::CostIndices;
use crate::sde::{SecurityClass, MAP, SKILLS};
use crate::{store, Character, Core, Event};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Rules from CCP's patch notes that the static data doesn't carry. Editable in the app.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Rules {
    /// The Secure Commerce Commission's surcharge on every job, as a fraction of EIV.
    pub scc_surcharge: f64,
    /// Facility tax in NPC stations.
    pub npc_facility_tax: f64,
    /// Share of the product's EIV that invention and copying jobs are charged on.
    pub invention_eiv_share: f64,
    pub copying_eiv_share: f64,
    /// ME and TE of a freshly invented blueprint copy, before the decryptor.
    pub invented_me: i64,
    pub invented_te: i64,
    /// ME and TE assumed for blueprints nobody owns (a fully researched original).
    pub assumed_me: i64,
    pub assumed_te: i64,
    /// Science skill levels add 1/30 each to invention chance, the encryption skill 1/40.
    pub invention_science_divisor: f64,
    pub invention_encryption_divisor: f64,
}

impl Default for Rules {
    fn default() -> Self {
        Rules {
            scc_surcharge: 0.04,
            npc_facility_tax: 0.0025,
            invention_eiv_share: 0.02,
            copying_eiv_share: 0.02,
            invented_me: 2,
            invented_te: 4,
            assumed_me: 10,
            assumed_te: 20,
            invention_science_divisor: 30.0,
            invention_encryption_divisor: 40.0,
        }
    }
}

pub const RULES_KEY: &str = "industry.rules";

pub fn rules(core: &Core) -> Rules {
    core.setting(RULES_KEY)
}

// ---------- facilities ----------

/// Where jobs run: an NPC station (no structure) or an Upwell structure with rigs, in a solar system.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Facility {
    pub id: String,
    pub name: String,
    pub system_id: i64,
    /// The structure type (Raitaru, Tatara, …); `None` for an NPC station.
    pub structure: Option<i64>,
    pub rigs: Vec<i64>,
    /// Facility tax as a fraction; `None` uses the NPC station rate (structures: what the owner set, often 0).
    pub tax: Option<f64>,
}

impl Facility {
    pub fn npc(system_id: i64) -> Self {
        Facility { id: "npc".into(), name: "NPC station".into(), system_id, ..Default::default() }
    }

    pub fn security(&self) -> SecurityClass {
        MAP.system(self.system_id).map(|s| s.security_class()).unwrap_or(SecurityClass::High)
    }

    pub fn tax(&self, rules: &Rules) -> f64 {
        self.tax.unwrap_or(if self.structure.is_some() { 0.0 } else { rules.npc_facility_tax })
    }

    /// The combined structure and rig multiplier for one kind of bonus on a job making `product`.
    pub fn multiplier(&self, data: &IndustryData, activity: Activity, kind: BonusKind, product: i64) -> f64 {
        let mut m = self.structure.and_then(|s| data.structure(s)).map(|s| s.multiplier(activity, kind)).unwrap_or(1.0);
        let sec = match self.security() {
            SecurityClass::High => 0,
            SecurityClass::Low => 1,
            SecurityClass::Null => 2,
        };
        for rig in self.rigs.iter().filter_map(|r| data.rig(*r)) {
            let Some(scale) = rig.security[sec] else { continue };
            for (a, k, percent, filter) in &rig.bonuses {
                if *a == activity && *k == kind && data.filter_matches(*filter, product) {
                    m *= 1.0 + percent / 100.0 * scale;
                }
            }
        }
        m
    }

    /// Whether the facility can run the activity at all (reactions need a refinery outside high security).
    pub fn supports(&self, data: &IndustryData, activity: Activity) -> bool {
        match (self.structure.and_then(|s| data.structure(s)), activity) {
            (None, Activity::Reaction) => false,
            (None, _) => true,
            (Some(s), Activity::Reaction) => s.supports(activity) && self.security() != SecurityClass::High,
            (Some(s), a) => s.supports(a),
        }
    }

    pub fn label(&self) -> String {
        let system = MAP.system(self.system_id).map(|s| s.name.clone()).unwrap_or_else(|| "?".into());
        format!("{} · {system}", self.name)
    }
}

/// The industry document: saved facilities, which one each activity uses by default, and saved projects.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IndustryDoc {
    pub facilities: Vec<Facility>,
    /// Activity key → facility id.
    pub defaults: BTreeMap<String, String>,
    pub projects: Vec<Project>,
    /// The character whose skills calculations use (None: the best level among all characters; 0: all at V).
    pub skills_of: Option<i64>,
    /// Hub (by name, see `sde::TRADE_HUBS`) whose prices value materials and products.
    pub price_hub: Option<String>,
}

pub const DOC: &str = "industry";

pub fn load(_core: &Core) -> IndustryDoc {
    store::load_doc(DOC)
}

pub fn save(core: &Core, doc: &IndustryDoc) {
    store::save_doc(DOC, doc);
    core.emit(Event::Doc(DOC.into()));
}

impl IndustryDoc {
    /// The facility jobs of `activity` use: the chosen default, else an NPC station in Jita (the place nearly
    /// everyone can reach).
    pub fn facility_for(&self, activity: Activity) -> Facility {
        self.defaults
            .get(activity.key())
            .and_then(|id| self.facilities.iter().find(|f| &f.id == id))
            .cloned()
            .unwrap_or_else(|| Facility::npc(crate::sde::TRADE_HUBS[0].2))
    }
}

/// A saved build: what to make and how.
#[derive(Clone, Debug, Serialize, Deserialize, Default, PartialEq)]
#[serde(default)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// (type, quantity)
    pub targets: Vec<(i64, i64)>,
    pub options: PlanChoices,
}

/// The choices a plan is made with (stored with projects).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PlanChoices {
    /// Build manufactured parts (components, T1 items for T2 builds) instead of buying them.
    pub build_components: bool,
    /// Run reactions for reaction products instead of buying them.
    pub build_reactions: bool,
    /// Invent T2 blueprint copies (and count their datacores) for T2 items with no original owned.
    pub invent: bool,
    pub decryptor: Option<i64>,
    /// Per item: true build, false buy (overrides the switches above).
    pub overrides: BTreeMap<i64, bool>,
    /// Take materials the characters already have (assets) off the shopping list.
    pub use_stock: bool,
    /// Longest job in days before it's split into several.
    pub max_job_days: f64,
}

impl Default for PlanChoices {
    fn default() -> Self {
        PlanChoices {
            build_components: true,
            build_reactions: false,
            invent: true,
            decryptor: None,
            overrides: BTreeMap::new(),
            use_stock: false,
            max_job_days: 30.0,
        }
    }
}

// ---------- skills ----------

/// Skill levels a calculation uses.
pub type Levels = HashMap<i64, i64>;

/// A character's trained levels.
pub fn levels_of(c: &Character) -> Levels {
    c.skills.as_ref().map(|s| s.skills.iter().map(|k| (k.skill_id, k.trained_skill_level as i64)).collect()).unwrap_or_default()
}

/// The best level of every skill among `characters`.
pub fn best_levels(characters: &[std::sync::Arc<Character>]) -> Levels {
    let mut out = Levels::new();
    for c in characters {
        for (s, l) in levels_of(c) {
            let e = out.entry(s).or_insert(0);
            *e = (*e).max(l);
        }
    }
    out
}

/// Every skill at level V (for planning without a character).
pub fn all_five() -> Levels {
    SKILLS.by_id.keys().map(|id| (*id, 5)).collect()
}

/// Skill bonus attributes that shorten each activity, for any trained skill.
fn time_attributes(activity: Activity) -> &'static [&'static str] {
    match activity {
        Activity::Manufacturing => &["manufacturingTimeBonus", "advancedIndustrySkillIndustryJobTimeBonus"],
        Activity::Reaction => &["reactionTimeBonus"],
        Activity::Copying => &["copySpeedBonus", "advancedIndustrySkillIndustryJobTimeBonus"],
        Activity::ResearchMaterial => &["mineralNeedResearchBonus", "advancedIndustrySkillIndustryJobTimeBonus"],
        Activity::ResearchTime => &["blueprintmanufactureTimeBonus", "advancedIndustrySkillIndustryJobTimeBonus"],
        Activity::Invention => &["advancedIndustrySkillIndustryJobTimeBonus"],
    }
}

/// The skills' combined time multiplier for a job (e.g. 0.68 with Industry V and Advanced Industry V).
pub fn skill_time_multiplier(data: &IndustryData, activity: Activity, required: &[(i64, i64)], levels: &Levels) -> f64 {
    let mut m = 1.0;
    for attr in time_attributes(activity) {
        for (skill, bonus) in data.skills_with(attr) {
            m *= 1.0 + bonus / 100.0 * *levels.get(&skill).unwrap_or(&0) as f64;
        }
    }
    // The T2 construction and science skills only speed up blueprints that require them.
    if activity == Activity::Manufacturing {
        for (skill, _) in required {
            if let Some(bonus) = data.skill_bonus(*skill, "manufactureTimePerLevel") {
                m *= 1.0 + bonus / 100.0 * *levels.get(skill).unwrap_or(&0) as f64;
            }
        }
    }
    m
}

/// Industry job slots of one kind: (manufacturing, science, reactions).
pub fn slots(data: &IndustryData, levels: &Levels) -> (i64, i64, i64) {
    let count = |attr: &str| -> i64 {
        1 + data.skills_with(attr).iter().map(|(s, per)| (*per * *levels.get(s).unwrap_or(&0) as f64) as i64).sum::<i64>()
    };
    (count("manufacturingSlotBonus"), count("laboratorySlotsBonus"), count("reactionSlotBonus"))
}

// ---------- one job ----------

/// Units of one material a job needs.
pub fn material_amount(base: i64, runs: i64, me: i64, facility: f64) -> i64 {
    if base <= 0 || runs <= 0 {
        return 0;
    }
    let raw = base as f64 * runs as f64 * (1.0 - me as f64 / 100.0) * facility;
    // EVE rounds to 2 decimals before rounding up, so float noise like 100.0000001 doesn't cost a unit.
    let units = ((raw * 100.0).round() / 100.0).ceil() as i64;
    units.max(runs)
}

/// The estimated item value of one run of a manufacturing or reaction job.
pub fn eiv_per_run(bp: &Blueprint, activity: Activity, adjusted: &HashMap<i64, f64>) -> f64 {
    bp.activity(activity).map(|a| a.materials.iter().map(|(t, q)| adjusted.get(t).copied().unwrap_or(0.0) * *q as f64).sum()).unwrap_or(0.0)
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JobCost {
    pub eiv: f64,
    pub cost_index: f64,
    /// EIV × cost index × the facility's cost bonus.
    pub gross: f64,
    pub facility_tax: f64,
    pub scc: f64,
    pub total: f64,
}

/// The installation cost of a job with estimated item value `eiv`.
pub fn job_cost(eiv: f64, cost_index: f64, cost_bonus: f64, tax: f64, rules: &Rules) -> JobCost {
    let gross = eiv * cost_index * cost_bonus;
    let facility_tax = eiv * tax;
    let scc = eiv * rules.scc_surcharge;
    JobCost { eiv, cost_index, gross, facility_tax, scc, total: gross + facility_tax + scc }
}

pub fn cost_index(indices: Option<&CostIndices>, system: i64, activity: Activity) -> f64 {
    indices.and_then(|i| i.get(&system)).and_then(|m| m.get(activity.index_key())).copied().unwrap_or(0.0)
}

/// One manufacturing or reaction job, worked out.
#[derive(Clone, Debug, Default)]
pub struct JobResult {
    pub blueprint: i64,
    pub activity: Option<Activity>,
    pub runs: i64,
    pub me: i64,
    pub te: i64,
    pub product: i64,
    pub produced: i64,
    /// (type, units)
    pub materials: Vec<(i64, i64)>,
    /// Seconds.
    pub time: f64,
    pub cost: JobCost,
    pub material_multiplier: f64,
    pub time_multiplier: f64,
}

pub struct JobInput<'a> {
    pub blueprint: &'a Blueprint,
    pub runs: i64,
    pub me: i64,
    pub te: i64,
    pub facility: &'a Facility,
    pub levels: &'a Levels,
    pub adjusted: &'a HashMap<i64, f64>,
    pub indices: Option<&'a CostIndices>,
    pub rules: &'a Rules,
}

/// Works out a manufacturing or reaction job (whichever the blueprint has).
pub fn manufacture(input: &JobInput) -> Option<JobResult> {
    let data = &*INDUSTRY;
    let (activity, act, product) = input.blueprint.output()?;
    let mat_mult = input.facility.multiplier(data, activity, BonusKind::Material, product.type_id);
    // Reaction formulas can't be researched: ME and TE don't apply.
    let (me, te) = if activity == Activity::Reaction { (0, 0) } else { (input.me, input.te) };
    let materials = act.materials.iter().map(|(t, q)| (*t, material_amount(*q, input.runs, me, mat_mult))).collect();
    let time_mult = input.facility.multiplier(data, activity, BonusKind::Time, product.type_id)
        * skill_time_multiplier(data, activity, &act.skills, input.levels)
        * (1.0 - te as f64 / 100.0);
    let eiv = eiv_per_run(input.blueprint, activity, input.adjusted) * input.runs as f64;
    let cost_bonus = input.facility.multiplier(data, activity, BonusKind::Cost, product.type_id);
    let index = cost_index(input.indices, input.facility.system_id, activity);
    Some(JobResult {
        blueprint: input.blueprint.id,
        activity: Some(activity),
        runs: input.runs,
        me,
        te,
        product: product.type_id,
        produced: product.quantity * input.runs,
        materials,
        time: act.time as f64 * input.runs as f64 * time_mult,
        cost: job_cost(eiv, index, cost_bonus, input.facility.tax(input.rules), input.rules),
        material_multiplier: mat_mult * (1.0 - me as f64 / 100.0),
        time_multiplier: time_mult,
    })
}

// ---------- invention ----------

#[derive(Clone, Debug, Default)]
pub struct Invention {
    /// The blueprint invented from (T1).
    pub source: i64,
    /// The blueprint invented (T2).
    pub blueprint: i64,
    pub chance: f64,
    /// Runs on each successful copy.
    pub runs: i64,
    pub me: i64,
    pub te: i64,
    /// (type, units) per attempt, decryptor included.
    pub materials: Vec<(i64, i64)>,
    pub decryptor: Option<i64>,
    /// Seconds per attempt.
    pub time: f64,
    /// Job cost per attempt.
    pub cost: JobCost,
}

/// Where and with what an invention job runs.
#[derive(Clone, Copy)]
pub struct InventionEnv<'a> {
    pub facility: &'a Facility,
    pub levels: &'a Levels,
    pub adjusted: &'a HashMap<i64, f64>,
    pub indices: Option<&'a CostIndices>,
    pub rules: &'a Rules,
}

/// Invention of `blueprint` from `source` with a character's skills and an optional decryptor.
pub fn invention(source: &Blueprint, blueprint: &Blueprint, decryptor: Option<i64>, env: &InventionEnv) -> Option<Invention> {
    let data = &*INDUSTRY;
    let InventionEnv { facility, levels, adjusted, indices, rules } = *env;
    let inv = source.activity(Activity::Invention)?;
    let product = inv.products.iter().find(|p| p.type_id == blueprint.id)?;
    let (mut science, mut encryption) = (0.0, 0.0);
    for (skill, _) in &inv.skills {
        let level = *levels.get(skill).unwrap_or(&0) as f64;
        let is_encryption = SKILLS.get(*skill).map(|s| s.name.ends_with("Encryption Methods")).unwrap_or(false);
        if is_encryption {
            encryption += level;
        } else {
            science += level;
        }
    }
    let dec = decryptor.and_then(|d| data.decryptor(d));
    let chance = (product.probability.unwrap_or(0.0)
        * (1.0 + science / rules.invention_science_divisor + encryption / rules.invention_encryption_divisor)
        * dec.map(|d| d.probability).unwrap_or(1.0))
    .min(1.0);
    let mut materials: Vec<(i64, i64)> = inv.materials.clone();
    if let Some(d) = dec {
        materials.push((d.id, 1));
    }
    let (_, _, made) = blueprint.output()?;
    let time_mult = facility.multiplier(data, Activity::Invention, BonusKind::Time, made.type_id)
        * skill_time_multiplier(data, Activity::Invention, &inv.skills, levels);
    let product_eiv = eiv_per_run(blueprint, Activity::Manufacturing, adjusted);
    let eiv = product_eiv * rules.invention_eiv_share;
    let cost_bonus = facility.multiplier(data, Activity::Invention, BonusKind::Cost, made.type_id);
    let index = cost_index(indices, facility.system_id, Activity::Invention);
    Some(Invention {
        source: source.id,
        blueprint: blueprint.id,
        chance,
        runs: (product.quantity + dec.map(|d| d.runs).unwrap_or(0)).max(1),
        me: rules.invented_me + dec.map(|d| d.me).unwrap_or(0),
        te: rules.invented_te + dec.map(|d| d.te).unwrap_or(0),
        materials,
        decryptor,
        time: inv.time as f64 * time_mult,
        cost: job_cost(eiv, index, cost_bonus, facility.tax(rules), rules),
    })
}

// ---------- planner ----------

/// Everything a plan needs besides the targets.
pub struct PlanInput<'a> {
    pub choices: &'a PlanChoices,
    pub doc: &'a IndustryDoc,
    pub levels: &'a Levels,
    /// Best (ME, TE) per blueprint among the characters' originals; blueprints in here count as owned.
    pub owned: &'a HashMap<i64, (i64, i64)>,
    /// Units of each type the characters have.
    pub stock: &'a HashMap<i64, i64>,
    pub adjusted: &'a HashMap<i64, f64>,
    pub indices: Option<&'a CostIndices>,
    pub rules: &'a Rules,
}

/// One item the plan builds, with all of its jobs.
#[derive(Clone, Debug, Default)]
pub struct Step {
    pub product: i64,
    pub blueprint: i64,
    pub activity: Option<Activity>,
    /// How many levels below a target (0 = a target).
    pub depth: usize,
    pub needed: i64,
    pub runs: i64,
    /// Runs per job, after splitting long builds (and invented copies' run limits).
    pub jobs: Vec<i64>,
    pub me: i64,
    pub te: i64,
    pub produced: i64,
    /// (type, units) for all jobs together.
    pub materials: Vec<(i64, i64)>,
    /// Seconds per job, in the same order as `jobs`.
    pub job_times: Vec<f64>,
    pub job_cost: f64,
    pub facility: String,
    /// Whether one of the characters has an original of the blueprint.
    pub owned: bool,
}

#[derive(Clone, Debug, Default)]
pub struct InventionStep {
    pub invention: Invention,
    /// Successful copies needed.
    pub copies: i64,
    /// Expected attempts (copies / chance, rounded up).
    pub attempts: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Purchase {
    pub type_id: i64,
    pub needed: i64,
    /// Taken from the characters' stock.
    pub from_stock: i64,
    pub to_buy: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    /// Deepest first: the order to run them in.
    pub steps: Vec<Step>,
    pub inventions: Vec<InventionStep>,
    pub purchases: Vec<Purchase>,
    /// Targets nothing in the game can build.
    pub unbuildable: Vec<i64>,
}

impl Plan {
    pub fn job_count(&self) -> usize {
        self.steps.iter().map(|s| s.jobs.len()).sum()
    }
    pub fn job_costs(&self) -> f64 {
        self.steps.iter().map(|s| s.job_cost).sum::<f64>() + self.inventions.iter().map(|i| i.invention.cost.total * i.attempts as f64).sum::<f64>()
    }
    /// Cost of everything to buy at `prices` (types without a price are left out and listed).
    pub fn purchase_cost(&self, prices: &HashMap<i64, f64>) -> (f64, Vec<i64>) {
        let mut missing = Vec::new();
        let mut total = 0.0;
        for p in &self.purchases {
            match prices.get(&p.type_id) {
                Some(price) => total += price * p.to_buy as f64,
                None if p.to_buy > 0 => missing.push(p.type_id),
                None => {}
            }
        }
        (total, missing)
    }
    /// The sum of job durations per kind of slot: (manufacturing, science, reactions) seconds.
    pub fn slot_time(&self) -> (f64, f64, f64) {
        let mut t = (0.0, 0.0, 0.0);
        for s in &self.steps {
            let sum: f64 = s.job_times.iter().sum();
            match s.activity {
                Some(Activity::Reaction) => t.2 += sum,
                _ => t.0 += sum,
            }
        }
        for i in &self.inventions {
            t.1 += i.invention.time * i.attempts as f64;
        }
        t
    }
}

/// How long jobs take on `slots` parallel slots: each job goes to the slot that frees up first, longest first.
pub fn wall_time(mut jobs: Vec<f64>, slots: usize) -> f64 {
    if jobs.is_empty() || slots == 0 {
        return 0.0;
    }
    jobs.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let mut free = vec![0.0f64; slots];
    for j in jobs {
        let i = free.iter().enumerate().min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal)).map(|(i, _)| i).unwrap_or(0);
        free[i] += j;
    }
    free.into_iter().fold(0.0, f64::max)
}

/// Whether the plan builds `type_id` (rather than buying it).
fn builds(data: &IndustryData, choices: &PlanChoices, type_id: i64, is_target: bool) -> Option<(i64, Activity)> {
    let (bp, activity) = data.maker(type_id)?;
    let wanted = match choices.overrides.get(&type_id) {
        Some(b) => *b,
        None if is_target => true,
        None => match activity {
            Activity::Reaction => choices.build_reactions,
            _ => choices.build_components,
        },
    };
    wanted.then_some((bp.id, activity))
}

/// Plans building `targets` ((type, units)): every job level by level, inventions for T2 blueprints, and what's
/// left to buy.
pub fn plan(targets: &[(i64, i64)], input: &PlanInput) -> Plan {
    let data = &*INDUSTRY;
    let choices = input.choices;
    let target_set: HashSet<i64> = targets.iter().map(|t| t.0).collect();
    let mut out = Plan::default();

    // How deep each built item sits, so that every item is planned only after everything that uses it.
    let mut depth: HashMap<i64, usize> = HashMap::new();
    fn walk(data: &IndustryData, choices: &PlanChoices, t: i64, d: usize, targets: &HashSet<i64>, depth: &mut HashMap<i64, usize>, stack: &mut Vec<i64>) {
        // Guards against loops in the data and absurd chains.
        if stack.contains(&t) || d > 24 {
            return;
        }
        let Some((bp, activity)) = builds(data, choices, t, d == 0 && targets.contains(&t)) else { return };
        if depth.get(&t).is_some_and(|old| *old >= d) {
            return;
        }
        depth.insert(t, d);
        stack.push(t);
        if let Some(a) = data.blueprint(bp).and_then(|b| b.activity(activity)) {
            for (m, _) in &a.materials {
                walk(data, choices, *m, d + 1, targets, depth, stack);
            }
        }
        stack.pop();
    }
    for (t, _) in targets {
        walk(data, choices, *t, 0, &target_set, &mut depth, &mut Vec::new());
    }
    for (t, _) in targets {
        if !depth.contains_key(t) {
            out.unbuildable.push(*t);
        }
    }

    let mut need: HashMap<i64, i64> = HashMap::new();
    for (t, q) in targets {
        *need.entry(*t).or_default() += q;
    }
    let mut stock: HashMap<i64, i64> = if choices.use_stock { input.stock.clone() } else { HashMap::new() };
    let mut take_stock = |t: i64, n: i64, is_target: bool| -> i64 {
        // Finished targets in a hangar are usually for sale, not for this build: stock only covers inputs.
        if is_target {
            return 0;
        }
        let have = stock.entry(t).or_default();
        let used = (*have).min(n).max(0);
        *have -= used;
        used
    };

    let mut order: Vec<(usize, i64)> = depth.iter().map(|(t, d)| (*d, *t)).collect();
    order.sort();
    for (d, t) in order {
        let total = need.get(&t).copied().unwrap_or(0);
        if total <= 0 {
            continue;
        }
        let is_target = target_set.contains(&t) && d == 0;
        let from_stock = take_stock(t, total, is_target);
        let needed = total - from_stock;
        if from_stock > 0 {
            out.purchases.push(Purchase { type_id: t, needed: total, from_stock, to_buy: 0 });
        }
        if needed <= 0 {
            continue;
        }
        let Some((bp_id, activity)) = builds(data, choices, t, is_target) else { continue };
        let Some(bp) = data.blueprint(bp_id) else { continue };
        let Some(act) = bp.activity(activity) else { continue };
        let per_run = act.products.first().map(|p| p.quantity).unwrap_or(1).max(1);
        let runs = (needed + per_run - 1) / per_run;
        let facility = input.doc.facility_for(activity);

        // T2 and other invented blueprints: unless an original is owned, copies come from invention.
        let owned = input.owned.get(&bp_id).copied();
        let sources = data.invention_sources(bp_id);
        let invented = if owned.is_none() && choices.invent && activity == Activity::Manufacturing {
            sources.first().and_then(|src| {
                let inv_fac = input.doc.facility_for(Activity::Invention);
                let env = InventionEnv { facility: &inv_fac, levels: input.levels, adjusted: input.adjusted, indices: input.indices, rules: input.rules };
                invention(src, bp, choices.decryptor, &env)
            })
        } else {
            None
        };
        let (me, te) = match (&invented, owned) {
            (Some(inv), _) => (inv.me, inv.te),
            (None, Some(o)) => o,
            (None, None) if sources.is_empty() => (input.rules.assumed_me, input.rules.assumed_te),
            // an invented blueprint nobody owns or invents here: assume a typical invented copy
            (None, None) => (input.rules.invented_me, input.rules.invented_te),
        };

        // Split into jobs: invented copies carry a few runs each; long builds are split at max_job_days.
        let probe = JobInput { blueprint: bp, runs: 1, me, te, facility: &facility, levels: input.levels, adjusted: input.adjusted, indices: input.indices, rules: input.rules };
        let per_run_time = manufacture(&probe).map(|j| j.time).unwrap_or(act.time as f64).max(1.0);
        let by_time = ((choices.max_job_days * 86400.0) / per_run_time).floor().max(1.0) as i64;
        let max_per_job = match &invented {
            Some(inv) => inv.runs.min(by_time),
            None => by_time,
        };
        let mut jobs = Vec::new();
        let mut left = runs;
        while left > 0 {
            let r = left.min(max_per_job);
            jobs.push(r);
            left -= r;
        }
        let mut materials: BTreeMap<i64, i64> = BTreeMap::new();
        let mut job_times = Vec::new();
        let mut job_cost = 0.0;
        for r in &jobs {
            let job = JobInput { runs: *r, ..probe };
            if let Some(j) = manufacture(&job) {
                for (m, q) in j.materials {
                    *materials.entry(m).or_default() += q;
                }
                job_times.push(j.time);
                job_cost += j.cost.total;
            }
        }
        for (m, q) in &materials {
            *need.entry(*m).or_default() += q;
        }
        if let Some(inv) = invented {
            let copies = (runs + inv.runs - 1) / inv.runs;
            let attempts = if inv.chance > 0.0 { (copies as f64 / inv.chance).ceil() as i64 } else { 0 };
            for (m, q) in &inv.materials {
                *need.entry(*m).or_default() += q * attempts;
            }
            out.inventions.push(InventionStep { invention: inv, copies, attempts });
        }
        out.steps.push(Step {
            product: t,
            blueprint: bp_id,
            activity: Some(activity),
            depth: d,
            needed,
            runs,
            jobs,
            me,
            te,
            produced: runs * per_run,
            materials: materials.into_iter().collect(),
            job_times,
            job_cost,
            facility: facility.label(),
            owned: owned.is_some(),
        });
    }

    // Whatever isn't built is bought (minus stock).
    let built: HashSet<i64> = out.steps.iter().map(|s| s.product).collect();
    let mut buy: Vec<(i64, i64)> = need.into_iter().filter(|(t, q)| *q > 0 && !built.contains(t) && !out.unbuildable.contains(t)).collect();
    buy.sort();
    for (t, q) in buy {
        let from_stock = take_stock(t, q, false);
        out.purchases.push(Purchase { type_id: t, needed: q, from_stock, to_buy: q - from_stock });
    }
    out.purchases.sort_by_key(|p| std::cmp::Reverse(p.to_buy));
    // deepest first: reactions, then components, then the targets
    out.steps.sort_by(|a, b| b.depth.cmp(&a.depth).then_with(|| data.type_name(a.product).cmp(&data.type_name(b.product))));
    out
}

/// Best (ME, TE) per blueprint type among the characters' originals.
pub fn owned_originals(characters: &[std::sync::Arc<Character>]) -> HashMap<i64, (i64, i64)> {
    let mut out: HashMap<i64, (i64, i64)> = HashMap::new();
    for c in characters {
        for b in c.blueprints.iter().flat_map(|d| &d.blueprints) {
            if b.is_copy() {
                continue;
            }
            let e = out.entry(b.type_id).or_insert((b.material_efficiency, b.time_efficiency));
            if (b.material_efficiency, b.time_efficiency) > *e {
                *e = (b.material_efficiency, b.time_efficiency);
            }
        }
    }
    out
}

/// Units of every type in the characters' assets.
pub fn stock(characters: &[std::sync::Arc<Character>]) -> HashMap<i64, i64> {
    let mut out: HashMap<i64, i64> = HashMap::new();
    for c in characters {
        for a in c.assets.iter().flat_map(|d| &d.items) {
            if !a.is_blueprint_copy {
                *out.entry(a.type_id).or_default() += a.quantity;
            }
        }
    }
    out
}

/// Text for EVE's multibuy window: "Name<TAB>quantity" per line.
pub fn multibuy(purchases: &[Purchase]) -> String {
    purchases.iter().filter(|p| p.to_buy > 0).map(|p| format!("{}\t{}", INDUSTRY.type_name(p.type_id), p.to_buy)).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_prices() -> HashMap<i64, f64> {
        HashMap::new()
    }

    #[test]
    fn material_rounding() {
        assert_eq!(material_amount(1, 10, 10, 1.0), 10); // never less than one per run
        assert_eq!(material_amount(100, 1, 10, 1.0), 90);
        assert_eq!(material_amount(100, 10, 10, 0.99), 891);
        assert_eq!(material_amount(0, 10, 10, 1.0), 0);
        // 333 × 3 × 0.9 = 899.1 → 900
        assert_eq!(material_amount(333, 3, 10, 1.0), 900);
    }

    #[test]
    fn job_costs_and_facilities() {
        let rules = Rules::default();
        let c = job_cost(1_000_000.0, 0.05, 0.97, 0.0, &rules);
        assert!((c.gross - 48_500.0).abs() < 1e-6);
        assert!((c.scc - 40_000.0).abs() < 1e-6);
        assert!((c.total - 88_500.0).abs() < 1e-6);
        let data = &*INDUSTRY;
        // A Raitaru with a T1 medium ship ME rig in low sec: 0.99 × (1 − 0.02 × 1.9)
        let tama = 30002813; // Tama, 0.3
        assert_eq!(MAP.system(tama).unwrap().security_class(), SecurityClass::Low);
        let f = Facility { structure: Some(35825), rigs: vec![37146], system_id: tama, ..Default::default() };
        let thorax = 627; // a cruiser (Medium T1 ship)
        let m = f.multiplier(data, Activity::Manufacturing, BonusKind::Material, thorax);
        assert!((m - 0.99 * (1.0 - 0.02 * 1.9)).abs() < 1e-9, "{m}");
        // the rig doesn't help frigates
        let m = f.multiplier(data, Activity::Manufacturing, BonusKind::Material, 587);
        assert!((m - 0.99).abs() < 1e-9);
        assert!(!Facility::npc(30000142).supports(data, Activity::Reaction));
    }

    #[test]
    fn skills_and_slots() {
        let data = &*INDUSTRY;
        let mut levels = Levels::new();
        levels.insert(3380, 5); // Industry
        levels.insert(3388, 5); // Advanced Industry
        let m = skill_time_multiplier(data, Activity::Manufacturing, &[], &levels);
        assert!((m - 0.8 * 0.85).abs() < 1e-9, "{m}");
        levels.insert(3387, 5); // Mass Production
        levels.insert(24625, 5);
        assert_eq!(slots(data, &levels).0, 11);
    }

    #[test]
    fn rifter_job() {
        let data = &*INDUSTRY;
        let bp = data.blueprint(691).unwrap();
        let f = Facility::npc(30000142);
        let rules = Rules::default();
        let levels = Levels::new();
        let adjusted = no_prices();
        let j = manufacture(&JobInput { blueprint: bp, runs: 10, me: 10, te: 20, facility: &f, levels: &levels, adjusted: &adjusted, indices: None, rules: &rules }).unwrap();
        assert_eq!(j.product, 587);
        assert_eq!(j.produced, 10);
        let base_trit = bp.activity(Activity::Manufacturing).unwrap().materials.iter().find(|m| m.0 == 34).unwrap().1;
        let trit = j.materials.iter().find(|m| m.0 == 34).unwrap().1;
        assert_eq!(trit, material_amount(base_trit, 10, 10, 1.0));
        assert!((j.time - bp.activity(Activity::Manufacturing).unwrap().time as f64 * 10.0 * 0.8).abs() < 1e-6);
    }

    #[test]
    fn plans_t2_with_invention() {
        let rules = Rules::default();
        let doc = IndustryDoc::default();
        let choices = PlanChoices::default();
        let levels = all_five();
        let empty = HashMap::new();
        let stock = HashMap::new();
        let adjusted = no_prices();
        let input = PlanInput { choices: &choices, doc: &doc, levels: &levels, owned: &empty, stock: &stock, adjusted: &adjusted, indices: None, rules: &rules };
        // 3 Jaguars: invented copies, a Rifter per Jaguar built from minerals, T2 components bought
        let p = plan(&[(11400, 3)], &input);
        assert!(p.unbuildable.is_empty());
        let jaguar = p.steps.iter().find(|s| s.product == 11400).unwrap();
        assert_eq!(jaguar.runs, 3);
        assert_eq!(jaguar.me, rules.invented_me);
        let inv = &p.inventions[0];
        assert!(inv.invention.chance > 0.3 && inv.invention.chance < 1.0, "{}", inv.invention.chance);
        assert!(inv.attempts >= inv.copies);
        assert!(p.steps.iter().any(|s| s.product == 587 && s.depth == 1), "rifter built");
        assert!(p.purchases.iter().any(|b| b.type_id == 34 && b.to_buy > 0), "tritanium bought");
        assert!(multibuy(&p.purchases).contains("Tritanium\t"));
        // with stock, owned tritanium leaves the shopping list
        let mut stock = HashMap::new();
        stock.insert(34, 1_000_000_000);
        let choices = PlanChoices { use_stock: true, ..PlanChoices::default() };
        let input = PlanInput { choices: &choices, stock: &stock, ..input };
        let p = plan(&[(11400, 3)], &input);
        assert!(p.purchases.iter().any(|b| b.type_id == 34 && b.to_buy == 0 && b.from_stock > 0));
    }

    #[test]
    fn splits_long_jobs() {
        assert_eq!(wall_time(vec![10.0, 10.0, 10.0, 10.0], 2), 20.0);
        assert_eq!(wall_time(vec![30.0, 10.0, 10.0, 10.0], 2), 30.0);
        let rules = Rules::default();
        let doc = IndustryDoc::default();
        let choices = PlanChoices { max_job_days: 0.5, ..PlanChoices::default() };
        let levels = Levels::new();
        let empty = HashMap::new();
        let no_stock = HashMap::new();
        let adjusted = no_prices();
        let input = PlanInput { choices: &choices, doc: &doc, levels: &levels, owned: &empty, stock: &no_stock, adjusted: &adjusted, indices: None, rules: &rules };
        let p = plan(&[(587, 200)], &input);
        let rifter = p.steps.iter().find(|s| s.product == 587).unwrap();
        assert!(rifter.jobs.len() > 1);
        assert_eq!(rifter.jobs.iter().sum::<i64>(), 200);
    }
}
