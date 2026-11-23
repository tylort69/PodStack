//! Builds a demo data directory: six characters of one player (a main, a capital alt, a logistics alt, a hauler, a
//! mid-skilled alt and a new one) with skills, half a year of contracts, and a doctrine library with groups and SIGs.
//! For UI testing and screenshots, never for a real data directory.
//!
//!     cargo run -p podstack-core --example demo_data -- <empty data dir> [cache dir]
//!     PODSTACK_OFFLINE=1 PODSTACK_DATA_DIR=<data dir> PODSTACK_CACHE_DIR=<cache dir> cargo run
//!
//! (`PODSTACK_OFFLINE` keeps PodStack from refreshing the demo characters, whose logins aren't real.)
//!
//! Doctrine fits are looked up through ESI (as importing them in the app does), so this needs a connection.

use chrono::{DateTime, Duration, Utc};
use podstack_core::character::Character;
use podstack_core::domains::contracts::{Bid, Contract, ContractItem, ContractsData};
use podstack_core::domains::info::{Alliance, Corporation, Info};
use podstack_core::domains::skills::{Attributes, QueueEntry, Skill, SkillsData};
use podstack_core::features::contracts::{Archive, ArchivedContract, Valuation};
use podstack_core::features::doctrines::{self, Doctrine, DoctrineDoc, Group};
use podstack_core::sde::SKILLS;
use podstack_core::universe::Place;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const CHARS: [(i64, &str, Profile); 6] = [
    (90000001, "Kira Vex", Profile::Veteran),
    (90000002, "Daro Kell", Profile::Capitals),
    (90000003, "Mira Solenne", Profile::Logistics),
    (90000004, "Tovan Rusk", Profile::Hauler),
    (90000005, "Ysa Morrow", Profile::Mid),
    (90000006, "Pell Arno", Profile::New),
];
const CORP: (i64, &str, &str) = (98000100, "Vexis Logistics", "VEXL");
const ALLIANCE: (i64, &str, &str) = (99000100, "Northern Accord", "NACRD");

#[derive(Clone, Copy)]
enum Profile {
    Veteran,
    Capitals,
    Logistics,
    Hauler,
    Mid,
    New,
}

fn is_capital(name: &str) -> bool {
    ["Dreadnought", "Carrier", "Titan", "Supercarrier", "Capital", "Force Auxiliary", "Jump Freighter", "Freighter", "Lancer"]
        .iter()
        .any(|k| name.contains(k))
}

/// The level a profile has in a skill.
fn level(p: Profile, group: &str, name: &str) -> u8 {
    let cap = is_capital(name);
    match p {
        // a few titans and jump freighters still training
        Profile::Veteran if name.contains("Titan") || name.contains("Jump Freighter") || name.contains("Lancer") => 4,
        Profile::Veteran => 5,
        Profile::Capitals => match group {
            "Spaceship Command" => 5,
            "Armor" | "Engineering" | "Gunnery" | "Neural Enhancement" => 5,
            "Navigation" | "Shields" | "Targeting" | "Drones" | "Rigging" => 4,
            "Electronic Systems" | "Missiles" | "Fleet Support" => 3,
            _ => 1,
        },
        Profile::Logistics => match group {
            "Spaceship Command" if cap => if name.contains("Force Auxiliary") || name.starts_with("Capital ") { 1 } else { 0 },
            "Spaceship Command" | "Armor" | "Shields" | "Engineering" => 5,
            "Navigation" | "Electronic Systems" | "Targeting" | "Fleet Support" => 4,
            "Drones" | "Rigging" => 3,
            _ => 1,
        },
        Profile::Hauler => match group {
            "Spaceship Command" if name.contains("Freighter") || name.contains("Industrial") || name.contains("Transport") => 5,
            "Spaceship Command" if cap => 0,
            "Trade" | "Production" | "Resource Processing" | "Planet Management" | "Science" => 5,
            "Spaceship Command" | "Navigation" | "Engineering" | "Shields" => 3,
            _ => 1,
        },
        Profile::Mid => match group {
            "Spaceship Command" if cap => 0,
            "Spaceship Command" | "Gunnery" | "Missiles" | "Drones" | "Navigation" | "Shields" | "Armor" | "Engineering" => 4,
            "Targeting" | "Electronic Systems" | "Rigging" => 3,
            _ => 1,
        },
        Profile::New => match group {
            "Spaceship Command" if cap => 0,
            "Spaceship Command" | "Navigation" | "Engineering" => 3,
            "Gunnery" | "Missiles" | "Drones" | "Shields" | "Armor" => 2,
            _ => 0,
        },
    }
}

fn skills_for(p: Profile) -> SkillsData {
    let mut skills = Vec::new();
    let mut total = 0;
    for g in &SKILLS.groups {
        for id in &g.types {
            let Some(info) = SKILLS.get(*id) else { continue };
            let l = level(p, &g.name, &info.name);
            if l == 0 {
                continue;
            }
            let sp = info.sp_for_level(l);
            total += sp;
            skills.push(Skill {
                skill_id: *id,
                skillpoints_in_skill: sp,
                trained_skill_level: l,
                active_skill_level: l,
                skill_name: info.name.clone(),
                group_name: g.name.clone(),
                ..Default::default()
            });
        }
    }
    SkillsData { skills, total_sp: total, unallocated_sp: 0 }
}

fn place(id: i64, name: &str, system: i64, sys: &str, sec: f64, region: &str) -> Place {
    Place {
        id,
        name: name.into(),
        system_id: Some(system),
        system_name: Some(sys.into()),
        security: Some(sec),
        region_name: Some(region.into()),
        kind: "station".into(),
    }
}

/// A tiny deterministic random source, so the demo looks the same every time.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo).max(1) as u64) as i64
    }
    fn f(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (self.next() % 10_000) as f64 / 10_000.0 * (hi - lo)
    }
}

fn item(type_id: i64, name: &str, quantity: i64) -> ContractItem {
    ContractItem { record_id: type_id * 1000 + quantity, type_id, name: name.into(), quantity, is_included: true, ..Default::default() }
}

const FITS: &[(&str, &str, &str, &str)] = &[
    ("Init", "Armor HACs", "DPS", "[Zealot, Zealot - Mainline]\nHeat Sink II\nHeat Sink II\nMedium Armor Repairer II\n800mm Steel Plates II\nMultispectrum Energized Membrane II\nDamage Control II\n\n10MN Afterburner II\nWarp Disruptor II\nStasis Webifier II\n\nHeavy Pulse Laser II\nHeavy Pulse Laser II\nHeavy Pulse Laser II\nHeavy Pulse Laser II\nHeavy Pulse Laser II\n\nScorch M x2000"),
    ("Init", "Armor HACs", "Logistics", "[Guardian, Guardian - Mainline]\nDamage Control II\n1600mm Steel Plates II\nMultispectrum Energized Membrane II\n\n50MN Microwarpdrive II\nCap Recharger II\n\nMedium Remote Armor Repairer II\nMedium Remote Armor Repairer II\nMedium Remote Armor Repairer II\nMedium Remote Capacitor Transmitter II"),
    ("Init", "Armor HACs", "Tackle", "[Sabre, Sabre - Bubbler]\nDamage Control II\nNanofiber Internal Structure II\n\n5MN Microwarpdrive II\nWarp Disruptor II\nMedium Shield Extender II\n\nInterdiction Sphere Launcher I\n125mm Gatling AutoCannon II\n125mm Gatling AutoCannon II"),
    ("Init › Capitals", "Dread Bomb", "DPS", "[Revelation, Revelation - Bomb]\nCapital Armor Repairer II\nHeat Sink II\nHeat Sink II\nHeat Sink II\n\nSiege Module II\nCap Recharger II\n\nDual Giga Pulse Laser II\nDual Giga Pulse Laser II\nDual Giga Pulse Laser II"),
    ("Init › Capitals", "Dread Bomb", "DPS", "[Naglfar, Naglfar - Bomb]\nGyrostabilizer II\nGyrostabilizer II\nDamage Control II\n\nSiege Module II\n\nHexa 2500mm Repeating Cannon II\nHexa 2500mm Repeating Cannon II"),
    ("Init › Capitals", "Capital Logistics", "Logistics", "[Apostle, Apostle - Triage]\nDamage Control II\nCapital Armor Repairer II\n\nTriage Module II\nCap Recharger II\n\nCapital Remote Armor Repairer II\nCapital Remote Armor Repairer II\nCapital Remote Armor Repairer II"),
    ("Init › Incursions", "Shield Vanguard", "DPS", "[Vindicator, Vindicator - Vanguard]\nMagnetic Field Stabilizer II\nMagnetic Field Stabilizer II\nDamage Control II\n\nLarge Shield Extender II\nStasis Webifier II\n\nNeutron Blaster Cannon II\nNeutron Blaster Cannon II\nNeutron Blaster Cannon II"),
    ("Init › Incursions", "Shield Vanguard", "Logistics", "[Basilisk, Basilisk - Vanguard]\nPower Diagnostic System II\n\nLarge Shield Extender II\n\nMedium Remote Shield Booster II\nMedium Remote Shield Booster II\nMedium Remote Shield Booster II"),
    ("Home Defense", "Kitchen Sink", "DPS", "[Drake, Drake - Home Defense]\nBallistic Control System II\nBallistic Control System II\nDamage Control II\n\nLarge Shield Extender II\nLarge Shield Extender II\n10MN Afterburner II\n\nHeavy Missile Launcher II\nHeavy Missile Launcher II\nHeavy Missile Launcher II\nHeavy Missile Launcher II\nHeavy Missile Launcher II"),
    ("Home Defense", "Kitchen Sink", "DPS", "[Ferox, Ferox - Rails]\nMagnetic Field Stabilizer II\nMagnetic Field Stabilizer II\nDamage Control II\n\nLarge Shield Extender II\n10MN Afterburner II\n\n250mm Railgun II\n250mm Railgun II\n250mm Railgun II\n250mm Railgun II"),
    ("Home Defense", "Kitchen Sink", "Logistics", "[Scimitar, Scimitar - Home Defense]\nDamage Control II\n\nLarge Shield Extender II\n50MN Microwarpdrive II\n\nMedium Remote Shield Booster II\nMedium Remote Shield Booster II\nMedium Remote Shield Booster II"),
];

/// The id of the group at a path like "Init › Capitals", made (with any missing parents) if it isn't there.
fn ensure_path(doc: &mut DoctrineDoc, path: &str) -> String {
    let mut prefix: Vec<&str> = Vec::new();
    let mut parent: Option<String> = None;
    for name in path.split(" › ") {
        prefix.push(name);
        let key = prefix.join(" › ");
        let found = doc.all_groups().into_iter().find(|(p, _)| p.join(" › ") == key).map(|(_, g)| g.id.clone());
        let id = match found {
            Some(id) => id,
            None => {
                let g = Group { id: doctrines::new_id(), name: name.into(), ..Default::default() };
                let id = g.id.clone();
                match &parent {
                    Some(pid) => doc.group_mut(pid).unwrap().groups.push(g),
                    None => doc.groups.push(g),
                }
                id
            }
        };
        parent = Some(id);
    }
    parent.unwrap()
}

/// Doctrine ships in hangars, for the Asset Search's doctrine view: some fitted as the doctrine says, some packaged,
/// one fitted another way. Characters with nothing here (the new one) are left without assets, as if not loaded.
fn add_assets(dir: &std::path::Path, doc: &DoctrineDoc, places: &[Place]) {
    use podstack_core::domains::assets::{Asset, AssetsData};
    // (character, ship, how: 'f' fitted as the doctrine, 'p' packaged, 'o' assembled but fitted otherwise, station index)
    let owned: &[(i64, &str, char, usize)] = &[
        (90000001, "Zealot", 'f', 0),
        (90000001, "Guardian", 'p', 1),
        (90000001, "Vindicator", 'f', 0),
        (90000002, "Revelation", 'f', 1),
        (90000002, "Naglfar", 'p', 3),
        (90000003, "Guardian", 'f', 1),
        (90000003, "Basilisk", 'o', 0),
        (90000005, "Drake", 'f', 2),
        (90000005, "Ferox", 'p', 2),
    ];
    let fits = doc.all_fits();
    let mut next = 1_000_000_000_000i64;
    let mut id = || {
        next += 1;
        next
    };
    for (char_id, _, _) in CHARS.iter().filter(|(_, _, p)| !matches!(p, Profile::New)) {
        let path = dir.join("characters").join(format!("{char_id}.json"));
        let mut c: Character = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let mut data = AssetsData::default();
        for (_, ship, how, at) in owned.iter().filter(|o| o.0 == *char_id) {
            let Some((_, _, fit)) = fits.iter().find(|(_, _, f)| f.ship_name == *ship) else { continue };
            let place = &places[*at];
            data.locations.insert(place.id, place.clone());
            let hull = id();
            let asset = |item_id, type_id, name: &str, quantity, flag: &str, parent: Option<i64>, singleton| Asset {
                item_id,
                type_id,
                name: name.into(),
                quantity,
                flag: flag.into(),
                parent_id: parent,
                root_location_id: place.id,
                is_singleton: singleton,
                ..Default::default()
            };
            data.items.push(asset(hull, fit.ship_id, ship, 1, "Hangar", None, *how != 'p'));
            if *how == 'p' {
                continue;
            }
            // a ship fitted otherwise keeps only its first module
            let rest = fit.items.iter().skip(1).take(if *how == 'o' { 1 } else { usize::MAX });
            for (slot, i) in rest.enumerate() {
                let flag = match i.category.as_str() {
                    "Module" | "Subsystem" => format!("LoSlot{slot}"),
                    "Drone" => "DroneBay".into(),
                    _ => "Cargo".into(),
                };
                let stacked = !matches!(i.category.as_str(), "Module" | "Subsystem");
                for _ in 0..if stacked { 1 } else { i.quantity.max(1) } {
                    data.items.push(asset(id(), i.type_id, &i.name, if stacked { i.quantity as i64 } else { 1 }, &flag, Some(hull), !stacked));
                }
            }
        }
        c.assets = Some(data);
        std::fs::write(&path, serde_json::to_string(&c).unwrap()).unwrap();
    }
}

/// Two accounts for the first four characters; the last two stay unassigned.
fn add_accounts(dir: &std::path::Path) {
    let doc = serde_json::json!({
        "accounts": { "main": { "name": "Main account" }, "alts": { "name": "Alt account" } },
        "assignments": { "90000001": "main", "90000002": "main", "90000003": "alts", "90000004": "alts" },
    });
    std::fs::write(dir.join("accounts.json"), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let Some(dir) = args.get(1).map(PathBuf::from) else {
        eprintln!("usage: demo_data <empty data dir> [cache dir]");
        std::process::exit(2);
    };
    if dir.join("auth.json").exists() {
        eprintln!("{} already has data: use an empty directory", dir.display());
        std::process::exit(1);
    }
    std::fs::create_dir_all(dir.join("characters")).unwrap();
    // SAFETY: single-threaded at this point, before the core reads its directories
    unsafe {
        std::env::set_var("PODSTACK_DATA_DIR", &dir);
        if let Some(c) = args.get(2) {
            std::env::set_var("PODSTACK_CACHE_DIR", c);
        }
    }
    let now = Utc::now();
    let mut rng = Rng(0x5eed_1234_abcd);

    // auth: tokens that look fine but don't work; run PodStack with PODSTACK_OFFLINE=1 so it doesn't try them
    let mut auth = serde_json::Map::new();
    for (id, name, _) in CHARS {
        auth.insert(
            id.to_string(),
            serde_json::json!({
                "id": id, "name": name, "access_token": "demo", "access_token_expiry": now + Duration::days(3650),
                "refresh_token": "demo", "client_id": "demo", "scopes": podstack_core::config::scope_names(),
                "last_refresh": { "date": now, "success": true },
                "added": now - Duration::days(400) + Duration::minutes(id - 90000000),
            }),
        );
    }
    std::fs::write(dir.join("auth.json"), serde_json::to_string_pretty(&serde_json::json!({ "characters": auth })).unwrap()).unwrap();

    // places
    let jita = place(60003760, "Jita IV - Moon 4 - Caldari Navy Assembly Plant", 30000142, "Jita", 0.95, "The Forge");
    let amarr = place(60008494, "Amarr VIII (Oris) - Emperor Family Academy", 30002187, "Amarr", 1.0, "Domain");
    let dodixie = place(60011866, "Dodixie IX - Moon 20 - Federation Navy Assembly Plant", 30002659, "Dodixie", 0.87, "Sinq Laison");
    let rens = place(60004588, "Rens VI - Moon 8 - Brutor Tribe Treasury", 30002510, "Rens", 0.9, "Heimatar");
    let hubs = [jita.clone(), amarr.clone(), dodixie.clone(), rens.clone()];
    let sellers: Vec<(i64, String)> =
        ["Ava Kestrel", "Brin Solace", "Corwin Vale", "Dax Marrow", "Eline Frost", "Fen Aldric"].iter().enumerate().map(|(i, n)| (95000000 + i as i64, n.to_string())).collect();
    let ships = [(12005, "Ishtar", 280e6, 10000.0), (22456, "Sabre", 65e6, 2500.0), (24698, "Drake", 55e6, 15000.0), (29990, "Loki", 420e6, 5000.0), (11987, "Guardian", 260e6, 10000.0), (12003, "Zealot", 240e6, 10000.0)];
    let loot = [(34, "Tritanium", 4.1), (35, "Pyerite", 11.0), (36, "Mexallon", 48.0), (37, "Isogen", 95.0), (2456, "Hobgoblin II", 400e3), (2488, "Warrior II", 350e3)];

    let mut next_id = 210_000_000i64;
    let mut archive = Archive::default();
    let mut add = |c: Contract, valuation: Option<Valuation>, source: String, first_seen: DateTime<Utc>| {
        archive.contracts.insert(
            c.contract_id,
            ArchivedContract { contract: c, sources: BTreeSet::from([source]), first_seen, last_seen: now, valuation },
        );
    };
    let base = |id: &mut i64, kind: &str, t: DateTime<Utc>| {
        *id += 1;
        Contract {
            contract_id: *id,
            kind: kind.into(),
            status: "finished".into(),
            availability: "personal".into(),
            issuer_corporation_id: 98000900,
            issuer_corporation_name: "Doctrine Supply Co".into(),
            date_issued: t - Duration::hours(5),
            date_expired: t + Duration::days(14),
            date_accepted: Some(t),
            date_completed: Some(t),
            items_loaded: true,
            start_location: Some(jita.clone()),
            end_location: Some(jita.clone()),
            ..Default::default()
        }
    };
    let archive_start = now - Duration::days(175);
    let seen = |t: DateTime<Utc>| t.max(archive_start);
    let value = |sell: f64, t: DateTime<Utc>| Valuation { at: t, station: 60003760, included_sell: sell, included_buy: sell * 0.92, ..Default::default() };

    // every character buys doctrine ships on alliance contracts
    for (cid, cname, _) in CHARS.iter().take(5) {
        for _ in 0..rng.range(3, 8) {
            let t = now - Duration::days(rng.range(1, 180)) - Duration::hours(rng.range(0, 20));
            let (tid, n, p, vol) = ships[rng.range(0, ships.len() as i64) as usize];
            let s = &sellers[rng.range(0, 6) as usize];
            let mut c = base(&mut next_id, "item_exchange", t);
            c.issuer_id = s.0;
            c.issuer_name = s.1.clone();
            c.availability = "alliance".into();
            c.assignee_id = Some(ALLIANCE.0);
            c.assignee_name = Some(ALLIANCE.1.into());
            c.acceptor_id = Some(*cid);
            c.acceptor_name = Some(cname.to_string());
            c.title = format!("{n} doctrine fit");
            c.price = Some((p * rng.f(0.88, 1.05) / 1e5).round() * 1e5);
            c.volume = Some(vol);
            c.items = vec![item(tid, n, 1), item(2456, "Hobgoblin II", 5)];
            add(c, Some(value(p * 1.03, t)), format!("c:{cid}"), seen(t));
        }
    }
    // the hauler and the mid alt sell loot to an alliance buyback
    for cid in [90000004, 90000005] {
        let cname = CHARS.iter().find(|c| c.0 == cid).unwrap().1;
        for _ in 0..rng.range(8, 14) {
            let t = now - Duration::days(rng.range(1, 170));
            let mut items = Vec::new();
            let mut worth = 0.0;
            for _ in 0..3 {
                let (tid, n, pr) = loot[rng.range(0, loot.len() as i64) as usize];
                let q = if pr > 1e5 { rng.range(5, 60) } else { rng.range(20_000, 400_000) };
                worth += pr * q as f64;
                items.push(item(tid, n, q));
            }
            let mut c = base(&mut next_id, "item_exchange", t);
            c.issuer_id = cid;
            c.issuer_name = cname.into();
            c.issuer_corporation_id = CORP.0;
            c.issuer_corporation_name = CORP.1.into();
            c.assignee_id = Some(98000500);
            c.assignee_name = Some("Accord Buyback".into());
            c.acceptor_id = Some(98000500);
            c.acceptor_name = Some("Accord Buyback".into());
            c.title = "Buyback".into();
            c.price = Some((worth * 0.9 * 0.92 / 1e3).round() * 1e3);
            c.volume = Some((worth / 400.0).round());
            c.items = items;
            add(c, Some(value(worth, t)), format!("c:{cid}"), seen(t));
        }
    }
    // the hauler runs public couriers
    for _ in 0..24 {
        let t = now - Duration::days(rng.range(1, 150));
        let a = rng.range(0, 4) as usize;
        let b = (a + rng.range(1, 4) as usize) % 4;
        let vol = [12_000.0, 60_000.0, 120_000.0, 320_000.0, 800_000.0][rng.range(0, 5) as usize];
        let s = &sellers[rng.range(0, 6) as usize];
        let mut c = base(&mut next_id, "courier", t);
        c.issuer_id = s.0;
        c.issuer_name = s.1.clone();
        c.availability = "public".into();
        c.acceptor_id = Some(90000004);
        c.acceptor_name = Some("Tovan Rusk".into());
        c.date_completed = Some(t + Duration::hours(rng.range(2, 30)));
        c.days_to_complete = Some(3);
        c.reward = Some((vol * rng.f(60.0, 140.0) / 1e4).round() * 1e4 + 2e6);
        c.collateral = Some((rng.f(1e8, 3e9) / 1e6).round() * 1e6);
        c.volume = Some(vol);
        c.start_location = Some(hubs[a].clone());
        c.end_location = Some(hubs[b].clone());
        if rng.range(0, 25) == 0 {
            c.status = "failed".into();
        }
        add(c, None, "c:90000004".into(), seen(t));
    }
    // the main sold a battleship at auction
    {
        let t = now - Duration::days(20);
        let mut c = base(&mut next_id, "auction", t);
        c.issuer_id = 90000001;
        c.issuer_name = "Kira Vex".into();
        c.availability = "public".into();
        c.acceptor_id = Some(sellers[2].0);
        c.acceptor_name = Some(sellers[2].1.clone());
        c.price = Some(200e6);
        c.buyout = Some(400e6);
        c.volume = Some(50_000.0);
        c.items = vec![item(638, "Raven", 1)];
        c.bids = vec![Bid { bid_id: 1, bidder_id: sellers[2].0, amount: 265e6, date_bid: t - Duration::hours(2) }];
        add(c, Some(value(230e6, t)), "c:90000001".into(), seen(t));
    }
    // a ship moved from the main to the logistics alt
    {
        let t = now - Duration::days(12);
        let mut c = base(&mut next_id, "item_exchange", t);
        c.issuer_id = 90000001;
        c.issuer_name = "Kira Vex".into();
        c.issuer_corporation_id = CORP.0;
        c.issuer_corporation_name = CORP.1.into();
        c.acceptor_id = Some(90000003);
        c.acceptor_name = Some("Mira Solenne".into());
        c.assignee_id = Some(90000003);
        c.assignee_name = Some("Mira Solenne".into());
        c.price = Some(0.0);
        c.volume = Some(10_000.0);
        c.items = vec![item(11987, "Guardian", 1)];
        add(c, Some(value(262e6, t)), "c:90000001".into(), seen(t));
    }
    // open work: a courier being hauled, one shipped, a sale listed
    {
        let mut c = base(&mut next_id, "courier", now - Duration::days(2));
        c.status = "in_progress".into();
        c.date_completed = None;
        c.issuer_id = sellers[0].0;
        c.issuer_name = sellers[0].1.clone();
        c.availability = "public".into();
        c.acceptor_id = Some(90000004);
        c.acceptor_name = Some("Tovan Rusk".into());
        c.date_accepted = Some(now - Duration::days(2) - Duration::hours(14));
        c.days_to_complete = Some(3);
        c.reward = Some(38e6);
        c.collateral = Some(1.8e9);
        c.volume = Some(320_000.0);
        c.end_location = Some(amarr.clone());
        add(c, None, "c:90000004".into(), now - Duration::hours(2));

        let mut c = base(&mut next_id, "item_exchange", now - Duration::days(2));
        c.status = "outstanding".into();
        c.date_accepted = None;
        c.date_completed = None;
        c.issuer_id = 90000001;
        c.issuer_name = "Kira Vex".into();
        c.issuer_corporation_id = CORP.0;
        c.issuer_corporation_name = CORP.1.into();
        c.availability = "public".into();
        c.price = Some(310e6);
        c.volume = Some(10_000.0);
        c.items = vec![item(12005, "Ishtar", 1)];
        add(c, None, "c:90000001".into(), now - Duration::hours(2));
    }
    std::fs::write(dir.join("contracts_archive.json"), serde_json::to_string(&archive).unwrap()).unwrap();

    // characters
    for (id, name, profile) in CHARS {
        let mut c = Character::new(id);
        c.info = Some(Info {
            name: name.into(),
            corporation_id: CORP.0,
            alliance_id: Some(ALLIANCE.0),
            corporation: Some(Corporation { name: CORP.1.into(), ticker: CORP.2.into(), member_count: 42 }),
            alliance: Some(Alliance { name: ALLIANCE.1.into(), ticker: ALLIANCE.2.into() }),
            security_status: Some(2.1),
            birthday: Some(now - Duration::days(400 * (7 - (id - 90000000)))),
            ..Default::default()
        });
        let skills = skills_for(profile);
        // a few levels in training (the new character is idle, for the queue health views)
        if !matches!(profile, Profile::New) {
            let mut at = now - Duration::hours(3);
            let mut queue = Vec::new();
            for (pos, s) in skills.skills.iter().filter(|s| s.trained_skill_level < 5).take(4).enumerate() {
                let info = SKILLS.get(s.skill_id).unwrap();
                let level = s.trained_skill_level + 1;
                let hours = 6 + 10 * (pos as i64 + 1) * info.training_time_multiplier as i64;
                queue.push(QueueEntry {
                    skill_id: s.skill_id,
                    finished_level: level,
                    queue_position: pos as i32,
                    start_date: Some(at),
                    finish_date: Some(at + Duration::hours(hours)),
                    training_start_sp: Some(s.skillpoints_in_skill),
                    level_start_sp: Some(info.sp_for_level(level - 1)),
                    level_end_sp: Some(info.sp_for_level(level)),
                    skill_name: info.name.clone(),
                });
                at += Duration::hours(hours);
            }
            c.skill_queue = Some(queue);
        }
        c.skills = Some(skills);
        c.attributes = Some(Attributes { charisma: 19, intelligence: 24, memory: 24, perception: 27, willpower: 21, ..Default::default() });
        c.wallet = Some([4.2e9, 1.1e9, 640e6, 2.8e9, 310e6, 12e6][(id - 90000001) as usize]);
        let listed: Vec<Contract> = archive
            .contracts
            .values()
            .filter(|e| e.sources.contains(&format!("c:{id}")) && (!e.contract.is_completed() || e.contract.date_issued > now - Duration::days(30)))
            .map(|e| e.contract.clone())
            .collect();
        let slots = listed.iter().filter(|k| k.issuer_id == id && !k.is_completed()).count() as u32;
        c.contracts = Some(ContractsData { contracts: listed, slots_used: slots });
        c.corp_contracts = Some(ContractsData::default());
        std::fs::write(dir.join("characters").join(format!("{id}.json")), serde_json::to_string(&c).unwrap()).unwrap();
    }

    // doctrines: resolved through ESI like an import in the app
    let core = podstack_core::Core::load();
    let mut doc = DoctrineDoc::default();
    let mut by_path: BTreeMap<(String, String), Vec<doctrines::DoctrineFit>> = BTreeMap::new();
    for (path, doctrine, role, eft) in FITS {
        let parsed = podstack_core::features::fitting::parse_eft(eft).unwrap();
        match doctrines::resolve(&core, &parsed, eft).await {
            Ok(mut f) => {
                f.role = role.to_string();
                if !f.unknown.is_empty() {
                    eprintln!("{}: not found: {:?}", f.name, f.unknown);
                }
                by_path.entry((path.to_string(), doctrine.to_string())).or_default().push(f);
            }
            Err(e) => eprintln!("{eft:.30}: {e}"),
        }
    }
    for ((path, doctrine), fits) in by_path {
        let id = ensure_path(&mut doc, &path);
        doc.group_mut(&id).unwrap().doctrines.push(Doctrine { id: doctrines::new_id(), name: doctrine, fits, ..Default::default() });
    }
    std::fs::write(dir.join("doctrines.json"), serde_json::to_string_pretty(&doc).unwrap()).unwrap();
    add_assets(&dir, &doc, &[jita, amarr, dodixie, rens]);
    add_accounts(&dir);
    core.universe.flush();
    println!("demo data written to {} ({} contracts, {} doctrine fits)", dir.display(), archive.contracts.len(), doc.fit_count());
}
