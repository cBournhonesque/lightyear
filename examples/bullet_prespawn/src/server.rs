use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;
use leafwing_input_manager::prelude::*;

use lightyear::client::prediction::Predicted;
pub use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear::server::config::PacketConfig;

use crate::protocol::*;
use crate::shared::{color_from_id, shared_config, shared_player_movement};
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
            .add(LeafwingInputPlugin::<MyProtocol, PlayerActions>::default())
            .add(LeafwingInputPlugin::<MyProtocol, AdminActions>::default())
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
            replicate_players.in_set(MainSet::ClientReplication),
        );
        // the physics/FixedUpdates systems that consume inputs should be run in the `FixedUpdate` schedule
        // app.add_systems(FixedUpdate, player_movement);
        app.add_systems(Update, handle_disconnections);
    }
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
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
    mut player_spawn_reader: EventReader<ComponentInsertEvent<PlayerId>>,
) {
    for event in player_spawn_reader.read() {
        debug!("received player spawn event: {:?}", event);
        let client_id = event.context();
        let entity = event.entity();

        if let Some(mut e) = commands.get_entity(entity) {
            let mut replicate = Replicate {
                // we want to replicate back to the original client, since they are using a pre-spawned entity
                replication_target: NetworkTarget::All,
                // NOTE: even with a pre-spawned Predicted entity, we need to specify who will run prediction
                prediction_target: NetworkTarget::Single(*client_id),
                // we want the other clients to apply interpolation for the player
                interpolation_target: NetworkTarget::AllExceptSingle(*client_id),
                // make sure that all predicted entities (i.e. all entities for a given client) are part of the same replication group
                replication_group: ReplicationGroup::new_id(*client_id),
                ..default()
            };
            // We don't want to replicate the ActionState to the original client, since they are updating it with
            // their own inputs (if you replicate it to the original client, it will be added on the Confirmed entity,
            // which will keep syncing it to the Predicted entity because the ActionState gets updated every tick)!
            // We also don't need the inputs of the other clients, because we are not predicting them
            replicate.add_target::<ActionState<PlayerActions>>(NetworkTarget::None);
            e.insert(replicate);
        }
    }
}
