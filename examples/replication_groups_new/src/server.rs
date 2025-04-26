use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use lightyear::client::components::Confirmed;
use lightyear::client::prediction::Predicted;
use lightyear::inputs::native::ActionState;
use lightyear::prelude::server::*;

use crate::protocol::*;
use crate::shared::{shared_movement_behaviour, shared_tail_behaviour};

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // the simulation systems that can be rolled back must run in FixedUpdate
        app.add_systems(FixedUpdate, (movement, shared_tail_behaviour).chain());
        app.add_systems(Update, handle_connections);
    }
}

/// Start the server
pub(crate) fn init(mut commands: Commands) {
    commands.start_server();
}

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    mut connections: EventReader<ConnectEvent>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.client_id;
        // Generate pseudo random color from client id.
        let h = (((client_id.to_bits().wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let player_position = Vec2::ZERO;
        let player_entity = commands
            .spawn(PlayerBundle::new(client_id, player_position))
            .id();
        let tail_length = 300.0;
        let tail_entity = commands
            .spawn(TailBundle::new(
                client_id,
                player_entity,
                player_position,
                tail_length,
            ))
            .id();
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<
        (&mut PlayerPosition, &ActionState<Inputs>),
        // if we run in host-server mode, we don't want to apply this system to the local client's entities
        // because they are already moved by the client plugin
        (Without<Confirmed>, Without<Predicted>),
    >,
) {
    for (position, inputs) in position_query.iter_mut() {
        if let Some(input) = &inputs.value {
            // NOTE: be careful to directly pass Mut<PlayerPosition>
            // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
            shared_movement_behaviour(position, input);
        }
    }
}
