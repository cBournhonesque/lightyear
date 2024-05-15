use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::prelude::*;

use lightyear::client::prediction::Predicted;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_player_movement};

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // Re-adding Replicate components to client-replicated entities must be done in this set for proper handling.
        app.add_systems(
            PreUpdate,
            replicate_players.in_set(ServerReplicationSet::ClientReplication),
        );
        // the physics/FixedUpdates systems that consume inputs should be run in the `FixedUpdate` schedule
        // app.add_systems(FixedUpdate, player_movement);
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.start_server();
    commands.spawn(
        TextBundle::from_section(
            "Server",
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE,
                ..default()
            },
        )
        .with_style(Style {
            align_self: AlignSelf::End,
            ..default()
        }),
    );
}

// // The client input only gets applied to predicted entities that we own
// // This works because we only predict the user's controlled entity.
// // If we were predicting more entities, we would have to only apply movement to the player owned one.
// fn player_movement(
//     tick_manager: Res<TickManager>,
//     mut player_query: Query<(&mut Transform, &ActionState<PlayerActions>, &PlayerId)>,
// ) {
//     for (transform, action_state, player_id) in player_query.iter_mut() {
//         shared_player_movement(transform, action_state);
//         // info!(tick = ?tick_manager.tick(), ?transform, actions = ?action_state.get_pressed(), "applying movement to predicted player");
//     }
// }

// Replicate the pre-spawned entities back to the client
pub(crate) fn replicate_players(
    mut commands: Commands,
    replicated_players: Query<(Entity, &Replicated), (Added<Replicated>, With<PlayerId>)>,
) {
    for (entity, replicated) in replicated_players.iter() {
        let client_id = replicated.client_id();
        debug!("received player spawn event from client {client_id:?}");
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
                // let the server know that this entity is controlled by client `client_id`
                // - the client will have a Controlled component for this entity when it's replicated
                // - when the client disconnects, this entity will be despawned
                controlled_by: ControlledBy {
                    target: NetworkTarget::Single(client_id),
                },
                // make sure that all predicted entities (i.e. all entities for a given client) are part of the same replication group
                group: ReplicationGroup::new_id(client_id.to_bits()),
                ..default()
            };
            e.insert((
                replicate,
                // We don't want to replicate the ActionState to the original client, since they are updating it with
                // their own inputs (if you replicate it to the original client, it will be added on the Confirmed entity,
                // which will keep syncing it to the Predicted entity because the ActionState gets updated every tick)!
                // We also don't need the inputs of the other clients, because we are not predicting them
                OverrideTargetComponent::<ActionState<PlayerActions>>::new(NetworkTarget::None),
                // The PrePredicted component must be replicated only to the original client
                OverrideTargetComponent::<PrePredicted>::new(NetworkTarget::Single(client_id)),
            ));
        }
    }
}
