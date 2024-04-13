use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;
use bevy_xpbd_2d::prelude::*;
use leafwing_input_manager::prelude::*;
use lightyear::_reexport::ServerMarker;

use lightyear::prelude::client::{Confirmed, Predicted};
pub use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_movement_behaviour, FixedSet};
use crate::{shared, ServerTransports, SharedSettings};

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
        // add leafwing plugins to handle inputs
        app.add_plugins((
            LeafwingInputPlugin::<MyProtocol, PlayerActions>::default(),
            // LeafwingInputPlugin::<MyProtocol, AdminActions>::default(),
        ));
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
        app.add_systems(FixedUpdate, movement.in_set(FixedSet::Main));
        app.add_systems(Update, handle_disconnections);
    }
}

/// System to start the server at Startup
fn start_server(world: &mut World) {
    world.start_server().expect("Failed to start server");
}

fn init(mut commands: Commands, global: Res<Global>) {
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

    // the ball is server-authoritative
    commands.spawn(BallBundle::new(
        Vec2::new(0.0, 0.0),
        Color::AZURE,
        // if true, we predict the ball on clients
        global.predict_all,
    ));
}

/// Server disconnection system, delete all player entities upon disconnection
pub(crate) fn handle_disconnections(
    mut disconnections: EventReader<DisconnectEvent>,
    mut commands: Commands,
    player_entities: Query<(Entity, &PlayerId)>,
) {
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        for (entity, player_id) in player_entities.iter() {
            if player_id.0 == *client_id {
                commands.entity(entity).despawn();
            }
        }
    }
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
            info!(?entity, tick = ?tick_manager.tick(), ?position, actions = ?action.get_pressed(), "applying movement to player");
        }
    }
}

// Replicate the pre-spawned entities back to the client
pub(crate) fn replicate_players(
    global: Res<Global>,
    mut commands: Commands,
    mut player_spawn_reader: EventReader<ComponentInsertEvent<PlayerId>>,
) {
    for event in player_spawn_reader.read() {
        let client_id = *event.context();
        let entity = event.entity();
        info!("received player spawn event: {:?}", event);

        // for all cursors we have received, add a Replicate component so that we can start replicating it
        // to other clients
        if let Some(mut e) = commands.get_entity(entity) {
            let mut replicate = Replicate {
                // we want to replicate back to the original client, since they are using a pre-predicted entity
                replication_target: NetworkTarget::All,
                // make sure that all entities that are predicted are part of the same replication group
                replication_group: REPLICATION_GROUP,
                ..default()
            };
            // We don't want to replicate the ActionState to the original client, since they are updating it with
            // their own inputs (if you replicate it to the original client, it will be added on the Confirmed entity,
            // which will keep syncing it to the Predicted entity because the ActionState gets updated every tick)!
            replicate.add_target::<ActionState<PlayerActions>>(NetworkTarget::AllExceptSingle(
                client_id,
            ));
            // if we receive a pre-predicted entity, only send the prepredicted component back
            // to the original client
            replicate.add_target::<PrePredicted>(NetworkTarget::Single(client_id));
            if global.predict_all {
                replicate.prediction_target = NetworkTarget::All;
                // // if we predict other players, we need to replicate their actions to all clients other than the original one
                // // (the original client will apply the actions locally)
                // replicate.disable_replicate_once::<ActionState<PlayerActions>>();
            } else {
                // we want the other clients to apply interpolation for the player
                replicate.interpolation_target = NetworkTarget::AllExceptSingle(client_id);
            }
            e.insert((
                replicate,
                // not all physics components are replicated over the network, so add them on the server as well
                PhysicsBundle::player(),
            ));
        }
    }
}
