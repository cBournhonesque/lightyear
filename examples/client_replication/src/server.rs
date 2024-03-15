use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;

pub use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_movement_behaviour};
use crate::{shared, ServerTransports, SharedSettings};

// Plugin group to add all server-related plugins
pub struct ServerPluginGroup {
    pub(crate) lightyear: ServerPlugin<MyProtocol>,
}

impl ServerPluginGroup {
    pub(crate) fn new(net_configs: Vec<NetConfig>) -> ServerPluginGroup {
        let config = ServerConfig {
            shared: shared_config(),
            net: net_configs,
            ..default()
        };
        let plugin_config = PluginConfig::new(config, protocol());
        ServerPluginGroup {
            lightyear: ServerPlugin::new(plugin_config),
        }
    }
}

impl PluginGroup for ServerPluginGroup {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(self.lightyear)
            .add(ExampleServerPlugin)
            .add(shared::SharedPlugin)
    }
}

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, init);
        // Re-adding Replicate components to client-replicated entities must be done in this set for proper handling.
        app.add_systems(
            PreUpdate,
            (replicate_cursors, replicate_players).in_set(MainSet::ClientReplication),
        );
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, (movement, delete_player));
        app.add_systems(Update, handle_disconnections);
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Server",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
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
pub(crate) fn movement(
    mut position_query: Query<(&mut PlayerPosition, &PlayerId)>,
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

// Replicate the pre-spawned entities back to the client
// Note that this needs to run before FixedUpdate, since we handle client inputs in the FixedUpdate schedule (subject to change)
// And we want to handle deletion properly
pub(crate) fn replicate_players(
    mut commands: Commands,
    mut player_spawn_reader: EventReader<ComponentInsertEvent<PlayerPosition>>,
) {
    for event in player_spawn_reader.read() {
        debug!("received player spawn event: {:?}", event);
        let client_id = event.context();
        let entity = event.entity();

        // for all cursors we have received, add a Replicate component so that we can start replicating it
        // to other clients
        if let Some(mut e) = commands.get_entity(entity) {
            e.insert(Replicate {
                // we want to replicate back to the original client, since they are using a pre-spawned entity
                replication_target: NetworkTarget::All,
                // NOTE: even with a pre-spawned Predicted entity, we need to specify who will run prediction
                // NOTE: Be careful to not override the pre-spawned prediction! we do not need to enable prediction
                //  because there is a pre-spawned predicted entity
                prediction_target: NetworkTarget::Only(vec![*client_id]),
                // we want the other clients to apply interpolation for the player
                interpolation_target: NetworkTarget::AllExcept(vec![*client_id]),
                ..default()
            });
        }
    }
}

pub(crate) fn replicate_cursors(
    mut commands: Commands,
    mut cursor_spawn_reader: EventReader<ComponentInsertEvent<CursorPosition>>,
) {
    for event in cursor_spawn_reader.read() {
        info!("received cursor spawn event: {:?}", event);
        let client_id = event.context();
        let entity = event.entity();

        // for all cursors we have received, add a Replicate component so that we can start replicating it
        // to other clients
        if let Some(mut e) = commands.get_entity(entity) {
            e.insert(Replicate {
                // do not replicate back to the owning entity!
                replication_target: NetworkTarget::AllExcept(vec![*client_id]),
                // we want the other clients to apply interpolation for the cursor
                interpolation_target: NetworkTarget::AllExcept(vec![*client_id]),
                ..default()
            });
        }
    }
}
