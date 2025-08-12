use crate::protocol::*;
use crate::shared;
use crate::shared::{
    SharedPlugin, WallBundle, color_from_id, player_bundle, shared_movement_behaviour,
};
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
        app.add_observer(handle_connected);
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

pub(crate) fn handle_connected(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<&RemoteId, With<ClientOf>>,
    mut commands: Commands,
) {
    let Ok(remote_id) = query.get(trigger.target()) else {
        return;
    };
    info!("Spawning player for client {:?}", remote_id);
    // we spawn an entity that will be replicated to all clients
    commands.spawn((
        Replicate::to_clients(NetworkTarget::All),
        PlayerId(remote_id.0),
        player_bundle(remote_id.0),
    ));
}
