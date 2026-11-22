//! System tray icon (StatusNotifierItem, shown by waybar and similar bars).
//!
//! STUB: replaced by its feature work. Keep the public signatures.

use podstack_core::Core;
use std::sync::Arc;

/// Starts the tray icon if enabled in settings (called once at startup, on the GTK thread).
pub fn start(_app: &gtk::Application, _window: &gtk::ApplicationWindow, _core: Arc<Core>) {}

/// Whether closing the window should hide it to the tray instead of quitting.
pub fn close_to_tray(_core: &Core) -> bool {
    false
}
