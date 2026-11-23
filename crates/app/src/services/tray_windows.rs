//! The tray icon on Windows: not built yet, so PodStack runs without one and closing the window quits.

pub use super::window_settings::{WindowSettings, SETTINGS_KEY};
use podstack_core::Core;
use std::sync::Arc;

pub fn start(_app: &gtk::Application, _window: &gtk::ApplicationWindow, _core: Arc<Core>) {}

/// Whether the tray icon is showing.
pub fn is_running() -> bool {
    false
}

/// Whether closing the window should hide it to the tray instead of quitting.
pub fn close_to_tray(_core: &Core) -> bool {
    false
}
