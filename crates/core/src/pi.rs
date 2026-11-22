//! Planetary industry static data: schematics (what each factory makes from what, and how fast), commodities by
//! tier with their volume and customs tax base, the structures colonies are built from, planet types, and every
//! planet in known space. From CCP's static data (see `static_data`), except the few facts the export doesn't
//! carry, which live here as constants: which raw resources each planet type has, and command center capacity per
//! upgrade level. Those haven't changed since planetary interaction launched.

use crate::static_data::{self, Live, PiFile, PiPinFile};
use std::collections::HashMap;

/// P0 (raw) to P4.
pub type Tier = u8;

#[derive(Clone, Debug)]
pub struct Commodity {
    pub id: i64,
    pub name: String,
    pub tier: Tier,
    /// m³ per unit.
    pub volume: f64,
    /// Customs offices charge their tax rate on this value per unit (not on the market price).
    pub tax_base: f64,
}

#[derive(Clone, Debug)]
pub struct Schematic {
    pub id: i64,
    pub name: String,
    /// Seconds per cycle.
    pub cycle: i64,
    /// (type, units per cycle)
    pub inputs: Vec<(i64, i64)>,
    /// (type, units per cycle)
    pub output: (i64, i64),
    /// Factory structures that can run it.
    pub pins: Vec<i64>,
}

impl Schematic {
    pub fn per_hour(&self) -> f64 {
        self.output.1 as f64 * 3600.0 / self.cycle.max(1) as f64
    }
    /// Units of `input` used per hour by one factory.
    pub fn input_per_hour(&self, input: i64) -> f64 {
        self.inputs.iter().find(|i| i.0 == input).map(|i| i.1 as f64 * 3600.0 / self.cycle.max(1) as f64).unwrap_or(0.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PinKind {
    Extractor,
    CommandCenter,
    Factory,
    Storage,
    Launchpad,
    ExtractorControlUnit,
}

impl PinKind {
    fn of_group(g: i64) -> Option<PinKind> {
        match g {
            1026 => Some(PinKind::Extractor),
            1027 => Some(PinKind::CommandCenter),
            1028 => Some(PinKind::Factory),
            1029 => Some(PinKind::Storage),
            1030 => Some(PinKind::Launchpad),
            1063 => Some(PinKind::ExtractorControlUnit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Pin {
    pub id: i64,
    pub name: String,
    pub kind: PinKind,
    pub planet_type: Option<i64>,
    pub capacity: f64,
    pub cpu: f64,
    pub power: f64,
    /// Percent of the tax base charged on exports and imports.
    pub export_tax: Option<f64>,
    pub import_tax: Option<f64>,
}

/// One of the eight colonizable planet kinds.
#[derive(Clone, Debug)]
pub struct PlanetKind {
    /// "temperate", as ESI names it.
    pub key: &'static str,
    pub label: &'static str,
    /// Planet type ids of this kind.
    pub types: Vec<i64>,
    /// Raw resources (P0) found on it.
    pub resources: Vec<i64>,
}

/// Raw resources per planet kind. Not in CCP's static data (the client knows it); unchanged since 2010.
const RESOURCES: [(&str, &str, [&str; 5]); 8] = [
    ("barren", "Barren", ["Aqueous Liquids", "Base Metals", "Carbon Compounds", "Microorganisms", "Noble Metals"]),
    ("gas", "Gas", ["Aqueous Liquids", "Base Metals", "Ionic Solutions", "Noble Gas", "Reactive Gas"]),
    ("ice", "Ice", ["Aqueous Liquids", "Heavy Metals", "Microorganisms", "Noble Gas", "Planktic Colonies"]),
    ("lava", "Lava", ["Base Metals", "Felsic Magma", "Heavy Metals", "Non-CS Crystals", "Suspended Plasma"]),
    ("oceanic", "Oceanic", ["Aqueous Liquids", "Carbon Compounds", "Complex Organisms", "Microorganisms", "Planktic Colonies"]),
    ("plasma", "Plasma", ["Base Metals", "Heavy Metals", "Noble Metals", "Non-CS Crystals", "Suspended Plasma"]),
    ("storm", "Storm", ["Aqueous Liquids", "Base Metals", "Ionic Solutions", "Noble Gas", "Suspended Plasma"]),
    ("temperate", "Temperate", ["Aqueous Liquids", "Autotrophs", "Carbon Compounds", "Complex Organisms", "Microorganisms"]),
];

/// Command center (power, CPU) per upgrade level 0-5. Level 0 matches the static data; the upgrades are client-side.
pub const COMMAND_CENTER: [(f64, f64); 6] =
    [(6000.0, 1675.0), (9000.0, 7057.0), (12000.0, 12136.0), (15000.0, 17215.0), (17000.0, 21315.0), (19000.0, 25415.0)];

/// Skills: Interplanetary Consolidation (+1 planet per level) and Command Center Upgrades (highest upgrade level).
pub const INTERPLANETARY_CONSOLIDATION: i64 = 2495;
pub const COMMAND_CENTER_UPGRADES: i64 = 2505;
pub const PLANETOLOGY: i64 = 2406;
pub const ADVANCED_PLANETOLOGY: i64 = 2403;
pub const REMOTE_SENSING: i64 = 13279;
/// The PI skills, in the order players think of them.
pub const SKILLS: [i64; 5] = [COMMAND_CENTER_UPGRADES, INTERPLANETARY_CONSOLIDATION, PLANETOLOGY, ADVANCED_PLANETOLOGY, REMOTE_SENSING];

pub struct PiData {
    pub commodities: HashMap<i64, Commodity>,
    pub schematics: HashMap<i64, Schematic>,
    /// Output type → schematic.
    made_by: HashMap<i64, i64>,
    /// Input type → schematics using it.
    used_by: HashMap<i64, Vec<i64>>,
    pub pins: HashMap<i64, Pin>,
    pub kinds: Vec<PlanetKind>,
    /// Planet type → kind index.
    kind_of_type: HashMap<i64, usize>,
    /// System → [(planet type, celestial index)].
    planets: HashMap<i64, Vec<(i64, i64)>>,
}

impl PiData {
    pub fn commodity(&self, id: i64) -> Option<&Commodity> {
        self.commodities.get(&id)
    }

    pub fn name(&self, id: i64) -> String {
        self.commodities.get(&id).map(|c| c.name.clone()).unwrap_or_else(|| crate::blueprints::INDUSTRY.type_name(id))
    }

    pub fn tier(&self, id: i64) -> Option<Tier> {
        self.commodities.get(&id).map(|c| c.tier)
    }

    /// Commodities of a tier, by name.
    pub fn of_tier(&self, tier: Tier) -> Vec<&Commodity> {
        let mut v: Vec<&Commodity> = self.commodities.values().filter(|c| c.tier == tier).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// The schematic that makes `output` (P1-P4).
    pub fn schematic_for(&self, output: i64) -> Option<&Schematic> {
        self.made_by.get(&output).and_then(|s| self.schematics.get(s))
    }

    /// Schematics that use `input`.
    pub fn used_by(&self, input: i64) -> Vec<&Schematic> {
        self.used_by.get(&input).map(|v| v.iter().filter_map(|s| self.schematics.get(s)).collect()).unwrap_or_default()
    }

    pub fn kind(&self, key: &str) -> Option<&PlanetKind> {
        self.kinds.iter().find(|k| k.key.eq_ignore_ascii_case(key))
    }

    pub fn kind_of_type(&self, planet_type: i64) -> Option<&PlanetKind> {
        self.kind_of_type.get(&planet_type).map(|i| &self.kinds[*i])
    }

    /// Planet kinds with `resource` (P0).
    pub fn kinds_with(&self, resource: i64) -> Vec<&PlanetKind> {
        self.kinds.iter().filter(|k| k.resources.contains(&resource)).collect()
    }

    /// The colonizable planets of a system: (planet type, celestial index).
    pub fn planets_in(&self, system: i64) -> &[(i64, i64)] {
        self.planets.get(&system).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn systems_with_planets(&self) -> impl Iterator<Item = (&i64, &Vec<(i64, i64)>)> {
        self.planets.iter()
    }

    /// The P0 resources `product` is ultimately made of.
    pub fn raw_inputs(&self, product: i64) -> Vec<i64> {
        let mut out = Vec::new();
        let mut stack = vec![product];
        let mut seen = 0;
        while let Some(t) = stack.pop() {
            seen += 1;
            if seen > 200 {
                break;
            }
            match self.schematic_for(t) {
                Some(s) => stack.extend(s.inputs.iter().map(|i| i.0)),
                None => {
                    if !out.contains(&t) {
                        out.push(t);
                    }
                }
            }
        }
        out.sort_by_key(|t| self.name(*t));
        out
    }
}

/// "Jita IV": a planet's name from its system and celestial index.
pub fn planet_name(system_name: &str, index: i64) -> String {
    format!("{system_name} {}", roman(index))
}

pub fn roman(mut n: i64) -> String {
    const R: [(i64, &str); 13] =
        [(1000, "M"), (900, "CM"), (500, "D"), (400, "CD"), (100, "C"), (90, "XC"), (50, "L"), (40, "XL"), (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I")];
    let mut s = String::new();
    for (v, r) in R {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

pub static PI: Live<PiData> = Live::new(load);

fn load() -> PiData {
    let text = static_data::read("pi.json", include_str!("../../../data/pi.json"));
    let f: PiFile = serde_json::from_str(&text).expect("pi.json");
    from_file(f)
}

pub fn from_file(f: PiFile) -> PiData {
    let tier_of = |group: i64| -> Tier {
        match group {
            1042 => 1,
            1034 => 2,
            1040 => 3,
            1041 => 4,
            _ => 0,
        }
    };
    let commodities: HashMap<i64, Commodity> = f
        .commodities
        .into_iter()
        .map(|(id, name, group, volume, tax_base)| (id, Commodity { id, name, tier: tier_of(group), volume, tax_base }))
        .collect();
    let mut made_by = HashMap::new();
    let mut used_by: HashMap<i64, Vec<i64>> = HashMap::new();
    let schematics: HashMap<i64, Schematic> = f
        .schematics
        .into_iter()
        .map(|(id, name, cycle, inputs, output, pins)| {
            made_by.insert(output.0, id);
            for i in &inputs {
                used_by.entry(i.0).or_default().push(id);
            }
            (id, Schematic { id, name, cycle, inputs, output, pins })
        })
        .collect();
    let pins = f
        .pins
        .into_iter()
        .filter_map(|p: PiPinFile| {
            Some((
                p.id,
                Pin {
                    id: p.id,
                    name: p.name,
                    kind: PinKind::of_group(p.group)?,
                    planet_type: p.planet_type,
                    capacity: p.capacity,
                    cpu: p.cpu,
                    power: p.power,
                    export_tax: p.export_tax,
                    import_tax: p.import_tax,
                },
            ))
        })
        .collect();
    let by_name: HashMap<String, i64> = commodities.values().map(|c| (c.name.to_lowercase(), c.id)).collect();
    let kinds: Vec<PlanetKind> = RESOURCES
        .iter()
        .map(|(key, label, res)| PlanetKind {
            key,
            label,
            types: f.planet_types.iter().filter(|(_, n)| n.eq_ignore_ascii_case(&format!("Planet ({label})"))).map(|(id, _)| *id).collect(),
            resources: res.iter().filter_map(|r| by_name.get(&r.to_lowercase()).copied()).collect(),
        })
        .collect();
    let mut kind_of_type = HashMap::new();
    for (i, k) in kinds.iter().enumerate() {
        for t in &k.types {
            kind_of_type.insert(*t, i);
        }
    }
    let planets = f
        .planets
        .into_iter()
        .map(|(s, list)| (s, list.into_iter().filter(|(t, _)| kind_of_type.contains_key(t)).collect::<Vec<_>>()))
        .collect();
    for v in used_by.values_mut() {
        v.sort();
    }
    PiData { commodities, schematics, made_by, used_by, pins, kinds, kind_of_type, planets }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_data() {
        let d = &*PI;
        assert_eq!(d.of_tier(0).len(), 15);
        assert_eq!(d.of_tier(1).len(), 15);
        assert_eq!(d.of_tier(2).len(), 24);
        assert_eq!(d.of_tier(3).len(), 21);
        assert_eq!(d.of_tier(4).len(), 8);
        for k in &d.kinds {
            assert!(!k.types.is_empty(), "{}", k.key);
            assert_eq!(k.resources.len(), 5, "{}", k.key);
        }
        // Coolant (P2) from Electrolytes and Water, both P1
        let coolant = d.of_tier(2).into_iter().find(|c| c.name == "Coolant").unwrap();
        let s = d.schematic_for(coolant.id).unwrap();
        assert_eq!(s.cycle, 3600);
        assert_eq!(s.inputs.len(), 2);
        assert!(s.inputs.iter().all(|i| d.tier(i.0) == Some(1)));
        // Water comes from Aqueous Liquids, found on 6 planet kinds
        let water = d.of_tier(1).into_iter().find(|c| c.name == "Water").unwrap();
        let p0 = d.schematic_for(water.id).unwrap().inputs[0].0;
        assert_eq!(d.kinds_with(p0).len(), 6);
        assert!(d.raw_inputs(coolant.id).len() == 2);
        // Jita has planets; launchpads hold 10,000 m³ and charge export tax
        assert!(!d.planets_in(30000142).is_empty());
        let pad = d.pins.values().find(|p| p.kind == PinKind::Launchpad).unwrap();
        assert_eq!(pad.capacity, 10000.0);
        assert!(pad.export_tax.is_some());
        assert!(d.commodity(p0).unwrap().tax_base > 0.0);
        assert_eq!(planet_name("Jita", 4), "Jita IV");
    }
}
