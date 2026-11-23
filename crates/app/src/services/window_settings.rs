//! Window and tray settings, shared by the tray icon on every platform and Settings → General.

use serde::{Deserialize, Serialize};

/// Settings key (`core.setting::<WindowSettings>(SETTINGS_KEY)`).
pub const SETTINGS_KEY: &str = "window";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowSettings {
    pub tray_icon: bool,
    /// Closing the window hides it to the tray (when the tray icon is showing).
    pub close_to_tray: bool,
    /// Start with only the tray icon.
    pub start_hidden: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        WindowSettings { tray_icon: true, close_to_tray: false, start_hidden: false }
    }
}
