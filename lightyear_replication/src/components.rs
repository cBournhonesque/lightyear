//! Components used for replication

use crate::send::components::ComponentReplicationOverride;
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
#[cfg(feature = "interpolation")]
use lightyear_core::prelude::Interpolated;
#[cfg(feature = "prediction")]
use lightyear_core::prelude::Predicted;
use lightyear_core::tick::Tick;
use lightyear_utils::collections::EntityHashMap;
use serde::{Deserialize, Serialize};
// TODO: how to define which subset of components a sender iterates through?
//  if a sender is only interested in a few components it might be expensive
//  maybe we can have a 'direction' in ComponentReplicationConfig and Client/ClientOf peers can precompute
//  a list of components based on this.

#[derive(Component, Reflect)]
pub struct ComponentReplicationOverrides<C> {
    /// Overrides that will be applied to all senders
    pub(crate) all_senders: Option<ComponentReplicationOverride>,
    /// Overrides that will be applied for a specific sender. Takes priority over `all_senders`
    pub(crate) per_sender: EntityHashMap<ComponentReplicationOverride>,
    _marker: core::marker::PhantomData<C>,
}

impl<C> Default for ComponentReplicationOverrides<C> {
    fn default() -> Self {
        Self {
            all_senders: None,
            per_sender: Default::default(),
            _marker: core::marker::PhantomData,
        }
    }
}

/// Marker component that indicates that the entity was initially spawned via replication
/// (it was being replicated from a remote world)
///
/// The component is added once and is then never modified anymore
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct InitialReplicated {
    /// Entity that holds the original [`ReplicationReceiver`](crate::receive::ReplicationReceiver) for this entity
    pub receiver: Entity,
}

/// Marker component that indicates that the entity is being replicated
/// from a remote world.
///
/// The component only exists while the peer does not have authority over
/// the entity.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct Replicated {
    /// Entity that holds the [`ReplicationReceiver`](crate::receive::ReplicationReceiver) for this entity
    pub receiver: Entity,
}

/// Marker component that can be added on the receiver side
/// to avoid despawning entities on [`crate::receive::ReplicationReceiver`] disconnect.
///
/// - If the component is added to a [`Replicated`] entity,
///   it won't be despawned on [`crate::receive::ReplicationReceiver`] disconnect.
/// - If the component is added to a [`crate::receive::ReplicationReceiver`] entity,
///   any related entities won't be despawned on disconnect.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct Persistent;

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ConfirmedTick {
    /// For replicated entities, this tick indicates
    /// the most recent tick where we applied a remote update to the entity
    pub tick: Tick,
}

// TODO: we need a ReplicateConfig similar to ComponentReplicationConfig
//  for entity-specific config, such as replicate-hierarchy

/// Marks an entity that directly applies the replication updates from the remote
///
/// In general, when an entity is replicated from the server to the client, multiple entities can be created on the client:
/// - an entity that simply contains the replicated components. It will have the marker component [`Confirmed`]
/// - an entity that is in the future compared to the confirmed entity, and does prediction with rollback. It will have the marker component [`Predicted`]
/// - an entity that is in the past compared to the confirmed entity and interpolates between multiple server updates. It will have the marker component [`Interpolated`]
#[derive(Deref, DerefMut, Component, Reflect, PartialEq, Default, Debug, Clone)]
#[reflect(Component)]
pub struct Confirmed<C>(pub C);
