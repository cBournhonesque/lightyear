use crate::channel::builder::{Channel, EntityActionsChannel, EntityUpdatesChannel};
use crate::netcode::ClientId;
use crate::protocol::channel::ChannelKind;
use bevy::prelude::Component;
use serde::{Deserialize, Serialize};

/// Component inserted to each replicable entities, to detect when they are despawned
#[derive(Component, Clone, Copy)]
pub struct DespawnTracker;

/// Component that indicates that an entity should be replicated. Added to the entity when it is spawned
/// in the world that sends replication updates.
#[derive(Component, Clone, Copy)]
pub struct Replicate {
    // TODO: be able to specify channel separately for entiy spawn/despawn and component insertion/removal?
    /// The channel to use for replication (indicated by the generic type)
    pub actions_channel: ChannelKind,
    pub updates_channel: ChannelKind,

    /// Which clients should this entity be replicated to
    pub replication_target: NetworkTarget,
    /// Which clients should predict this entity
    pub prediction_target: NetworkTarget,
    /// Which clients should interpolated this entity
    pub interpolation_target: NetworkTarget,
    // pub owner:
    // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
    //  it just keeps living but doesn't receive any updates. Should we make this configurable?
}

impl Replicate {
    pub fn with_channel<C: Channel>() -> Self {
        Self {
            actions_channel: ChannelKind::of::<C>(),
            updates_channel: ChannelKind::of::<C>(),
            ..Default::default()
        }
    }
}

impl Default for Replicate {
    fn default() -> Self {
        Self {
            actions_channel: ChannelKind::of::<EntityActionsChannel>(),
            updates_channel: ChannelKind::of::<EntityUpdatesChannel>(),
            replication_target: NetworkTarget::All,
            prediction_target: NetworkTarget::None,
            interpolation_target: NetworkTarget::None,
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
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShouldBeInterpolated;

// TODO: Right now we use the approach that we add an extra component to the Protocol of components to be replicated.
//  that's pretty dangerous because it's now hard for the user to derive new traits.
//  let's think of another approach later.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShouldBePredicted;
