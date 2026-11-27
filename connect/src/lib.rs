//! Everything the tray app does that isn't a window: settings files, the server client and
//! the connection state. Kept apart so it can be tested without a UI.

pub mod http;
pub mod paths;
pub mod session;
pub mod settings;
