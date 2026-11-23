//! Things that run for the whole session, outside any page.

pub mod alerts;
#[cfg(unix)]
pub mod tray;
#[cfg(windows)]
#[path = "tray_windows.rs"]
pub mod tray;
mod tray_menu;
mod window_settings;
