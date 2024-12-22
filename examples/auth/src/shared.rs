//! This module contains the shared code between the client and the server.
//!
//! The rendering code is here because you might want to run the example in host-server mode, where the server also acts as a client.
//! The simulation logic (movement, etc.) should be shared between client and server to guarantee that there won't be
//! mispredictions/rollbacks.
use bevy::prelude::*;
use bevy::utils::Duration;

use lightyear::prelude::*;
use lightyear::shared::config::Mode;

use crate::protocol::*;

#[derive(Clone)]
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // the protocol needs to be shared between the client and server
        app.add_plugins(ProtocolPlugin);
    }
}
