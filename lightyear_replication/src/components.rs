//! Components used for replication

use crate::send::components::ComponentReplicationOverride;
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use lightyear_core::id::PeerId;
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
    /// The peer that originally spawned the entity
    pub from: PeerId,
}

/// Marker component that indicates that the entity is being replicated
/// from a remote world.
///
/// The component only exists while the peer does not have authority over
/// the entity.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct Replicated {
    /// Entity that holds the ReplicationReceiver for this entity
    pub receiver: Entity,
    /// The remote peer that is actively replicating the entity
    pub from: PeerId,
}

// TODO: we need a ReplicateConfig similar to ComponentReplicationConfig
//  for entity-specific config, such as replicate-hierarchy

/// Marks an entity that directly applies the replication updates from the remote
///
/// In general, when an entity is replicated from the server to the client, multiple entities can be created on the client:
/// - an entity that simply contains the replicated components. It will have the marker component [`Confirmed`]
/// - an entity that is in the future compared to the confirmed entity, and does prediction with rollback. It will have the marker component [`Predicted`](lightyear_core::prediction::Predicted)
/// - an entity that is in the past compared to the confirmed entity and interpolates between multiple server updates. It will have the marker component [`Interpolated`](lightyear_core::interpolation::Interpolated)
#[derive(Component, Reflect, Default, Debug)]
#[reflect(Component)]
pub struct Confirmed {
    /// The corresponding Predicted entity
    pub predicted: Option<Entity>,
    /// The corresponding Interpolated entity
    pub interpolated: Option<Entity>,
    /// The tick that the confirmed entity is at.
    /// (this is latest server tick for which we applied updates to the entity)
    pub tick: Tick,
}

// TODO: enable this only if predicted feature
/// Indicates that an entity was pre-predicted
// NOTE: we do not map entities for this component, we want to receive the entities as is
//  because we already do the mapping at other steps
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct PrePredicted {
    pub confirmed_entity: Option<Entity>,
}
