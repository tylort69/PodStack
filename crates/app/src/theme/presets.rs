//! Built-in seed colors, one per EVE faction. Each seeds a full Material 3 scheme (dark and light).

pub struct Preset {
    pub name: &'static str,
    pub seed: &'static str,
    pub blurb: &'static str,
}

pub const FACTIONS: &[Preset] = &[
    Preset { name: "Amarr", seed: "#c9a227", blurb: "Gilded empire gold" },
    Preset { name: "Caldari", seed: "#3d7fb8", blurb: "Corporate steel blue" },
    Preset { name: "Gallente", seed: "#2f9e72", blurb: "Federation green" },
    Preset { name: "Minmatar", seed: "#b5562f", blurb: "Rust and scrap" },
    Preset { name: "Jove", seed: "#5f9ea0", blurb: "Ancient teal" },
    Preset { name: "ORE", seed: "#e0a800", blurb: "Mining-barge yellow" },
    Preset { name: "CONCORD", seed: "#1e6fd9", blurb: "Peacekeeper blue" },
    Preset { name: "EDENCOM", seed: "#2bb3e6", blurb: "Electric cyan" },
    Preset { name: "Triglavian", seed: "#b3261e", blurb: "Abyssal crimson" },
    Preset { name: "Sansha's Nation", seed: "#7cb342", blurb: "True Slave green" },
    Preset { name: "Guristas", seed: "#a8812d", blurb: "Pirate brass" },
    Preset { name: "Angel Cartel", seed: "#d84332", blurb: "Cartel red" },
    Preset { name: "Blood Raiders", seed: "#8e1b2c", blurb: "Covenant blood" },
    Preset { name: "Serpentis", seed: "#7b5ea7", blurb: "Booster violet" },
];

pub const DEFAULT_FACTION: &str = "Caldari";

pub fn find(name: &str) -> &'static Preset {
    FACTIONS.iter().find(|p| p.name.eq_ignore_ascii_case(name)).unwrap_or_else(|| find(DEFAULT_FACTION))
}
