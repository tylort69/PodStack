//! Desktop alerts (training stopped, injector ready, new mail, …) via freedesktop notifications.
//!
//! STUB: replaced by its feature work. Keep the public signatures.

use podstack_core::Core;
use std::sync::Arc;

/// Starts watching for alert conditions (called once at startup, on the GTK thread).
pub fn start(_core: Arc<Core>) {}
