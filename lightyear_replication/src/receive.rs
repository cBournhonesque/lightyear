use bevy_app::{App, Plugin, PostUpdate};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_replicon::client::ClientSystems;
use bevy_replicon::client::confirm_history::ConfirmHistory;
use crate::ReplicationSystems;
// TODO: add special rules so that entities with Predicted/Interpolation apply components differently



/// Marks an entity that directly applies the replication updates from the remote
///
/// In general, when an entity is replicated from the server to the client, multiple entities can be created on the client:
/// - an entity that simply contains the replicated components. It will have the marker component [`Confirmed`]
/// - an entity that is in the future compared to the confirmed entity, and does prediction with rollback. It will have the marker component [`Predicted`]
/// - an entity that is in the past compared to the confirmed entity and interpolates between multiple server updates. It will have the marker component [`Interpolated`]
#[derive(Deref, DerefMut, Component, Reflect, PartialEq, Default, Debug, Clone)]
#[reflect(Component)]
pub struct Confirmed<C>(pub C);

/// Replicated is used as a marker component to find entities that were replicated from a remote.
///
/// replicon always adds a [`ConfirmHistory`] component on replicated entities, so we can just use that.
pub type Replicated = ConfirmHistory;

pub struct ReceivePlugin;
impl Plugin for ReceivePlugin {
    fn build(&self, app: &mut App) {
        // make sure that any ordering relative to ReplicationSystems is also applied to ClientSystems
        app.configure_sets(PostUpdate, ClientSystems::Receive.in_set(ReplicationSystems::Receive));
    }
}
