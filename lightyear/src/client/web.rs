//! Module containing extra behaviour that we need when running in wasm

use bevy::prelude::*;
use bevy_web_keepalive::WebKeepalivePlugin;

/// In wasm, the main thread gets quickly throttled by the browser when it is hidden (e.g. when the user switches tabs).
/// This means that the app.update() function will not be called, because bevy's scheduler only runs `app.update()` when
/// the browser's requestAnimationFrame is called. (and that happens only when the tab is visible)
///
/// This is problematic because:
/// - we stop sending packets so the server disconnects the client because it doesn't receive keep-alives
/// - when the client reconnects, it also disconnects because it hasn't been receiving server packets
/// - the internal transport buffers can overflow because they are not being emptied
///
/// This solution spawns a WebWorker (a background thread) which is not throttled, and which runs
/// `app.update()` at a fixed interval. This way, the client can keep sending and receiving packets,
/// and updating the local World.
pub(crate) struct WebPlugin;

pub use bevy_web_keepalive::KeepaliveSettings;

impl Plugin for WebPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(WebKeepalivePlugin {
            // The interval is in milliseconds. We can run app.update() infrequently when in the background
            initial_wake_delay: 1000.0,
        });
    }
}
