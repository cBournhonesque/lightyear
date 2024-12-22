use bevy::prelude::*;
use bevy::utils::Duration;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour};
use lightyear::client::components::Confirmed;
use lightyear::client::interpolation::Interpolated;
use lightyear::client::prediction::Predicted;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::shared::replication::components::InitialReplicated;

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // Re-adding Replicate components to client-replicated entities must be done in this set for proper handling.
        app.add_systems(
            PreUpdate,
            (replicate_cursors, replicate_players).in_set(ServerReplicationSet::ClientReplication),
        );
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, (movement, delete_player));
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.start_server();
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<(&mut PlayerPosition, &PlayerId)>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    tick_manager: Res<TickManager>,
) {
    for input in input_reader.read() {
        let client_id = input.context();
        if let Some(input) = input.input() {
            if matches!(input, Inputs::None) {
                continue;
            }
            trace!(
                "Receiving input: {:?} from client: {:?} on tick: {:?}",
                input,
                client_id,
                tick_manager.tick()
            );

            for (position, player_id) in position_query.iter_mut() {
                if player_id.0 == *client_id {
                    // NOTE: be careful to directly pass Mut<PlayerPosition>
                    // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
                    shared_movement_behaviour(position, input);
                }
            }
        }
    }
}

fn delete_player(
    mut commands: Commands,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    query: Query<(Entity, &PlayerId), With<PlayerPosition>>,
) {
    for input in input_reader.read() {
        let client_id = input.context();
        if let Some(input) = input.input() {
            if matches!(input, Inputs::Delete) {
                debug!("received delete input!");
                for (entity, player_id) in query.iter() {
                    // NOTE: we could not accept the despawn (server conflict)
                    //  in which case the client would have to rollback to delete
                    if player_id.0 == *client_id {
                        // You can try 2 things here:
                        // - either you consider that the client's action is correct, and you despawn the entity. This should get replicated
                        //   to other clients.
                        // - you decide that the client's despawn is incorrect, and you do not despawn the entity. Then the client's prediction
                        //   should be rolled back, and the entity should not get despawned on client.
                        commands.entity(entity).despawn();
                    }
                }
            }
        }
    }
}

// Replicate the pre-predicted entities back to the client.
//
// The objective was to create a normal 'predicted' entity directly in the client timeline, instead
// of having to create the entity in the server timeline and wait for it to be replicated.
// Note that this needs to run before FixedUpdate, since we handle client inputs in the FixedUpdate schedule (subject to change)
// And we want to handle deletion properly
pub(crate) fn replicate_players(
    mut commands: Commands,
    replicated_players: Query<
        (Entity, &InitialReplicated),
        (With<PlayerPosition>, Added<InitialReplicated>),
    >,
) {
    for (entity, replicated) in replicated_players.iter() {
        let client_id = replicated.client_id();
        debug!("received player spawn event from {client_id:?}");
        // for all cursors we have received, add a Replicate component so that we can start replicating it
        // to other clients
        if let Some(mut e) = commands.get_entity(entity) {
            let replicate = Replicate {
                target: ReplicationTarget {
                    // we want to replicate back to the original client, since they are using a pre-spawned entity
                    target: NetworkTarget::All,
                },
                sync: SyncTarget {
                    // NOTE: even with a pre-spawned Predicted entity, we need to specify who will run prediction
                    prediction: NetworkTarget::Single(client_id),
                    // we want the other clients to apply interpolation for the player
                    interpolation: NetworkTarget::AllExceptSingle(client_id),
                },
                controlled_by: ControlledBy {
                    target: NetworkTarget::Single(client_id),
                    ..default()
                },
                ..default()
            };
            e.insert((
                replicate,
                // if we receive a pre-predicted entity, only send the prepredicted component back
                // to the original client
                OverrideTargetComponent::<PrePredicted>::new(NetworkTarget::Single(client_id)),
            ));
        }
    }
}

pub(crate) fn replicate_cursors(
    mut commands: Commands,
    replicated_cursor: Query<
        (Entity, &InitialReplicated),
        (With<CursorPosition>, Added<InitialReplicated>),
    >,
) {
    for (entity, replicated) in replicated_cursor.iter() {
        let client_id = replicated.client_id();
        info!("received cursor spawn event from client: {client_id:?}");
        // for all cursors we have received, add a Replicate component so that we can start replicating it
        // to other clients
        if let Some(mut e) = commands.get_entity(entity) {
            e.insert(Replicate {
                target: ReplicationTarget {
                    // do not replicate back to the client that owns the cursor!
                    target: NetworkTarget::AllExceptSingle(client_id),
                },
                authority: AuthorityPeer::Client(client_id),
                sync: SyncTarget {
                    // we want the other clients to apply interpolation for the cursor
                    interpolation: NetworkTarget::AllExceptSingle(client_id),
                    ..default()
                },
                controlled_by: ControlledBy {
                    target: NetworkTarget::Single(client_id),
                    ..default()
                },
                ..default()
            });
        }
    }
}
