//! Handling pausing connections for clients on the web.
//!
//! When a client on a browser switches to a different tab, the browser will throttle the bevy tab
//! (which is now in the background) to save resources. This means that the bevy schedule will no longer
//! run.

use crate::connection::server::ServerConnections;
use crate::server::events::MessageEvent;
use crate::shared::sets::{InternalMainSet, ServerMarker};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::error;

#[derive(Serialize, Deserialize)]
pub(crate) struct PauseMessage {
    pub(crate) paused: bool,
}

pub struct PausePlugin;

impl Plugin for PausePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            receive_pause_message.after(InternalMainSet::<ServerMarker>::EmitEvents),
        );
    }
}

fn receive_pause_message(
    mut messages: EventReader<MessageEvent<PauseMessage>>,
    mut server_connections: ResMut<ServerConnections>,
) {
    for message in messages.read() {
        let client_id = message.context;
        if message.message.paused {
            error!("Received pause message from client: {}", client_id);
            // Pause the connection
            server_connections.paused_clients.insert(client_id);
        } else {
            // Unpause the game
            error!("Received unpause message from client: {}", client_id);
            server_connections.paused_clients.remove(&client_id);
        }
    }
}
