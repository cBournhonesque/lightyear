use crate::components::Replicated;
use bevy::prelude::*;
use lightyear_connection::client::Disconnected;

pub struct ClientPlugin;

impl ClientPlugin {

    // TODO: this also affects ClientOf
    fn handle_disconnect(
        trigger: Trigger<OnAdd, Disconnected>,
        replicated_query: Query<(Entity, &Replicated)>,
        mut commands: Commands,
    ) {
        // TODO: this should also happen if the ReplicationReceiver is despawned?
        // despawn any entities that were spawned from replication
        replicated_query.iter().for_each(|(entity, replicated)| {
            // TODO: how to avoid this O(n) check? should the replication-receiver maintain a list of received entities?
            if replicated.receiver == trigger.target() {
                if let Ok(mut commands) = commands.get_entity(entity) {
                    commands.despawn();
                }
            }
        });
    }
}

impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {

    }
}