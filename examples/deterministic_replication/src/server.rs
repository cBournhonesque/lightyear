use crate::protocol::*;
use crate::shared;
use crate::shared::{color_from_id, shared_movement_behaviour, SharedPlugin, WallBundle};
use avian2d::prelude::*;
use bevy::color::palettes::css;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use core::time::Duration;
use leafwing_input_manager::prelude::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::SEND_INTERVAL;

/// In this example, the server does not simulate anything, it simply acts as a relay server
/// that handles:
/// - receiving and broadcasting player inputs
/// - handling game start
/// - keeping timelines in sync
#[derive(Clone)]
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(handle_new_client);
        app.add_observer(start_game);
    }
}

pub(crate) fn handle_new_client(trigger: Trigger<OnAdd, LinkOf>, mut commands: Commands) {
    commands
        .entity(trigger.target())
        .insert(ReplicationSender::new(
            SEND_INTERVAL,
            SendUpdatesMode::SinceLastAck,
            false,
        ));
}


/// Currently we do not support clients joining mid-game for deterministic replication.
/// So we'll have a trigger to start the game for all existing clients.
#[derive(Event)]
pub(crate) struct GameStart;


// TODO: how can we achieve this without replication from the server?
//  if there is no server, we could have all clients spawn the same world at the same time?

/// When we decide to start the game, we will replicate player entities to all clients.
pub(crate) fn start_game(
    trigger: Trigger<GameStart>,
    server: Query<&Server, With<Started>>,
    mut commands: Commands,
    query: Query<&RemoteId, (With<ClientOf>, With<Connected>)>,
) {
    if let Ok(server) = server.get(trigger.target()) {
        server.collection().iter().for_each(|link| {
            if let Ok(remote_id) = query.get(*link) {
                info!("Spawning player for client {:?}", remote_id);
                // we spawn an entity that will be replicated to all clients
                commands.spawn((
                   Replicate::to_clients(NetworkTarget::All),
                   PlayerId(remote_id.0)
                ));
            } else {
                warn!("Failed to get entity for server link {:?}", link);
            }
        });
    } else {
        warn!("Cannot start game, no server found.");
        return;
    }
}