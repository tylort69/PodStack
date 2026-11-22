//! Material You theming. The color scheme comes from (Settings → Appearance):
//! - **matugen** (when installed): colors extracted from the current Hyprland wallpaper (hyprpaper or swww) or a
//!   chosen image — the default when matugen is available;
//! - a built-in **EVE faction** theme;
//! - a **custom** seed color.
//!
//! A scheme becomes a GTK stylesheet ([`css::stylesheet`]) applied app-wide. The last matugen result is cached so
//! startup doesn't wait for it.

pub mod css;
pub mod presets;
pub mod scheme;

use gtk::gdk;
use podstack_core::{store, ThemeMode, ThemeSettings, ThemeSource};
use scheme::Scheme;
use std::cell::RefCell;
use std::process::Command;

thread_local! {
    static EXTRA_CSS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
    static CURRENT: RefCell<Option<Scheme>> = const { RefCell::new(None) };
}

pub fn matugen_available() -> bool {
    which("matugen")
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
}

/// The wallpaper on the first monitor, from hyprpaper or swww.
pub fn current_wallpaper() -> Option<String> {
    if let Ok(out) = Command::new("hyprctl").args(["hyprpaper", "listactive"]).output() {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            // "DP-2: /path/to/image.png" (older hyprpaper: "DP-2 = /path")
            if let Some((_, path)) = line.split_once(": ").or_else(|| line.split_once(" = ")) {
                let path = path.trim();
                if std::path::Path::new(path).is_file() {
                    return Some(path.to_string());
                }
            }
        }
    }
    for bin in ["swww", "awww"] {
        if let Ok(out) = Command::new(bin).arg("query").output() {
            let text = String::from_utf8_lossy(&out.stdout);
            for line in text.lines() {
                if let Some(idx) = line.find("image: ") {
                    let path = line[idx + 7..].trim();
                    if std::path::Path::new(path).is_file() {
                        return Some(path.to_string());
                    }
                }
            }
        }
    }
    None
}

fn matugen_cache_path() -> std::path::PathBuf {
    store::cache_dir().join("matugen.json")
}

/// Runs matugen on an image; returns its JSON (also cached for the next start).
pub fn run_matugen(image: &str, variant: &str, contrast: f64) -> Result<serde_json::Value, String> {
    let out = Command::new("matugen")
        .args(["image", image, "--json", "hex", "--dry-run", "-q", "--prefer", "saturation"])
        .args(["-t", &format!("scheme-{variant}")])
        .args(["--contrast", &format!("{contrast:.2}")])
        .output()
        .map_err(|e| format!("couldn't run matugen: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!("matugen failed: {}", err.lines().find(|l| !l.trim().is_empty()).unwrap_or("unknown error")));
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("matugen output: {e}"))?;
    let _ = store::write_json(&matugen_cache_path(), &json, false);
    Ok(json)
}

pub fn cached_matugen() -> Option<serde_json::Value> {
    store::read_json(&matugen_cache_path())
}

/// Whether the desktop prefers dark (for [`ThemeMode::System`]).
pub fn system_prefers_dark() -> bool {
    if let Ok(out) = Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "color-scheme"]).output() {
        let s = String::from_utf8_lossy(&out.stdout);
        if s.contains("prefer-light") || s.contains("default") {
            return false;
        }
        if s.contains("prefer-dark") {
            return true;
        }
    }
    true
}

pub fn is_dark(mode: ThemeMode) -> bool {
    match mode {
        ThemeMode::Dark => true,
        ThemeMode::Light => false,
        ThemeMode::System => system_prefers_dark(),
    }
}

/// The scheme for settings, without running matugen (uses its cached output; falls back to the default faction).
pub fn resolve_fast(t: &ThemeSettings) -> Scheme {
    let dark = is_dark(t.mode);
    let faction = |name: &str| Scheme::from_seed(presets::find(name).seed, dark, &t.variant, t.contrast);
    match &t.source {
        ThemeSource::Auto | ThemeSource::Matugen { .. } if matugen_available() => cached_matugen()
            .and_then(|j| Scheme::from_matugen(&j, dark))
            .unwrap_or_else(|| faction(presets::DEFAULT_FACTION)),
        ThemeSource::Auto | ThemeSource::Matugen { .. } => faction(presets::DEFAULT_FACTION),
        ThemeSource::Faction { name } => faction(name),
        ThemeSource::Custom { seed } => Scheme::from_seed(seed, dark, &t.variant, t.contrast),
    }
}

/// The scheme for settings, running matugen when the source uses it (blocking: call off the UI thread).
pub fn resolve_full(t: &ThemeSettings) -> Result<Scheme, String> {
    let dark = is_dark(t.mode);
    match &t.source {
        ThemeSource::Auto | ThemeSource::Matugen { image: None } if matugen_available() => {
            let wp = current_wallpaper().ok_or("No wallpaper found (hyprpaper or swww).")?;
            let json = run_matugen(&wp, &t.variant, t.contrast)?;
            Scheme::from_matugen(&json, dark).ok_or_else(|| "matugen printed no colors".into())
        }
        ThemeSource::Matugen { image: Some(img) } if matugen_available() => {
            let json = run_matugen(img, &t.variant, t.contrast)?;
            Scheme::from_matugen(&json, dark).ok_or_else(|| "matugen printed no colors".into())
        }
        ThemeSource::Matugen { .. } => Err("matugen isn't installed (pacman -S matugen).".into()),
        _ => Ok(resolve_fast(t)),
    }
}

/// Applies a scheme to the whole app.
pub fn apply(scheme: &Scheme) {
    let Some(display) = gdk::Display::default() else { return };
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_interface_color_scheme(if scheme.dark { gtk::InterfaceColorScheme::Dark } else { gtk::InterfaceColorScheme::Light });
    }
    let mut css = css::stylesheet(scheme);
    EXTRA_CSS.with(|e| {
        for extra in e.borrow().iter() {
            css.push_str(extra);
        }
    });
    PROVIDER.with(|p| {
        let mut p = p.borrow_mut();
        let provider = p.get_or_insert_with(|| {
            let provider = gtk::CssProvider::new();
            gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_USER);
            provider
        });
        provider.load_from_string(&css);
    });
    CURRENT.with(|c| *c.borrow_mut() = Some(scheme.clone()));
}

/// Adds a page's own styles to the app stylesheet (once; call it from the page's `build`). Use only the scheme's
/// color roles (`@primary`, `@surface_container`, …) so it follows the theme.
pub fn add_css(css: &'static str) {
    let added = EXTRA_CSS.with(|e| {
        let mut e = e.borrow_mut();
        if e.iter().any(|x| std::ptr::eq(*x, css)) {
            false
        } else {
            e.push(css);
            true
        }
    });
    if added {
        if let Some(s) = CURRENT.with(|c| c.borrow().clone()) {
            apply(&s);
        }
    }
}

/// The scheme in use (for widgets that draw with cairo, e.g. charts).
pub fn current() -> Scheme {
    CURRENT.with(|c| c.borrow().clone()).unwrap_or_else(|| resolve_fast(&ThemeSettings::default()))
}

/// A role of the current scheme as a GDK color.
pub fn rgba(role: &str) -> gdk::RGBA {
    gdk::RGBA::parse(current().get(role)).unwrap_or(gdk::RGBA::WHITE)
}

/// Applies the settings: the fast (cached) scheme now, then matugen's fresh one when it finishes.
pub fn apply_settings(t: &ThemeSettings) {
    apply(&resolve_fast(t));
    let uses_matugen = matches!(t.source, ThemeSource::Auto | ThemeSource::Matugen { .. }) && matugen_available();
    if uses_matugen {
        let t = t.clone();
        glib_spawn_blocking(move || resolve_full(&t), |r| match r {
            Ok(s) => apply(&s),
            Err(e) => tracing::warn!("theme: {e}"),
        });
    }
}

fn glib_spawn_blocking<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static, done: impl FnOnce(T) + 'static) {
    let (tx, rx) = futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    gtk::glib::spawn_future_local(async move {
        if let Ok(v) = rx.await {
            done(v);
        }
    });
}
