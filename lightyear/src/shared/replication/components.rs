//! Components used for replication
use crate::channel::builder::{Channel, EntityActionsChannel, EntityUpdatesChannel};
use crate::netcode::ClientId;
use crate::protocol::channel::ChannelKind;
use crate::server::room::{ClientVisibility, RoomId};
use bevy::prelude::{Component, Entity};
use lightyear_macros::MessageInternal;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Component inserted to each replicable entities, to detect when they are despawned
#[derive(Component, Clone, Copy)]
pub struct DespawnTracker;

/// Component that indicates that an entity should be replicated. Added to the entity when it is spawned
/// in the world that sends replication updates.
#[derive(Component, Clone)]
pub struct Replicate {
    /// Which clients should this entity be replicated to
    pub replication_target: NetworkTarget,
    /// Which clients should predict this entity
    pub prediction_target: NetworkTarget,
    /// Which clients should interpolated this entity
    pub interpolation_target: NetworkTarget,

    // TODO: this should not be public, but replicate is public... how to fix that?
    //  have a separate component ReplicateVisibility?
    //  or force users to use `Replicate::default().with...`?
    /// List of clients that we the entity is currently replicated to.
    /// Will be updated before the other replication systems
    pub replication_clients_cache: HashMap<ClientId, ClientVisibility>,
    pub replication_mode: ReplicationMode,
    // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
    //  it just keeps living but doesn't receive any updates. Should we make this configurable?
    pub replication_group: ReplicationGroup,
}

impl Replicate {
    pub(crate) fn group_id(&self, entity: Option<Entity>) -> ReplicationGroupId {
        match self.replication_group {
            ReplicationGroup::FromEntity => {
                ReplicationGroupId(entity.expect("need to provide an entity").to_bits())
            }
            ReplicationGroup::Group(id) => ReplicationGroupId(id),
        }
    }
}

#[derive(Default)]
pub enum ReplicationGroup {
    // the group id is the entity id
    #[default]
    FromEntity,
    // choose a different group id
    // note: it must not be the same as any entity id!
    // TODO: how can i generate one that doesn't conflict? maybe take u32 as input, and apply generation = u32::MAX - 1?
    Group(u64),
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReplicationGroupId(u64);

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum ReplicationMode {
    /// We will replicate this entity only to clients that are in the same room as the entity
    Room,
    /// We will replicate this entity to clients using only the [`NetworkTarget`], without caring about rooms
    #[default]
    NetworkTarget,
}

impl Default for Replicate {
    fn default() -> Self {
        Self {
            replication_target: NetworkTarget::All,
            prediction_target: NetworkTarget::None,
            interpolation_target: NetworkTarget::None,
            replication_clients_cache: HashMap::new(),
            replication_mode: ReplicationMode::default(),
            replication_group: Default::default(),
        }
    }
}

#[derive(Default, Clone, Copy)]
/// NetworkTarget indicated which clients should receive some message
pub enum NetworkTarget {
    #[default]
    /// Message sent to no client
    None,
    /// Message sent to all clients except for one
    AllExcept(ClientId),
    /// Message sent to all clients
    All,
    /// Message sent to only one client
    Only(ClientId),
}

impl NetworkTarget {
    /// Return true if we should replicate to the specified client
    pub(crate) fn should_send_to(&self, client_id: &ClientId) -> bool {
        match self {
            NetworkTarget::All => true,
            NetworkTarget::AllExcept(id) => id != client_id,
            NetworkTarget::Only(id) => id == client_id,
            NetworkTarget::None => false,
        }
    }
}

// TODO: Right now we use the approach that we add an extra component to the Protocol of components to be replicated.
//  that's pretty dangerous because it's now hard for the user to derive new traits.
//  let's think of another approach later.
#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShouldBeInterpolated;

// TODO: Right now we use the approach that we add an extra component to the Protocol of components to be replicated.
//  that's pretty dangerous because it's now hard for the user to derive new traits.
//  let's think of another approach later.
#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShouldBePredicted;
