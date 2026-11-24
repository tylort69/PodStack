//! Kills and losses of all the user's characters together.
//!
//! ESI lists a character's killmails for 90 days only, so every killmail PodStack sees is kept whole (victim, items,
//! attackers) in an archive (`killmail_archive.json`) that grows from the first refresh on. Each mail is valued once,
//! when it's archived, at CCP's average prices of that day ([`value`]), so old fights don't move with today's market.
//!
//! [`entries`] turns the archive into what each mail meant for the user: a kill (one of their characters is among
//! the attackers) or a loss (one of them is the victim). A mail several of the user's characters were on is one
//! entry listing all of them, so totals count it once. [`analyze`] adds those entries up for a period: headline
//! numbers, the time series, and the breakdowns by character, ship flown, victim class, place, enemy and hour.

use crate::domains::Cx;
use crate::features::wallet_flow::{self, BucketSize};
use crate::sde::{MAP, MARKET, SHIPS};
use crate::{store, Core, Event};
use chrono::{DateTime, Datelike, NaiveDate, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

pub const DOC: &str = "killmail_archive";
pub const SCOPE: &str = "esi-killmails.read_killmails.v1";

// ---------- ESI's killmail ----------

/// An item on a killmail: fitted, in a hold, or (for containers) inside another item.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct KillItem {
    pub item_type_id: i64,
    #[serde(default)]
    pub flag: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_destroyed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity_dropped: Option<i64>,
    /// 2 for a blueprint copy.
    #[serde(default)]
    pub singleton: i64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<KillItem>,
}

impl KillItem {
    pub fn destroyed(&self) -> i64 {
        self.quantity_destroyed.unwrap_or(0)
    }
    pub fn dropped(&self) -> i64 {
        self.quantity_dropped.unwrap_or(0)
    }
    pub fn is_blueprint_copy(&self) -> bool {
        self.singleton == 2
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Victim {
    #[serde(default)]
    pub character_id: Option<i64>,
    #[serde(default)]
    pub corporation_id: Option<i64>,
    #[serde(default)]
    pub alliance_id: Option<i64>,
    #[serde(default)]
    pub faction_id: Option<i64>,
    pub ship_type_id: i64,
    #[serde(default)]
    pub damage_taken: i64,
    #[serde(default)]
    pub items: Vec<KillItem>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Attacker {
    #[serde(default)]
    pub character_id: Option<i64>,
    #[serde(default)]
    pub corporation_id: Option<i64>,
    #[serde(default)]
    pub alliance_id: Option<i64>,
    #[serde(default)]
    pub faction_id: Option<i64>,
    #[serde(default)]
    pub ship_type_id: Option<i64>,
    #[serde(default)]
    pub weapon_type_id: Option<i64>,
    #[serde(default)]
    pub damage_done: i64,
    #[serde(default)]
    pub final_blow: bool,
    #[serde(default)]
    pub security_status: f64,
}

impl Attacker {
    /// NPCs have no character.
    pub fn is_npc(&self) -> bool {
        self.character_id.is_none()
    }
}

/// `GET /killmails/{id}/{hash}`, kept as ESI sends it.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct EsiKillmail {
    pub killmail_id: i64,
    pub killmail_time: DateTime<Utc>,
    pub solar_system_id: i64,
    pub victim: Victim,
    #[serde(default)]
    pub attackers: Vec<Attacker>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub war_id: Option<i64>,
}

impl EsiKillmail {
    pub fn final_blow(&self) -> Option<&Attacker> {
        self.attackers.iter().find(|a| a.final_blow).or(self.attackers.first())
    }

    /// Every id the mail shows a name for: pilots, corporations, alliances, factions, ships, weapons, items and
    /// the system.
    pub fn name_ids(&self) -> Vec<i64> {
        let v = &self.victim;
        let mut ids: Vec<i64> = [v.character_id, v.corporation_id, v.alliance_id, v.faction_id, Some(v.ship_type_id), Some(self.solar_system_id)]
            .into_iter()
            .flatten()
            .collect();
        for a in &self.attackers {
            ids.extend([a.character_id, a.corporation_id, a.alliance_id, a.faction_id, a.ship_type_id, a.weapon_type_id].into_iter().flatten());
        }
        fn items(list: &[KillItem], out: &mut Vec<i64>) {
            for i in list {
                out.push(i.item_type_id);
                items(&i.items, out);
            }
        }
        items(&v.items, &mut ids);
        ids.sort_unstable();
        ids.dedup();
        ids
    }
}

// ---------- valuation ----------

/// What a mail was worth, at the prices of the day it was archived. The hull counts as destroyed.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct KillValue {
    pub at: DateTime<Utc>,
    pub hull: f64,
    /// Hull plus the items destroyed.
    pub destroyed: f64,
    pub dropped: f64,
    /// Types with no price (counted as nothing above).
    #[serde(default)]
    pub unpriced: u32,
}

impl KillValue {
    pub fn total(&self) -> f64 {
        self.destroyed + self.dropped
    }
}

/// Values a mail with `prices` (type id → ISK). Blueprint copies count as nothing: they have no market, and their
/// original's price would be wildly wrong.
pub fn value(mail: &EsiKillmail, prices: &HashMap<i64, f64>, at: DateTime<Utc>) -> KillValue {
    let mut unpriced: HashSet<i64> = HashSet::new();
    let mut price = |t: i64| match prices.get(&t) {
        Some(p) => *p,
        None => {
            unpriced.insert(t);
            0.0
        }
    };
    let hull = price(mail.victim.ship_type_id);
    let mut v = KillValue { at, hull, destroyed: hull, ..Default::default() };
    fn walk(list: &[KillItem], price: &mut dyn FnMut(i64) -> f64, v: &mut KillValue) {
        for i in list {
            if !i.is_blueprint_copy() {
                let p = price(i.item_type_id);
                v.destroyed += p * i.destroyed() as f64;
                v.dropped += p * i.dropped() as f64;
            }
            walk(&i.items, price, v);
        }
    }
    walk(&mail.victim.items, &mut price, &mut v);
    v.unpriced = unpriced.len() as u32;
    v
}

// ---------- archive ----------

/// A killmail as archived: ESI's record, where it happened, which of the user's characters listed it, and its
/// value.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ArchivedKill {
    pub mail: EsiKillmail,
    pub hash: String,
    pub system: String,
    #[serde(default)]
    pub security: Option<f64>,
    #[serde(default)]
    pub region_id: Option<i64>,
    #[serde(default)]
    pub region: Option<String>,
    /// The user's characters whose recent killmails listed it.
    #[serde(default)]
    pub seen_by: BTreeSet<i64>,
    pub first_seen: DateTime<Utc>,
    #[serde(default)]
    pub value: Option<KillValue>,
}

impl ArchivedKill {
    pub fn id(&self) -> i64 {
        self.mail.killmail_id
    }
    pub fn time(&self) -> DateTime<Utc> {
        self.mail.killmail_time
    }
    pub fn total(&self) -> f64 {
        self.value.as_ref().map(|v| v.total()).unwrap_or(0.0)
    }
    pub fn band(&self) -> Band {
        band(self.mail.solar_system_id, self.region_id, self.security)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Archive {
    pub kills: BTreeMap<i64, ArchivedKill>,
    /// Names of the pilots, corporations, alliances, factions, types and systems the mails show.
    #[serde(default)]
    pub names: BTreeMap<i64, String>,
}

impl Archive {
    pub fn get(&self, id: i64) -> Option<&ArchivedKill> {
        self.kills.get(&id)
    }
    pub fn contains(&self, id: i64) -> bool {
        self.kills.contains_key(&id)
    }
    /// The name of a pilot, corporation, alliance, faction, type or system, if known.
    pub fn name(&self, id: i64) -> Option<&str> {
        self.names.get(&id).map(|s| s.as_str()).or_else(|| MARKET.get(id).map(|t| t.name.as_str()))
    }
    /// A type's name, else "Type #id".
    pub fn type_name(&self, id: i64) -> String {
        self.name(id).map(str::to_string).or_else(|| SHIPS.by_id.get(&id).map(|s| s.name.clone())).unwrap_or_else(|| format!("Type #{id}"))
    }
    /// The victim's pilot, else the corporation that owned the structure or deployable.
    pub fn victim_name(&self, k: &ArchivedKill) -> String {
        let v = &k.mail.victim;
        v.character_id.or(v.corporation_id).and_then(|i| self.name(i)).map(str::to_string).unwrap_or_else(|| "Unknown".into())
    }
    /// An attacker's pilot, else its corporation or faction (NPCs).
    pub fn attacker_name(&self, a: &Attacker) -> String {
        a.character_id.or(a.corporation_id).or(a.faction_id).and_then(|i| self.name(i)).map(str::to_string).unwrap_or_else(|| "NPC".into())
    }
    /// When the oldest mail happened: how far back the archive goes.
    pub fn oldest(&self) -> Option<DateTime<Utc>> {
        self.kills.values().map(|k| k.time()).min()
    }
}

static ARCHIVE: RwLock<Option<Arc<Archive>>> = RwLock::new(None);
/// Serializes read-modify-write of the archive: several characters refresh at once.
static WRITE: Mutex<()> = Mutex::new(());

/// The archive as it is now (cheap: it's shared, and read from disk only once).
pub fn archive_snapshot() -> Arc<Archive> {
    if let Some(a) = ARCHIVE.read().unwrap().as_ref() {
        return a.clone();
    }
    let mut slot = ARCHIVE.write().unwrap();
    slot.get_or_insert_with(|| Arc::new(store::load_doc::<Archive>(DOC))).clone()
}

/// Adds what one character's refresh found: `listed` are the ids its recent killmails show (marked seen by it),
/// `new` the mails fetched in full that the archive didn't have, `names` what their ids are called.
pub fn archive(core: &Core, character: i64, listed: &[i64], new: Vec<ArchivedKill>, names: HashMap<i64, String>) {
    let _guard = WRITE.lock().unwrap();
    let mut a = (*archive_snapshot()).clone();
    if merge(&mut a, character, listed, new, names) {
        store::save_doc(DOC, &a);
        *ARCHIVE.write().unwrap() = Some(Arc::new(a));
        core.emit(Event::Doc(DOC.into()));
    }
}

/// [`archive`]'s merge; whether anything changed. A mail already archived is kept as it is (killmails never
/// change), only who saw it grows.
fn merge(a: &mut Archive, character: i64, listed: &[i64], new: Vec<ArchivedKill>, names: HashMap<i64, String>) -> bool {
    let mut changed = false;
    for k in new {
        if let std::collections::btree_map::Entry::Vacant(slot) = a.kills.entry(k.id()) {
            slot.insert(k);
            changed = true;
        }
    }
    for id in listed {
        if let Some(k) = a.kills.get_mut(id) {
            changed |= k.seen_by.insert(character);
        }
    }
    for (id, n) in names {
        if a.names.get(&id) != Some(&n) {
            a.names.insert(id, n);
            changed = true;
        }
    }
    changed
}

/// How many mails one refresh fetches in full for the archive beyond the ones the Kill Log tab needs: a busy
/// character's first refresh can list hundreds, which then come in over a few hours instead of all at once.
pub const FETCH_BATCH: usize = 200;

/// Fetches, places and values the mails `wanted` ((id, hash), newest first) the archive doesn't have yet.
pub async fn fetch_new(cx: &Cx, wanted: &[(i64, String)]) -> (Vec<ArchivedKill>, HashMap<i64, String>) {
    let fetched: Vec<(EsiKillmail, String)> =
        futures::future::join_all(wanted.iter().map(|(id, hash)| async move {
            cx.esi().get(format!("killmails/{id}/{hash}")).json::<EsiKillmail>().await.ok().map(|m| (m, hash.clone()))
        }))
        .await
        .into_iter()
        .flatten()
        .collect();
    if fetched.is_empty() {
        return (Vec::new(), HashMap::new());
    }
    let names = cx.universe().names(cx.esi(), fetched.iter().flat_map(|(m, _)| m.name_ids()).collect::<Vec<_>>()).await;
    let prices = cx.core.market.average_prices(cx.esi()).await;
    let now = Utc::now();
    let mut systems = HashMap::new();
    for s in fetched.iter().map(|(m, _)| m.solar_system_id).collect::<HashSet<_>>() {
        if let Ok(info) = cx.universe().system(cx.esi(), s).await {
            systems.insert(s, info);
        }
    }
    let kills = fetched
        .into_iter()
        .map(|(mail, hash)| {
            let info = systems.get(&mail.solar_system_id);
            let known = MAP.system(mail.solar_system_id);
            ArchivedKill {
                value: (!prices.is_empty()).then(|| value(&mail, &prices, now)),
                system: info
                    .map(|i| i.name.clone())
                    .or_else(|| known.map(|s| s.name.clone()))
                    .unwrap_or_else(|| format!("System #{}", mail.solar_system_id)),
                security: info.map(|i| i.display_security()).or_else(|| known.map(|s| s.display_security())),
                region_id: info.map(|i| i.region_id).filter(|r| *r > 0).or_else(|| known.map(|s| s.region)),
                region: info
                    .map(|i| i.region_name.clone())
                    .filter(|r| !r.is_empty())
                    .or_else(|| known.and_then(|s| MAP.region_name(s.region).map(str::to_string))),
                hash,
                mail,
                seen_by: BTreeSet::from([cx.id]),
                first_seen: now,
            }
        })
        .collect();
    (kills, names)
}

/// Values the archived mails that couldn't be valued when archived (prices weren't available then).
pub async fn value_missing(cx: &Cx) {
    let todo: Vec<i64> = archive_snapshot().kills.values().filter(|k| k.value.is_none()).map(|k| k.id()).collect();
    if todo.is_empty() {
        return;
    }
    let prices = cx.core.market.average_prices(cx.esi()).await;
    if prices.is_empty() {
        return;
    }
    let now = Utc::now();
    let _guard = WRITE.lock().unwrap();
    let mut a = (*archive_snapshot()).clone();
    for id in todo {
        if let Some(k) = a.kills.get_mut(&id) {
            k.value = Some(value(&k.mail, &prices, now));
        }
    }
    store::save_doc(DOC, &a);
    *ARCHIVE.write().unwrap() = Some(Arc::new(a));
    cx.core.emit(Event::Doc(DOC.into()));
}

// ---------- places and ship classes ----------

/// Where a fight happened, by the rules that apply there.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Band {
    High,
    Low,
    Null,
    Pochven,
    Wormhole,
    Abyssal,
}

impl Band {
    pub const ALL: [Band; 6] = [Band::High, Band::Low, Band::Null, Band::Pochven, Band::Wormhole, Band::Abyssal];
    pub fn label(self) -> &'static str {
        match self {
            Band::High => "High-sec",
            Band::Low => "Low-sec",
            Band::Null => "Null-sec",
            Band::Pochven => "Pochven",
            Band::Wormhole => "W-space",
            Band::Abyssal => "Abyssal",
        }
    }
}

const POCHVEN: i64 = 10000070;

/// The band of a system: wormhole and abyssal systems by their id range, Pochven by its region, the rest by
/// security as the game shows it.
pub fn band(system_id: i64, region_id: Option<i64>, security: Option<f64>) -> Band {
    if (31_000_000..32_000_000).contains(&system_id) {
        return Band::Wormhole;
    }
    if system_id >= 32_000_000 {
        return Band::Abyssal;
    }
    let region = region_id.or_else(|| MAP.system(system_id).map(|s| s.region));
    if region == Some(POCHVEN) {
        return Band::Pochven;
    }
    let sec = security.or_else(|| MAP.system(system_id).map(|s| s.display_security())).unwrap_or(-1.0);
    if sec >= 0.5 {
        Band::High
    } else if sec > 0.0 {
        Band::Low
    } else {
        Band::Null
    }
}

/// A hull's class and group: "Frigates" and "Assault Frigate", "Structures" and "Citadel". The class is the
/// market's second level under Ships (else its top level); the group is the type's group.
pub fn ship_class(type_id: i64) -> (String, String) {
    let ship = SHIPS.by_id.get(&type_id);
    let market = MARKET.get(type_id);
    let path: Vec<String> = market.map(|t| MARKET.path(t.group).iter().map(|g| g.name.clone()).collect()).unwrap_or_default();
    let group = ship.map(|s| s.group.clone()).or_else(|| path.last().cloned()).unwrap_or_else(|| "Other".into());
    let class = match path.first().map(|s| s.as_str()) {
        Some("Ships") => path.get(1).cloned(),
        Some(_) => path.first().cloned(),
        None => None,
    }
    .unwrap_or_else(|| group.clone());
    (class, group)
}

/// Where an item sat on the ship, from its inventory flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Slot {
    High,
    Mid,
    Low,
    Rig,
    Subsystem,
    Service,
    Drones,
    Fighters,
    Implants,
    Cargo,
    Other,
}

impl Slot {
    pub fn label(self) -> &'static str {
        match self {
            Slot::High => "High slots",
            Slot::Mid => "Mid slots",
            Slot::Low => "Low slots",
            Slot::Rig => "Rigs",
            Slot::Subsystem => "Subsystems",
            Slot::Service => "Service slots",
            Slot::Drones => "Drone bay",
            Slot::Fighters => "Fighters",
            Slot::Implants => "Implants",
            Slot::Cargo => "Cargo",
            Slot::Other => "Other holds",
        }
    }
}

pub fn slot(flag: i64) -> Slot {
    match flag {
        27..=34 => Slot::High,
        19..=26 => Slot::Mid,
        11..=18 => Slot::Low,
        92..=99 => Slot::Rig,
        125..=132 => Slot::Subsystem,
        164..=171 => Slot::Service,
        87 => Slot::Drones,
        158..=163 => Slot::Fighters,
        89 => Slot::Implants,
        5 => Slot::Cargo,
        _ => Slot::Other,
    }
}

/// The victim's items by where they sat, racks first (in the game's order), then bays and holds.
pub fn fit(mail: &EsiKillmail) -> Vec<(Slot, Vec<&KillItem>)> {
    let mut by: BTreeMap<Slot, Vec<&KillItem>> = BTreeMap::new();
    for i in &mail.victim.items {
        by.entry(slot(i.flag)).or_default().push(i);
    }
    for list in by.values_mut() {
        list.sort_by_key(|i| (i.flag, i.item_type_id));
    }
    by.into_iter().collect()
}

// ---------- what each mail meant for the user ----------

/// A kill or a loss of the user's: one per mail, however many of their characters were on it.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub time: DateTime<Utc>,
    /// One of the user's characters died; the mail isn't counted as a kill then, even if another of them shot it.
    pub loss: bool,
    /// The user's characters on the mail: the victim, or the attackers (by damage done).
    pub chars: Vec<i64>,
    /// The user's character that landed the final blow.
    pub final_blow: Option<i64>,
    /// A kill by one of the user's characters with no other player on it (NPCs may help).
    pub solo: bool,
    /// The hulls the user's characters flew: (character, ship type).
    pub ships: Vec<(i64, i64)>,
    pub value: f64,
    pub valued: bool,
}

impl Entry {
    pub fn shared(&self) -> bool {
        self.chars.len() > 1
    }
}

/// What a mail meant for the user, whose characters are `mine`; None if none of them is on it.
pub fn entry(k: &ArchivedKill, mine: &HashSet<i64>) -> Option<Entry> {
    let m = &k.mail;
    let is_mine = |id: Option<i64>| id.is_some_and(|i| mine.contains(&i));
    let mut attackers: Vec<&Attacker> = m.attackers.iter().filter(|a| is_mine(a.character_id)).collect();
    attackers.sort_by_key(|a| std::cmp::Reverse(a.damage_done));
    let loss = is_mine(m.victim.character_id);
    if !loss && attackers.is_empty() {
        return None;
    }
    let (chars, ships) = if loss {
        let c = m.victim.character_id.unwrap_or_default();
        (vec![c], vec![(c, m.victim.ship_type_id)])
    } else {
        let chars: Vec<i64> = attackers.iter().filter_map(|a| a.character_id).collect();
        let ships = attackers.iter().filter_map(|a| Some((a.character_id?, a.ship_type_id?))).collect();
        (chars, ships)
    };
    let players = m.attackers.iter().filter(|a| !a.is_npc()).count();
    Some(Entry {
        id: m.killmail_id,
        time: m.killmail_time,
        loss,
        final_blow: if loss { None } else { m.attackers.iter().find(|a| a.final_blow && is_mine(a.character_id)).and_then(|a| a.character_id) },
        solo: !loss && players == 1,
        chars,
        ships,
        value: k.total(),
        valued: k.value.is_some(),
    })
}

/// The user's kills and losses since `since` (None: all of the archive), newest first.
pub fn entries(a: &Archive, mine: &HashSet<i64>, since: Option<DateTime<Utc>>) -> Vec<Entry> {
    let mut out: Vec<Entry> = a.kills.values().filter(|k| since.is_none_or(|s| k.time() >= s)).filter_map(|k| entry(k, mine)).collect();
    out.sort_by(|x, y| y.time.cmp(&x.time).then(y.id.cmp(&x.id)));
    out
}

// ---------- totals ----------

/// Kills and losses, and the ISK on each side.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tally {
    pub kills: u32,
    pub losses: u32,
    pub destroyed: f64,
    pub lost: f64,
}

impl Tally {
    fn add(&mut self, e: &Entry) {
        if e.loss {
            self.losses += 1;
            self.lost += e.value;
        } else {
            self.kills += 1;
            self.destroyed += e.value;
        }
    }
    pub fn count(&self) -> u32 {
        self.kills + self.losses
    }
    /// ISK destroyed as a share of all ISK on the mails, 0-1; None with no ISK either way.
    pub fn efficiency(&self) -> Option<f64> {
        let all = self.destroyed + self.lost;
        (all > 0.0).then(|| self.destroyed / all)
    }
}

/// The headline numbers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    pub tally: Tally,
    pub solo: u32,
    pub gang: u32,
    /// Kills where one of the user's characters landed the final blow.
    pub final_blows: u32,
    /// Mails more than one of the user's characters were on.
    pub shared: u32,
    /// Mails not valued yet (counted as nothing).
    pub unvalued: u32,
}

/// One row of a breakdown: a ship, a class, a system, an enemy, … `id` is the type, system, region or
/// corporation/alliance when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub label: String,
    pub id: Option<i64>,
    pub tally: Tally,
}

/// One character's numbers: a shared mail counts for each of the user's characters on it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CharacterStats {
    pub char_id: i64,
    pub tally: Tally,
    pub solo: u32,
    pub final_blows: u32,
    pub shared: u32,
    /// The hull it flew most on these mails.
    pub top_ship: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bucket {
    pub start: NaiveDate,
    pub tally: Tally,
}

#[derive(Clone, Debug, Default)]
pub struct Analysis {
    pub totals: Totals,
    pub buckets: Vec<Bucket>,
    pub bucket_size: Option<BucketSize>,
    pub characters: Vec<CharacterStats>,
    /// The hulls the user's characters flew; a shared kill counts once per hull on it.
    pub my_ships: Vec<Group>,
    /// Victims' classes and groups: kills are the user's kills of that class, losses the user's own.
    pub victim_classes: Vec<Group>,
    pub victim_groups: Vec<Group>,
    pub systems: Vec<Group>,
    pub regions: Vec<Group>,
    /// Every band, in [`Band::ALL`]'s order, with or without mails.
    pub bands: Vec<(Band, Tally)>,
    /// Kills: the victim's corporation (alliance); losses: each corporation (alliance) of a pilot or NPC on the
    /// mail, once per mail. The user's own characters don't count as enemies.
    pub enemy_corps: Vec<Group>,
    pub enemy_alliances: Vec<Group>,
    /// By hour of the day and weekday (Monday first), EVE time.
    pub hours: [Tally; 24],
    pub weekdays: [Tally; 7],
    /// The ten most valuable kills and losses, by mail id.
    pub top_kills: Vec<i64>,
    pub top_losses: Vec<i64>,
    /// The oldest mail counted.
    pub first: Option<DateTime<Utc>>,
}

fn grouped(map: HashMap<(String, Option<i64>), Tally>) -> Vec<Group> {
    let mut v: Vec<Group> = map.into_iter().map(|((label, id), tally)| Group { label, id, tally }).collect();
    v.sort_by(|a, b| {
        b.tally
            .count()
            .cmp(&a.tally.count())
            .then((b.tally.destroyed + b.tally.lost).total_cmp(&(a.tally.destroyed + a.tally.lost)))
            .then(a.label.cmp(&b.label))
    });
    v
}

/// Adds up `entries` (from [`entries`]) for the period from `since` (None: from the oldest) to `now`. `chars`
/// lists the user's characters, so ones without mails still get a row.
pub fn analyze(a: &Archive, entries: &[Entry], chars: &[i64], since: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Analysis {
    let mut out = Analysis::default();
    let mine: HashSet<i64> = chars.iter().copied().collect();
    let mut per_char: HashMap<i64, (CharacterStats, HashMap<i64, u32>)> =
        chars.iter().map(|id| (*id, (CharacterStats { char_id: *id, ..Default::default() }, HashMap::new()))).collect();
    let mut ships: HashMap<(String, Option<i64>), Tally> = HashMap::new();
    let mut classes: HashMap<(String, Option<i64>), Tally> = HashMap::new();
    let mut groups: HashMap<(String, Option<i64>), Tally> = HashMap::new();
    let mut systems: HashMap<(String, Option<i64>), Tally> = HashMap::new();
    let mut regions: HashMap<(String, Option<i64>), Tally> = HashMap::new();
    let mut bands: HashMap<Band, Tally> = HashMap::new();
    let mut corps: HashMap<(String, Option<i64>), Tally> = HashMap::new();
    let mut alliances: HashMap<(String, Option<i64>), Tally> = HashMap::new();
    let name = |id: i64| a.name(id).map(str::to_string).unwrap_or_else(|| format!("#{id}"));

    for e in entries {
        let Some(k) = a.get(e.id) else { continue };
        let m = &k.mail;
        let t = &mut out.totals;
        t.tally.add(e);
        if !e.loss {
            if e.solo {
                t.solo += 1;
            } else {
                t.gang += 1;
            }
            if e.final_blow.is_some() {
                t.final_blows += 1;
            }
        }
        if e.shared() {
            t.shared += 1;
        }
        if !e.valued {
            t.unvalued += 1;
        }

        for c in &e.chars {
            let (s, flown) = per_char.entry(*c).or_insert_with(|| (CharacterStats { char_id: *c, ..Default::default() }, HashMap::new()));
            s.tally.add(e);
            if !e.loss && e.solo {
                s.solo += 1;
            }
            if e.final_blow == Some(*c) {
                s.final_blows += 1;
            }
            if e.shared() {
                s.shared += 1;
            }
            for (_, ship) in e.ships.iter().filter(|(who, _)| who == c) {
                *flown.entry(*ship).or_default() += 1;
            }
        }
        let hulls: BTreeSet<i64> = e.ships.iter().map(|(_, s)| *s).collect();
        for h in hulls {
            ships.entry((a.type_name(h), Some(h))).or_default().add(e);
        }
        let (class, group) = ship_class(m.victim.ship_type_id);
        classes.entry((class, None)).or_default().add(e);
        groups.entry((group, None)).or_default().add(e);
        systems.entry((k.system.clone(), Some(m.solar_system_id))).or_default().add(e);
        let region = k.region.clone().unwrap_or_else(|| "Unknown".into());
        regions.entry((region, k.region_id)).or_default().add(e);
        bands.entry(k.band()).or_default().add(e);

        if e.loss {
            let theirs = || m.attackers.iter().filter(|x| !x.character_id.is_some_and(|c| mine.contains(&c)));
            let cs: BTreeSet<i64> = theirs().filter_map(|x| x.corporation_id.or(x.faction_id)).collect();
            for c in cs {
                corps.entry((name(c), Some(c))).or_default().add(e);
            }
            let al: BTreeSet<i64> = theirs().filter_map(|x| x.alliance_id).collect();
            for x in al {
                alliances.entry((name(x), Some(x))).or_default().add(e);
            }
        } else {
            if let Some(c) = m.victim.corporation_id.or(m.victim.faction_id) {
                corps.entry((name(c), Some(c))).or_default().add(e);
            }
            if let Some(x) = m.victim.alliance_id {
                alliances.entry((name(x), Some(x))).or_default().add(e);
            }
        }
        out.hours[e.time.hour() as usize].add(e);
        out.weekdays[e.time.weekday().num_days_from_monday() as usize].add(e);
    }

    let mut characters: Vec<CharacterStats> = per_char
        .into_values()
        .map(|(mut s, flown)| {
            s.top_ship = flown.into_iter().max_by(|x, y| x.1.cmp(&y.1).then(y.0.cmp(&x.0))).map(|(t, _)| t);
            s
        })
        .collect();
    characters
        .sort_by(|x, y| y.tally.destroyed.total_cmp(&x.tally.destroyed).then(y.tally.kills.cmp(&x.tally.kills)).then(x.char_id.cmp(&y.char_id)));
    out.characters = characters;
    out.my_ships = grouped(ships);
    out.victim_classes = grouped(classes);
    out.victim_groups = grouped(groups);
    out.systems = grouped(systems);
    out.regions = grouped(regions);
    out.bands = Band::ALL.iter().map(|b| (*b, bands.get(b).copied().unwrap_or_default())).collect();
    out.enemy_corps = grouped(corps);
    out.enemy_alliances = grouped(alliances);

    let top = |loss: bool| {
        let mut v: Vec<&Entry> = entries.iter().filter(|e| e.loss == loss && e.value > 0.0).collect();
        v.sort_by(|x, y| y.value.total_cmp(&x.value).then(y.id.cmp(&x.id)));
        v.into_iter().take(10).map(|e| e.id).collect::<Vec<i64>>()
    };
    out.top_kills = top(false);
    out.top_losses = top(true);

    out.first = entries.iter().map(|e| e.time).min();
    if let Some(start) = since.or(out.first).filter(|s| *s <= now) {
        let size = wallet_flow::bucket_size(start, now);
        let mut index: HashMap<NaiveDate, usize> = HashMap::new();
        let mut d = wallet_flow::bucket_start(start.date_naive(), size);
        while d <= now.date_naive() {
            index.insert(d, out.buckets.len());
            out.buckets.push(Bucket { start: d, tally: Tally::default() });
            d = wallet_flow::next_bucket(d, size);
        }
        for e in entries {
            if let Some(&i) = index.get(&wallet_flow::bucket_start(e.time.date_naive(), size)) {
                out.buckets[i].tally.add(e);
            }
        }
        out.bucket_size = Some(size);
    }
    out
}

/// The user's characters whose killmails can't load for lack of the scope.
pub fn missing_scope(chars: &[Arc<crate::Character>]) -> Vec<Arc<crate::Character>> {
    chars.iter().filter(|c| c.refreshes.get("killmails").and_then(|r| r.error.as_deref()) == Some("scope")).cloned().collect()
}

/// A mail's zKillboard page.
pub fn zkillboard_url(id: i64) -> String {
    crate::features::character_extras::zkillboard_url(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    const ME: i64 = 90000001;
    const ALT: i64 = 90000002;

    fn at(d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, d, h, 0, 0).unwrap()
    }

    fn attacker(c: Option<i64>, corp: i64, ship: i64, dmg: i64, fb: bool) -> Attacker {
        Attacker { character_id: c, corporation_id: Some(corp), ship_type_id: Some(ship), damage_done: dmg, final_blow: fb, ..Default::default() }
    }

    fn kill(id: i64, t: DateTime<Utc>, victim: Option<i64>, victim_corp: i64, ship: i64, attackers: Vec<Attacker>, worth: f64) -> ArchivedKill {
        ArchivedKill {
            mail: EsiKillmail {
                killmail_id: id,
                killmail_time: t,
                solar_system_id: 30002187,
                victim: Victim { character_id: victim, corporation_id: Some(victim_corp), ship_type_id: ship, ..Default::default() },
                attackers,
                war_id: None,
            },
            hash: "h".into(),
            system: "Amarr".into(),
            security: Some(1.0),
            region_id: Some(10000043),
            region: Some("Domain".into()),
            seen_by: BTreeSet::new(),
            first_seen: t,
            value: Some(KillValue { at: t, hull: worth, destroyed: worth, dropped: 0.0, unpriced: 0 }),
        }
    }

    fn archive_of(kills: Vec<ArchivedKill>) -> Archive {
        Archive { kills: kills.into_iter().map(|k| (k.id(), k)).collect(), names: BTreeMap::from([(98000001, "Enemy Corp".to_string())]) }
    }

    #[test]
    fn values_hull_items_and_containers() {
        let json = r#"{"killmail_id":1,"killmail_time":"2026-11-01T12:00:00Z","solar_system_id":30000142,
            "victim":{"character_id":5,"ship_type_id":587,"items":[
                {"item_type_id":2873,"flag":27,"quantity_destroyed":1,"singleton":0},
                {"item_type_id":2873,"flag":28,"quantity_dropped":1,"singleton":0},
                {"item_type_id":34,"flag":5,"quantity_dropped":1000,"singleton":0},
                {"item_type_id":999,"flag":5,"quantity_destroyed":1,"singleton":2},
                {"item_type_id":3467,"flag":5,"quantity_destroyed":1,"singleton":0,"items":[
                    {"item_type_id":34,"flag":0,"quantity_destroyed":500,"singleton":0}]},
                {"item_type_id":4242,"flag":5,"quantity_destroyed":3,"singleton":0}]},
            "attackers":[{"character_id":7,"final_blow":true,"damage_done":100}]}"#;
        let m: EsiKillmail = serde_json::from_str(json).unwrap();
        let prices = HashMap::from([(587, 1_000_000.0), (2873, 50_000.0), (34, 5.0), (999, 1e9), (3467, 10_000.0)]);
        let v = value(&m, &prices, at(1, 12));
        assert_eq!(v.hull, 1_000_000.0);
        // hull + one launcher + container + 500 tritanium inside it; the BPC counts as nothing
        assert_eq!(v.destroyed, 1_000_000.0 + 50_000.0 + 10_000.0 + 2_500.0);
        assert_eq!(v.dropped, 50_000.0 + 5_000.0);
        assert_eq!(v.unpriced, 1);
        assert_eq!(v.total(), v.destroyed + v.dropped);
        assert!(m.name_ids().contains(&3467) && m.name_ids().contains(&30000142));
    }

    #[test]
    fn kills_losses_solo_and_final_blows() {
        let mine = HashSet::from([ME, ALT]);
        let solo =
            kill(1, at(1, 10), Some(5), 98000001, 587, vec![attacker(Some(ME), 1, 11987, 900, true), attacker(None, 500021, 0, 50, false)], 10e6);
        let e = entry(&solo, &mine).unwrap();
        assert!(!e.loss && e.solo && e.final_blow == Some(ME) && !e.shared());

        let loss = kill(2, at(1, 11), Some(ALT), 98000100, 24698, vec![attacker(Some(5), 98000001, 587, 100, true)], 50e6);
        let e = entry(&loss, &mine).unwrap();
        assert!(e.loss && !e.solo && e.final_blow.is_none());
        assert_eq!(e.chars, vec![ALT]);
        assert_eq!(e.ships, vec![(ALT, 24698)]);

        let none = kill(3, at(1, 12), Some(5), 98000001, 587, vec![attacker(Some(6), 1, 587, 100, true)], 1e6);
        assert!(entry(&none, &mine).is_none());
    }

    #[test]
    fn shared_kill_counts_once_in_totals_and_for_each_character() {
        let mine = HashSet::from([ME, ALT]);
        let shared = kill(
            1,
            at(2, 20),
            Some(5),
            98000001,
            587,
            vec![attacker(Some(ALT), 1, 12005, 300, false), attacker(Some(ME), 1, 11987, 700, true), attacker(Some(8), 2, 587, 10, false)],
            30e6,
        );
        let e = entry(&shared, &mine).unwrap();
        assert_eq!(e.chars, vec![ME, ALT], "by damage done");
        assert!(e.shared() && !e.solo);
        let a = archive_of(vec![shared]);
        let es = entries(&a, &mine, None);
        let an = analyze(&a, &es, &[ME, ALT], None, at(3, 0));
        assert_eq!(an.totals.tally.kills, 1);
        assert_eq!(an.totals.tally.destroyed, 30e6);
        assert_eq!(an.totals.shared, 1);
        assert_eq!(an.totals.gang, 1);
        assert_eq!(an.totals.final_blows, 1);
        for c in &an.characters {
            assert_eq!(c.tally.kills, 1);
            assert_eq!(c.tally.destroyed, 30e6);
            assert_eq!(c.shared, 1);
        }
        assert_eq!(an.characters.iter().find(|c| c.char_id == ME).unwrap().final_blows, 1);
        assert_eq!(an.characters.iter().find(|c| c.char_id == ALT).unwrap().top_ship, Some(12005));
        // both hulls get the kill
        assert_eq!(an.my_ships.len(), 2);
    }

    #[test]
    fn a_loss_to_my_own_alt_is_a_loss_only() {
        let mine = HashSet::from([ME, ALT]);
        let awox = kill(1, at(1, 1), Some(ME), 98000100, 587, vec![attacker(Some(ALT), 98000100, 587, 100, true)], 1e6);
        let e = entry(&awox, &mine).unwrap();
        assert!(e.loss);
        assert_eq!(e.chars, vec![ME]);
        let a = archive_of(vec![awox]);
        let an = analyze(&a, &entries(&a, &mine, None), &[ME, ALT], None, at(2, 0));
        assert_eq!(an.totals.tally.kills, 0);
        assert_eq!(an.totals.tally.losses, 1);
        // the alt isn't an enemy
        assert!(an.enemy_corps.is_empty());
    }

    #[test]
    fn analysis_breakdowns() {
        let mine = HashSet::from([ME]);
        let a = archive_of(vec![
            kill(1, at(2, 19), Some(5), 98000001, 587, vec![attacker(Some(ME), 1, 11987, 100, true)], 30e6),
            kill(2, at(2, 21), Some(6), 98000001, 587, vec![attacker(Some(ME), 1, 11987, 100, true), attacker(Some(9), 1, 587, 100, false)], 10e6),
            kill(
                3,
                at(3, 19),
                Some(ME),
                98000100,
                11987,
                vec![attacker(Some(5), 98000001, 587, 100, true), attacker(Some(6), 98000001, 587, 100, false)],
                60e6,
            ),
            // before the period
            kill(4, at(1, 1), Some(7), 98000001, 587, vec![attacker(Some(ME), 1, 11987, 100, true)], 5e6),
        ]);
        let since = Some(at(2, 0));
        let es = entries(&a, &mine, since);
        assert_eq!(es.iter().map(|e| e.id).collect::<Vec<_>>(), vec![3, 2, 1], "newest first, period only");
        let an = analyze(&a, &es, &[ME, ALT], since, at(4, 0));
        let t = an.totals;
        assert_eq!((t.tally.kills, t.tally.losses, t.solo, t.gang, t.final_blows), (2, 1, 1, 1, 2));
        assert_eq!(t.tally.efficiency(), Some(40e6 / 100e6));
        // the alt has a row with nothing on it
        assert_eq!(an.characters.len(), 2);
        assert_eq!(an.characters.iter().find(|c| c.char_id == ALT).unwrap().tally, Tally::default());
        // the Guardian flown: two kills and its own loss
        let g = an.my_ships.iter().find(|g| g.id == Some(11987)).unwrap();
        assert_eq!((g.tally.kills, g.tally.losses), (2, 1));
        // the enemy corporation: killed twice, on the loss once (two of its pilots, one mail)
        let enemy = an.enemy_corps.iter().find(|g| g.id == Some(98000001)).unwrap();
        assert_eq!(enemy.label, "Enemy Corp");
        assert_eq!((enemy.tally.kills, enemy.tally.losses), (2, 1));
        assert_eq!(an.hours[19].count(), 2);
        assert_eq!(an.weekdays[at(2, 0).weekday().num_days_from_monday() as usize].kills, 2);
        assert_eq!(an.bands[0], (Band::High, Tally { kills: 2, losses: 1, destroyed: 40e6, lost: 60e6 }));
        assert_eq!(an.systems[0].label, "Amarr");
        assert_eq!(an.top_kills, vec![1, 2]);
        assert_eq!(an.top_losses, vec![3]);
        assert_eq!(an.bucket_size, Some(BucketSize::Day));
        assert_eq!(an.buckets.len(), 3);
        assert_eq!(an.buckets[0].tally.kills, 2);
        assert_eq!(an.buckets[1].tally.losses, 1);
    }

    #[test]
    fn merge_keeps_mails_and_adds_who_saw_them() {
        let mut a = Archive::default();
        let k = kill(1, at(1, 1), Some(5), 98000001, 587, vec![attacker(Some(ME), 1, 587, 1, true)], 1e6);
        assert!(merge(&mut a, ME, &[1], vec![k.clone()], HashMap::from([(5, "Victim".into())])));
        assert!(!merge(&mut a, ME, &[1], vec![k.clone()], HashMap::from([(5, "Victim".into())])), "nothing new");
        let mut changed = k.clone();
        changed.value = None;
        assert!(merge(&mut a, ALT, &[1, 2], vec![changed], HashMap::new()));
        let kept = a.get(1).unwrap();
        assert!(kept.value.is_some(), "an archived mail isn't replaced");
        assert_eq!(kept.seen_by, BTreeSet::from([ME, ALT]));
        assert_eq!(a.name(5), Some("Victim"));
    }

    #[test]
    fn bands_and_slots() {
        assert_eq!(band(31000005, None, Some(-1.0)), Band::Wormhole);
        assert_eq!(band(32000001, None, None), Band::Abyssal);
        assert_eq!(band(30000001, Some(POCHVEN), Some(-1.0)), Band::Pochven);
        assert_eq!(band(1, Some(1), Some(0.5)), Band::High);
        assert_eq!(band(1, Some(1), Some(0.1)), Band::Low);
        assert_eq!(band(1, Some(1), Some(0.0)), Band::Null);
        assert_eq!(slot(27), Slot::High);
        assert_eq!(slot(19), Slot::Mid);
        assert_eq!(slot(11), Slot::Low);
        assert_eq!(slot(92), Slot::Rig);
        assert_eq!(slot(87), Slot::Drones);
        assert_eq!(slot(5), Slot::Cargo);
        assert_eq!(slot(133), Slot::Other);
    }

    #[test]
    fn ship_classes_from_static_data() {
        // Rifter: a standard frigate
        let (class, group) = ship_class(587);
        assert_eq!(group, "Frigate");
        assert_eq!(class, "Frigates");
        let (class, group) = ship_class(-1);
        assert_eq!((class.as_str(), group.as_str()), ("Other", "Other"));
    }
}
