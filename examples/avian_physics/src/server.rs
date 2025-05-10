use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour};
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::client::{Confirmed, Predicted};
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::server::input::InputSystemSet;
use lightyear::shared::replication::components::InitialReplicated;

// Plugin for server-specific logic
pub struct ExampleServerPlugin {
    pub(crate) predict_all: bool,
}

#[derive(Resource)]
pub struct Global {
    predict_all: bool,
}

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Global {
            predict_all: self.predict_all,
        });

        app.add_systems(Startup, (start_server, init));

        // Re-adding Replicate components to client-replicated entities must be done in this set for proper handling.
        app.add_systems(
            PreUpdate,
            replicate_players.in_set(ServerReplicationSet::ClientReplication),
        );
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement);
    }
}

/// System to start the server at Startup
fn start_server(mut commands: Commands) {
    commands.start_server();
}

fn init(mut commands: Commands, global: Res<Global>) {
    // the ball is server-authoritative
    commands.spawn(BallBundle::new(
        Vec2::new(0.0, 0.0),
        css::AZURE.into(),
        // if true, we predict the ball on clients
        global.predict_all,
    ));
}

/// Read client inputs and move players
/// NOTE: this system can now be run in both client/server!
pub(crate) fn movement(
    tick_manager: Res<TickManager>,
    mut action_query: Query<
        (
            Entity,
            &Position,
            &mut LinearVelocity,
            &ActionState<PlayerActions>,
        ),
        // if we run in host-server mode, we don't want to apply this system to the local client's entities
        // because they are already moved by the client plugin
        (Without<Confirmed>, Without<Predicted>),
    >,
) {
    for (entity, position, velocity, action) in action_query.iter_mut() {
        if !action.get_pressed().is_empty() {
            // NOTE: be careful to directly pass Mut<PlayerPosition>
            // getting a mutable reference triggers change detection, unless you use `as_deref_mut()`
            shared_movement_behaviour(velocity, action);
            trace!(?entity, tick = ?tick_manager.tick(), ?position, actions = ?action.get_pressed(), "applying movement to player");
        }
    }
}

// Replicate the pre-predicted entities back to the client
// We have to use `InitialReplicated` instead of `Replicated`, because
// the server has already assumed authority over the entity so the `Replicated` component
// has been removed
pub(crate) fn replicate_players(
    global: Res<Global>,
    mut commands: Commands,
    query: Query<(Entity, &InitialReplicated), (Added<InitialReplicated>, With<PlayerId>)>,
) {
    for (entity, replicated) in query.iter() {
        let client_id = replicated.client_id();
        info!(
            "Received player spawn event from client {client_id:?}. Replicating back to all clients"
        );

        // for all player entities we have received, add a Replicate component so that we can start replicating it
        // to other clients
        if let Ok(mut e) = commands.get_entity(entity) {
            // we want to replicate back to the original client, since they are using a pre-predicted entity
            let mut sync_target = SyncTarget::default();
            if global.predict_all {
                sync_target.prediction = NetworkTarget::All;
            } else {
                // we want the other clients to apply interpolation for the player
                sync_target.interpolation = NetworkTarget::AllExceptSingle(client_id);
            }
            e.insert((
                ReplicateToClient::default(),
                sync_target,
                // make sure that all entities that are predicted are part of the same replication group
                REPLICATION_GROUP,
                ControlledBy {
                    target: NetworkTarget::Single(client_id),
                    ..default()
                },
                // if we receive a pre-predicted entity, only send the prepredicted component back
                // to the original client
                OverrideTarget::default().insert::<PrePredicted>(NetworkTarget::Single(client_id)),
                // not all physics components are replicated over the network, so add them on the server as well
                PhysicsBundle::player(),
            ));
        }
    }
}
