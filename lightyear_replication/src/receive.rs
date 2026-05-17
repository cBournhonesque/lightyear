use crate::ReplicationSystems;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_replicon::client::ClientSystems;
use bevy_replicon::client::confirm_history::ConfirmHistory;
// TODO: add special rules so that entities with Predicted/Interpolation apply components differently

/// Replicated is used as a marker component to find entities that were replicated from a remote.
///
/// replicon always adds a [`ConfirmHistory`] component on replicated entities, so we can just use that.
pub type Replicated = ConfirmHistory;

/// Marker component added to a link entity to enable incoming replication.
///
/// A link entity represents a connection to a remote peer. Adding
/// `ReplicationReceiver` to it allows the replication systems to process
/// entity data received through that connection.
///
/// On the server, this is only needed if you want to receive client-to-server
/// entity replication (e.g. for [`PreSpawned`](crate::prespawn::PreSpawned)
/// entities). For normal server-to-client replication, only
/// [`ReplicationSender`](crate::send::ReplicationSender) is required on the
/// server side.
#[derive(Component, Default)]
pub struct ReplicationReceiver;

pub struct ReceivePlugin;
impl Plugin for ReceivePlugin {
    fn build(&self, app: &mut App) {
        // make sure that any ordering relative to ReplicationSystems is also applied to ClientSystems
        app.configure_sets(
            PreUpdate,
            ClientSystems::Receive.in_set(ReplicationSystems::Receive),
        );
        app.configure_sets(
            PostUpdate,
            ClientSystems::Receive.in_set(ReplicationSystems::Receive),
        );
    }
}
