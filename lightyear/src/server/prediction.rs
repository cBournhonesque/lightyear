//! Handles logic related to prespawning entities

use crate::prelude::server::{AuthorityCommandExt, AuthorityPeer};
use crate::prelude::{PrePredicted, Replicated, ServerConnectionManager};
use crate::server::networking::NetworkingState;
use bevy::prelude::*;
use tracing::debug;

/// When we receive an entity that a clients wants PrePredicted,
/// we immediately transfer authority back to the server. The server will replicate the PrePredicted
/// component back to the client. Upon receipt, the client will replace PrePredicted with Predicted.
///
/// The entity mapping is done on the client.
pub(crate) fn handle_pre_predicted(
    trigger: Trigger<OnAdd, PrePredicted>,
    mut commands: Commands,
    mut manager: ResMut<ServerConnectionManager>,
    q: Query<(Entity, &PrePredicted, &Replicated)>,
    server_state: Res<State<NetworkingState>>,
) {
    if server_state.get() != &NetworkingState::Started {
        return;
    }
    if let Ok((local_entity, pre_predicted, replicated)) = q.get(trigger.target()) {
        let sending_client = replicated.from.unwrap();
        // if the client who created the PrePredicted entity is the local client, no need to do anything!
        // (the client Observer already adds Predicted on the entity)
        if sending_client.is_local() {
            return;
        }
        let confirmed_entity = pre_predicted.confirmed_entity.unwrap();
        // update the mapping so that when we send updates, the server entity gets mapped
        // to the client's confirmed entity
        manager
            .connection_mut(sending_client)
            .unwrap()
            .replication_receiver
            .remote_entity_map
            .insert(confirmed_entity, local_entity);
        debug!(
            ?confirmed_entity,
            ?local_entity,
            "Received PrePredicted entity from client: {:?}. Transferring authority to server",
            replicated.from
        );
        commands
            .entity(local_entity)
            .transfer_authority(AuthorityPeer::Server);
    }
}
