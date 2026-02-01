use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_replicon::client::{ClientSystems};
use bevy_replicon::client::confirm_history::ConfirmHistory;
use crate::ReplicationSystems;
// TODO: add special rules so that entities with Predicted/Interpolation apply components differently

/// Replicated is used as a marker component to find entities that were replicated from a remote.
///
/// replicon always adds a [`ConfirmHistory`] component on replicated entities, so we can just use that.
pub type Replicated = ConfirmHistory;

/// Marker component to indicate that this peer is allowed to receive replication messages
#[derive(Component, Default)]
pub struct ReplicationReceiver;


pub struct ReceivePlugin;
impl Plugin for ReceivePlugin {
    fn build(&self, app: &mut App) {
        // make sure that any ordering relative to ReplicationSystems is also applied to ClientSystems
        app.configure_sets(PostUpdate, ClientSystems::Receive.in_set(ReplicationSystems::Receive));
    }
}
