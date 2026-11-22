//! Bundled fonts (TrueType: HarfBuzz can't shape WOFF2): Inter (UI text, SIL OFL) and Material Icons (icon ligatures, Apache 2.0). Registered with Pango at
//! startup so they work without being installed system-wide.

use gtk::prelude::*;
use pango::prelude::FontMapExt;

const FONTS: &[(&str, &[u8])] = &[
    ("Inter.ttf", include_bytes!("../../../assets/fonts/Inter.ttf")),
    ("MaterialIcons-400.ttf", include_bytes!("../../../assets/fonts/MaterialIcons-400.ttf")),
];

pub fn register() {
    let dir = podstack_core::store::cache_dir().join("fonts");
    let _ = std::fs::create_dir_all(&dir);
    let map = pangocairo_fontmap();
    for (name, bytes) in FONTS {
        let path = dir.join(name);
        let stale = std::fs::metadata(&path).map(|m| m.len() != bytes.len() as u64).unwrap_or(true);
        if stale {
            if let Err(e) = std::fs::write(&path, bytes) {
                tracing::warn!("writing font {name}: {e}");
                continue;
            }
        }
        if let Err(e) = map.add_font_file(&path) {
            tracing::warn!("loading font {name}: {e}");
        }
    }
}

fn pangocairo_fontmap() -> pango::FontMap {
    // GTK renders with the default Pango font map, which every widget's context shares
    gtk::Label::new(None).pango_context().font_map().expect("font map")
}
