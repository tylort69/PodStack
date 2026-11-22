//! A Material 3 color scheme (one mode): every color role as "#rrggbb". Built from a seed color with
//! material-color-utilities, or read from matugen's JSON output.

use material_colors::color::Rgb;
use material_colors::dynamic_color::variant::Variant;
use material_colors::theme::ThemeBuilder;
use std::collections::BTreeMap;
use std::str::FromStr;

pub const ROLES: &[&str] = &[
    "primary", "on_primary", "primary_container", "on_primary_container", "inverse_primary",
    "primary_fixed", "primary_fixed_dim", "on_primary_fixed", "on_primary_fixed_variant",
    "secondary", "on_secondary", "secondary_container", "on_secondary_container",
    "secondary_fixed", "secondary_fixed_dim", "on_secondary_fixed", "on_secondary_fixed_variant",
    "tertiary", "on_tertiary", "tertiary_container", "on_tertiary_container",
    "tertiary_fixed", "tertiary_fixed_dim", "on_tertiary_fixed", "on_tertiary_fixed_variant",
    "error", "on_error", "error_container", "on_error_container",
    "surface_dim", "surface", "surface_tint", "surface_bright",
    "surface_container_lowest", "surface_container_low", "surface_container", "surface_container_high",
    "surface_container_highest", "on_surface", "on_surface_variant", "outline", "outline_variant",
    "inverse_surface", "inverse_on_surface", "surface_variant", "background", "on_background", "shadow", "scrim",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Scheme {
    pub dark: bool,
    pub colors: BTreeMap<String, String>,
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue)
}

pub fn variant_from_name(name: &str) -> Variant {
    match name.trim_start_matches("scheme-") {
        "vibrant" => Variant::Vibrant,
        "expressive" => Variant::Expressive,
        "content" => Variant::Content,
        "fidelity" => Variant::Fidelity,
        "neutral" => Variant::Neutral,
        "monochrome" => Variant::Monochrome,
        "rainbow" => Variant::Rainbow,
        "fruit-salad" => Variant::FruitSalad,
        _ => Variant::TonalSpot,
    }
}

pub const VARIANTS: &[(&str, &str)] = &[
    ("tonal-spot", "Tonal spot"),
    ("vibrant", "Vibrant"),
    ("expressive", "Expressive"),
    ("content", "Content"),
    ("fidelity", "Fidelity"),
    ("neutral", "Neutral"),
    ("monochrome", "Monochrome"),
    ("rainbow", "Rainbow"),
    ("fruit-salad", "Fruit salad"),
];

pub fn parse_hex(s: &str) -> Option<Rgb> {
    Rgb::from_str(s.trim()).ok()
}

impl Scheme {
    /// A scheme generated from a seed color.
    pub fn from_seed(seed: &str, dark: bool, variant: &str, contrast: f64) -> Scheme {
        let rgb = parse_hex(seed).unwrap_or(Rgb::new(0x3d, 0x7f, 0xb8));
        let theme = ThemeBuilder::with_source(rgb).variant(variant_from_name(variant)).contrast_level(contrast).build();
        let s = if dark { theme.schemes.dark } else { theme.schemes.light };
        let pairs: [(&str, Rgb); 49] = [
            ("primary", s.primary), ("on_primary", s.on_primary), ("primary_container", s.primary_container),
            ("on_primary_container", s.on_primary_container), ("inverse_primary", s.inverse_primary),
            ("primary_fixed", s.primary_fixed), ("primary_fixed_dim", s.primary_fixed_dim),
            ("on_primary_fixed", s.on_primary_fixed), ("on_primary_fixed_variant", s.on_primary_fixed_variant),
            ("secondary", s.secondary), ("on_secondary", s.on_secondary), ("secondary_container", s.secondary_container),
            ("on_secondary_container", s.on_secondary_container), ("secondary_fixed", s.secondary_fixed),
            ("secondary_fixed_dim", s.secondary_fixed_dim), ("on_secondary_fixed", s.on_secondary_fixed),
            ("on_secondary_fixed_variant", s.on_secondary_fixed_variant), ("tertiary", s.tertiary),
            ("on_tertiary", s.on_tertiary), ("tertiary_container", s.tertiary_container),
            ("on_tertiary_container", s.on_tertiary_container), ("tertiary_fixed", s.tertiary_fixed),
            ("tertiary_fixed_dim", s.tertiary_fixed_dim), ("on_tertiary_fixed", s.on_tertiary_fixed),
            ("on_tertiary_fixed_variant", s.on_tertiary_fixed_variant), ("error", s.error), ("on_error", s.on_error),
            ("error_container", s.error_container), ("on_error_container", s.on_error_container),
            ("surface_dim", s.surface_dim), ("surface", s.surface), ("surface_tint", s.surface_tint),
            ("surface_bright", s.surface_bright), ("surface_container_lowest", s.surface_container_lowest),
            ("surface_container_low", s.surface_container_low), ("surface_container", s.surface_container),
            ("surface_container_high", s.surface_container_high), ("surface_container_highest", s.surface_container_highest),
            ("on_surface", s.on_surface), ("on_surface_variant", s.on_surface_variant), ("outline", s.outline),
            ("outline_variant", s.outline_variant), ("inverse_surface", s.inverse_surface),
            ("inverse_on_surface", s.inverse_on_surface), ("surface_variant", s.surface_variant),
            ("background", s.background), ("on_background", s.on_background), ("shadow", s.shadow), ("scrim", s.scrim),
        ];
        Scheme { dark, colors: pairs.iter().map(|(k, v)| (k.to_string(), hex(*v))).collect() }
    }

    /// A scheme from matugen's `--json hex` output (`colors.<role>.<dark|light>.color`).
    pub fn from_matugen(json: &serde_json::Value, dark: bool) -> Option<Scheme> {
        let colors = json.get("colors")?.as_object()?;
        let mode = if dark { "dark" } else { "light" };
        let mut out = BTreeMap::new();
        for role in ROLES {
            let c = colors.get(*role).and_then(|r| r.get(mode)).and_then(|m| m.get("color").or(Some(m))).and_then(|c| c.as_str());
            if let Some(c) = c {
                out.insert(role.to_string(), c.to_string());
            }
        }
        // matugen versions differ in which roles they print; fill gaps from its source color
        if out.len() < ROLES.len() {
            let seed = colors
                .get("source_color")
                .and_then(|r| r.get(mode))
                .and_then(|m| m.get("color"))
                .and_then(|c| c.as_str())
                .or_else(|| out.get("primary").map(|s| s.as_str()))?
                .to_string();
            let fallback = Scheme::from_seed(&seed, dark, "tonal-spot", 0.0);
            for (k, v) in fallback.colors {
                out.entry(k).or_insert(v);
            }
        }
        Some(Scheme { dark, colors: out })
    }

    pub fn get(&self, role: &str) -> &str {
        self.colors.get(role).map(|s| s.as_str()).unwrap_or("#ff00ff")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_scheme_has_every_role() {
        let s = Scheme::from_seed("#c9a227", true, "tonal-spot", 0.0);
        assert_eq!(s.colors.len(), ROLES.len());
        assert!(s.get("surface").starts_with('#'));
    }

    #[test]
    fn reads_matugen_json() {
        let json = serde_json::json!({"colors": {"primary": {"dark": {"color": "#ffb2b7"}, "light": {"color": "#8f4a50"}}, "source_color": {"dark": {"color": "#8f4a50"}}}});
        let s = Scheme::from_matugen(&json, true).unwrap();
        assert_eq!(s.get("primary"), "#ffb2b7");
        assert_eq!(s.colors.len(), ROLES.len());
    }
}
