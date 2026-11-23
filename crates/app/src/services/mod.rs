//! Things that run for the whole session, outside any page.

pub mod alerts;
#[cfg(unix)]
pub mod tray;
#[cfg(windows)]
#[path = "tray_windows.rs"]
pub mod tray;
mod window_settings;
