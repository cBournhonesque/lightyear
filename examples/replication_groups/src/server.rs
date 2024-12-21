use bevy::prelude::*;
use bevy::utils::Duration;
use bevy::utils::HashMap;

use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
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
    mut position_query: Query<(&ControlledBy, &mut PlayerPosition)>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    tick_manager: Res<TickManager>,
) {
    for input in input_reader.read() {
        let client_id = input.context();
        if let Some(input) = input.input() {
            debug!(
                "Receiving input: {:?} from client: {:?} on tick: {:?}",
                input,
                client_id,
                tick_manager.tick()
            );
            // NOTE: you can define a mapping from client_id to entity_id to avoid iterating through all
            //  entities here
            for (controlled_by, position) in position_query.iter_mut() {
                if controlled_by.targets(client_id) {
                    shared::shared_movement_behaviour(position, input);
                }
            }
        }
    }
}
