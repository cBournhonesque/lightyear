//! Components used for replication
use bevy::prelude::{Component, Entity};
use bevy::utils::{EntityHashSet, HashMap, HashSet};
use serde::{Deserialize, Serialize};

use crate::_reexport::IntoKind;
use lightyear_macros::MessageInternal;

use crate::channel::builder::Channel;
use crate::client::components::{ComponentSyncMode, SyncComponent};
use crate::netcode::ClientId;
use crate::prelude::{EntityMapper, MapEntities};
use crate::protocol::Protocol;
use crate::server::room::ClientVisibility;

/// Component inserted to each replicable entities, to detect when they are despawned
#[derive(Component, Clone, Copy)]
pub struct DespawnTracker;

/// Component that indicates that an entity should be replicated. Added to the entity when it is spawned
/// in the world that sends replication updates.
#[derive(Component, Clone)]
pub struct Replicate<P: Protocol> {
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
    #[doc(hidden)]
    pub replication_clients_cache: HashMap<ClientId, ClientVisibility>,
    pub replication_mode: ReplicationMode,
    // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
    //  it just keeps living but doesn't receive any updates. Should we make this configurable?
    pub replication_group: ReplicationGroup,

    /// By default, all components will be ignored. You can add components to this list to make them
    /// not replicated for this specific entity
    pub disabled_components: HashSet<P::ComponentKinds>,
}

impl<P: Protocol> Replicate<P> {
    pub(crate) fn group_id(&self, entity: Option<Entity>) -> ReplicationGroupId {
        match self.replication_group {
            ReplicationGroup::FromEntity => {
                ReplicationGroupId(entity.expect("need to provide an entity").to_bits())
            }
            ReplicationGroup::Group(id) => ReplicationGroupId(id),
        }
    }

    /// Disable the replication of a component for this entity
    pub fn disable_component<C>(&mut self)
    where
        C: IntoKind<P::ComponentKinds>,
    {
        self.disabled_components.insert(C::into_kind());
    }

    /// Enable the replication of a component for this entity
    pub(crate) fn enable_component<C>(&mut self)
    where
        C: IntoKind<P::ComponentKinds>,
    {
        self.disabled_components.remove(&C::into_kind());
    }
}

#[derive(Debug, Default, Copy, Clone)]
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
pub struct ReplicationGroupId(pub u64);

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum ReplicationMode {
    /// We will replicate this entity only to clients that are in the same room as the entity
    Room,
    /// We will replicate this entity to clients using only the [`NetworkTarget`], without caring about rooms
    #[default]
    NetworkTarget,
}

impl<P: Protocol> Default for Replicate<P> {
    fn default() -> Self {
        Self {
            replication_target: NetworkTarget::All,
            prediction_target: NetworkTarget::None,
            interpolation_target: NetworkTarget::None,
            replication_clients_cache: HashMap::new(),
            replication_mode: ReplicationMode::default(),
            replication_group: Default::default(),
            disabled_components: HashSet::default(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
/// NetworkTarget indicated which clients should receive some message
pub enum NetworkTarget {
    #[default]
    /// Message sent to no client
    None,
    /// Message sent to all clients except for these
    AllExcept(Vec<ClientId>),
    /// Message sent to all clients
    All,
    /// Message sent to only these
    Only(Vec<ClientId>),
}

impl NetworkTarget {
    /// Return true if we should replicate to the specified client
    pub(crate) fn should_send_to(&self, client_id: &ClientId) -> bool {
        match self {
            NetworkTarget::All => true,
            NetworkTarget::AllExcept(client_ids) => !client_ids.contains(client_id),
            NetworkTarget::Only(client_ids) => client_ids.contains(client_id),
            NetworkTarget::None => false,
        }
    }

    pub(crate) fn exclude(&mut self, client_ids: Vec<ClientId>) {
        match self {
            NetworkTarget::All => {
                *self = NetworkTarget::AllExcept(client_ids);
            }
            NetworkTarget::AllExcept(existing_client_ids) => {
                let mut new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                client_ids.into_iter().for_each(|id| {
                    new_excluded_ids.insert(id);
                });
                *existing_client_ids = Vec::from_iter(new_excluded_ids);
            }
            NetworkTarget::Only(existing_client_ids) => {
                let mut new_ids = HashSet::from_iter(existing_client_ids.clone());
                client_ids.into_iter().for_each(|id| {
                    new_ids.remove(&id);
                });
                if new_ids.is_empty() {
                    *self = NetworkTarget::None;
                } else {
                    *existing_client_ids = Vec::from_iter(new_ids);
                }
            }
            NetworkTarget::None => {}
        }
    }
}

// TODO: Right now we use the approach that we add an extra component to the Protocol of components to be replicated.
//  that's pretty dangerous because it's now hard for the user to derive new traits.
//  let's think of another approach later.
#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShouldBeInterpolated;

impl SyncComponent for ShouldBeInterpolated {
    fn mode() -> ComponentSyncMode {
        ComponentSyncMode::None
    }
}

// TODO: Right now we use the approach that we add an extra component to the Protocol of components to be replicated.
//  that's pretty dangerous because it's now hard for the user to derive new traits.
//  let's think of another approach later.
// NOTE: we do not map entities for this component, we want to receive the entities as is
#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShouldBePredicted {
    // TODO: rename this?
    //  - also the server already gets the client entity in the message, so it's a waste of space...
    //  - maybe use a different component: ClientToServer -> Prespawned (None)
    //  - ServerToClient -> Prespawned (entity)
    // if this is set, the predicted entity has been pre-spawned on the client
    pub client_entity: Option<Entity>,
}

// NOTE: need to define this here because otherwise we get the error
// "impl doesn't use only types from inside the current crate"
// TODO: does this mean that we cannot use existing types such as Transform?
//  might need a list of pre-existing types?
impl SyncComponent for ShouldBePredicted {
    fn mode() -> ComponentSyncMode {
        ComponentSyncMode::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_target() {
        let mut target = NetworkTarget::All;
        assert!(target.should_send_to(&0));
        target.exclude(vec![1, 2]);
        assert_eq!(target, NetworkTarget::AllExcept(vec![1, 2]));

        target = NetworkTarget::AllExcept(vec![0]);
        assert!(!target.should_send_to(&0));
        assert!(target.should_send_to(&1));
        target.exclude(vec![0, 1]);
        assert!(matches!(target, NetworkTarget::AllExcept(_)));

        if let NetworkTarget::AllExcept(ids) = target {
            assert!(ids.contains(&0));
            assert!(ids.contains(&1));
        }

        target = NetworkTarget::Only(vec![0]);
        assert!(target.should_send_to(&0));
        assert!(!target.should_send_to(&1));
        target.exclude(vec![1]);
        assert_eq!(target, NetworkTarget::Only(vec![0]));
        target.exclude(vec![0, 2]);
        assert_eq!(target, NetworkTarget::None);

        target = NetworkTarget::None;
        assert!(!target.should_send_to(&0));
        target.exclude(vec![1]);
        assert_eq!(target, NetworkTarget::None);
    }
}
