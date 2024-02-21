//! Components used for replication
use bevy::prelude::{Component, Entity};
use bevy::utils::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use tracing::trace;

use lightyear_macros::MessageInternal;

use crate::_reexport::FromType;
use crate::channel::builder::Channel;
use crate::client::components::SyncComponent;
use crate::connection::netcode::ClientId;
use crate::protocol::Protocol;
use crate::server::room::ClientVisibility;

/// Component inserted to each replicable entities, to detect when they are despawned
#[derive(Component, Clone, Copy)]
pub struct DespawnTracker;

/// Component that indicates that an entity should be replicated. Added to the entity when it is spawned
/// in the world that sends replication updates.
#[derive(Component, Clone, PartialEq, Debug)]
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
    /// List of clients that the entity is currently replicated to.
    /// Will be updated before the other replication systems
    #[doc(hidden)]
    pub replication_clients_cache: HashMap<ClientId, ClientVisibility>,
    pub replication_mode: ReplicationMode,
    // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
    //  it just keeps living but doesn't receive any updates. Should we make this configurable?
    pub replication_group: ReplicationGroup,
    /// If true, recursively add `Replicate` and `ParentSync` components to all children to make sure they are replicated
    /// If false, you can still replicate hierarchies, but in a more fine-grained manner. You will have to add the `Replicate`
    /// and `ParentSync` components to the children yourself
    pub replicate_hierarchy: bool,

    /// Lets you override the replication modalities for a specific component
    pub per_component_metadata: HashMap<P::ComponentKinds, PerComponentReplicationMetadata>,
}

/// This lets you specify how to customize the replication behaviour for a given component
#[derive(Clone, Debug, PartialEq)]
pub struct PerComponentReplicationMetadata {
    /// If true, do not replicate the component. (By default, all components of this entity that are present in the
    /// ComponentProtocol) will be replicated.
    disabled: bool,
    /// If true, replicate only inserts/removals of the component, not the updates.
    /// (i.e. the component will only get replicated once at spawn)
    /// This is useful for components such as `ActionState`, which should only be replicated once
    replicate_once: bool,
    /// Custom replication target for this component. We will replicate to the intersection of
    /// the entity's replication target and this target
    target: NetworkTarget,
}
impl Default for PerComponentReplicationMetadata {
    fn default() -> Self {
        Self {
            disabled: false,
            replicate_once: false,
            target: NetworkTarget::All,
        }
    }
}

impl<P: Protocol> Replicate<P> {
    pub(crate) fn group_id(&self, entity: Option<Entity>) -> ReplicationGroupId {
        self.replication_group.group_id(entity)
    }

    /// Returns true if we don't want to replicate the component
    pub fn is_disabled<C>(&self) -> bool
    where
        P::ComponentKinds: FromType<C>,
    {
        let kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.per_component_metadata
            .get(&kind)
            .is_some_and(|metadata| metadata.disabled)
    }

    /// If true, the component will be replicated only once, when the entity is spawned.
    /// We do not replicate component updates
    pub fn is_replicate_once<C>(&self) -> bool
    where
        P::ComponentKinds: FromType<C>,
    {
        let kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.per_component_metadata
            .get(&kind)
            .is_some_and(|metadata| metadata.replicate_once)
    }

    /// Replication target for this specific component
    /// This will be the intersection of the provided `entity_target`, and the `target` of the component
    /// if it exists
    pub fn target<C>(&self, mut entity_target: NetworkTarget) -> NetworkTarget
    where
        P::ComponentKinds: FromType<C>,
    {
        let kind = <P::ComponentKinds as FromType<C>>::from_type();
        match self.per_component_metadata.get(&kind) {
            None => entity_target,
            Some(metadata) => {
                entity_target.intersection(metadata.target.clone());
                trace!(?kind, "final target: {:?}", entity_target);
                entity_target
            }
        }
    }

    /// Disable the replication of a component for this entity
    pub fn disable_component<C>(&mut self)
    where
        P::ComponentKinds: FromType<C>,
    {
        let kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.per_component_metadata
            .entry(kind)
            .or_default()
            .disabled = true;
    }

    /// Enable the replication of a component for this entity
    pub fn enable_component<C>(&mut self)
    where
        P::ComponentKinds: FromType<C>,
    {
        let kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.per_component_metadata
            .entry(kind)
            .or_default()
            .disabled = false;
        // if we are back at the default, remove the entry
        if self.per_component_metadata.get(&kind).unwrap()
            == &PerComponentReplicationMetadata::default()
        {
            self.per_component_metadata.remove(&kind);
        }
    }

    pub fn enable_replicate_once<C>(&mut self)
    where
        P::ComponentKinds: FromType<C>,
    {
        let kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.per_component_metadata
            .entry(kind)
            .or_default()
            .replicate_once = true;
    }

    pub fn disable_replicate_once<C>(&mut self)
    where
        P::ComponentKinds: FromType<C>,
    {
        let kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.per_component_metadata
            .entry(kind)
            .or_default()
            .replicate_once = false;
        // if we are back at the default, remove the entry
        if self.per_component_metadata.get(&kind).unwrap()
            == &PerComponentReplicationMetadata::default()
        {
            self.per_component_metadata.remove(&kind);
        }
    }

    pub fn add_target<C>(&mut self, target: NetworkTarget)
    where
        P::ComponentKinds: FromType<C>,
    {
        let kind = <P::ComponentKinds as FromType<C>>::from_type();
        self.per_component_metadata.entry(kind).or_default().target = target;
        // if we are back at the default, remove the entry
        if self.per_component_metadata.get(&kind).unwrap()
            == &PerComponentReplicationMetadata::default()
        {
            self.per_component_metadata.remove(&kind);
        }
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq)]
pub enum ReplicationGroupIdBuilder {
    // the group id is the entity id
    #[default]
    FromEntity,
    // choose a different group id
    // note: it must not be the same as any entity id!
    // TODO: how can i generate one that doesn't conflict with an existing entity? maybe take u32 as input, and apply generation = u32::MAX - 1?
    //  or reserver some entities on the sender world?
    Group(u64),
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ReplicationGroup {
    id_builder: ReplicationGroupIdBuilder,
    /// the priority of the accumulation group
    /// (priority will get reset to this value every time a message gets sent successfully)
    base_priority: f32,
}

impl Default for ReplicationGroup {
    fn default() -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::FromEntity,
            base_priority: 1.0,
        }
    }
}

impl ReplicationGroup {
    pub const fn new_from_entity() -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::FromEntity,
            base_priority: 1.0,
        }
    }

    pub const fn new_id(id: u64) -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::Group(id),
            base_priority: 1.0,
        }
    }

    pub(crate) fn group_id(&self, entity: Option<Entity>) -> ReplicationGroupId {
        match self.id_builder {
            ReplicationGroupIdBuilder::FromEntity => {
                ReplicationGroupId(entity.expect("need to provide an entity").to_bits())
            }
            ReplicationGroupIdBuilder::Group(id) => ReplicationGroupId(id),
        }
    }

    pub(crate) fn priority(&self) -> f32 {
        self.base_priority
    }

    pub fn set_priority(mut self, priority: f32) -> Self {
        self.base_priority = priority;
        self
    }

    pub fn set_id(mut self, id: u64) -> Self {
        self.id_builder = ReplicationGroupIdBuilder::Group(id);
        self
    }
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
        #[allow(unused_mut)]
        let mut replicate = Self {
            replication_target: NetworkTarget::All,
            prediction_target: NetworkTarget::None,
            interpolation_target: NetworkTarget::None,
            replication_clients_cache: HashMap::new(),
            replication_mode: ReplicationMode::default(),
            replication_group: Default::default(),
            replicate_hierarchy: true,
            per_component_metadata: HashMap::default(),
        };
        // those metadata components should only be replicated once
        replicate.enable_replicate_once::<ShouldBePredicted>();
        replicate.enable_replicate_once::<ShouldBeInterpolated>();
        // cfg_if! {
        //     // the ActionState components are replicated only once when the entity is spawned
        //     // then they get updated by the user inputs, not by replication!
        //     if #[cfg(feature = "leafwing")] {
        //         use leafwing_input_manager::prelude::ActionState;
        //         replicate.enable_replicate_once::<ActionState<P::LeafwingInput1>>();
        //         replicate.enable_replicate_once::<ActionState<P::LeafwingInput2>>();
        //     }
        // }
        replicate
    }
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
/// NetworkTarget indicated which clients should receive some message
pub enum NetworkTarget {
    #[default]
    /// Message sent to no client
    None,
    /// Message sent to all clients except one
    AllExceptSingle(ClientId),
    /// Message sent to all clients except for these
    AllExcept(Vec<ClientId>),
    /// Message sent to all clients
    All,
    /// Message sent to only these
    Only(Vec<ClientId>),
    /// Message sent to only this one client
    Single(ClientId),
}

impl NetworkTarget {
    /// Return true if we should replicate to the specified client
    pub(crate) fn should_send_to(&self, client_id: &ClientId) -> bool {
        match self {
            NetworkTarget::All => true,
            NetworkTarget::AllExceptSingle(single) => client_id != single,
            NetworkTarget::AllExcept(client_ids) => !client_ids.contains(client_id),
            NetworkTarget::Only(client_ids) => client_ids.contains(client_id),
            NetworkTarget::Single(single) => client_id == single,
            NetworkTarget::None => false,
        }
    }

    /// Compute the intersection of this target with another one (A ∩ B)
    pub(crate) fn intersection(&mut self, target: NetworkTarget) {
        match self {
            NetworkTarget::All => {
                *self = target;
            }
            NetworkTarget::AllExceptSingle(existing_client_id) => {
                let mut a = NetworkTarget::AllExcept(vec![*existing_client_id]);
                a.intersection(target);
                *self = a;
            }
            NetworkTarget::AllExcept(existing_client_ids) => match target {
                NetworkTarget::None => {
                    *self = NetworkTarget::None;
                }
                NetworkTarget::AllExceptSingle(target_client_id) => {
                    let mut new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                    new_excluded_ids.insert(target_client_id);
                    *existing_client_ids = Vec::from_iter(new_excluded_ids);
                }
                NetworkTarget::AllExcept(target_client_ids) => {
                    let mut new_excluded_ids = HashSet::from_iter(existing_client_ids.clone());
                    target_client_ids.into_iter().for_each(|id| {
                        new_excluded_ids.insert(id);
                    });
                    *existing_client_ids = Vec::from_iter(new_excluded_ids);
                }
                NetworkTarget::All => {}
                NetworkTarget::Only(target_client_ids) => {
                    let mut new_included_ids = HashSet::from_iter(target_client_ids.clone());
                    existing_client_ids.iter_mut().for_each(|id| {
                        new_included_ids.remove(id);
                    });
                    *self = NetworkTarget::Only(Vec::from_iter(new_included_ids));
                }
                NetworkTarget::Single(target_client_id) => {
                    if existing_client_ids.contains(&target_client_id) {
                        *self = NetworkTarget::None;
                    } else {
                        *self = NetworkTarget::Single(target_client_id);
                    }
                }
            },
            NetworkTarget::Only(existing_client_ids) => match target {
                NetworkTarget::None => {
                    *self = NetworkTarget::None;
                }
                NetworkTarget::AllExceptSingle(target_client_id) => {
                    let mut new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    new_included_ids.remove(&target_client_id);
                    *existing_client_ids = Vec::from_iter(new_included_ids);
                }
                NetworkTarget::AllExcept(target_client_ids) => {
                    let mut new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    target_client_ids.into_iter().for_each(|id| {
                        new_included_ids.remove(&id);
                    });
                    *existing_client_ids = Vec::from_iter(new_included_ids);
                }
                NetworkTarget::All => {}
                NetworkTarget::Single(target_client_id) => {
                    if existing_client_ids.contains(&target_client_id) {
                        *self = NetworkTarget::Single(target_client_id);
                    } else {
                        *self = NetworkTarget::None;
                    }
                }
                NetworkTarget::Only(target_client_ids) => {
                    let new_included_ids = HashSet::from_iter(existing_client_ids.clone());
                    let target_included_ids = HashSet::from_iter(target_client_ids.clone());
                    let intersection = new_included_ids.intersection(&target_included_ids).cloned();
                    *existing_client_ids = intersection.collect::<Vec<_>>();
                }
            },
            NetworkTarget::Single(existing_client_id) => {
                let mut a = NetworkTarget::Only(vec![*existing_client_id]);
                a.intersection(target);
                *self = a;
            }
            NetworkTarget::None => {}
        }
    }

    /// Compute the difference of this target with another one (A - B)
    pub(crate) fn exclude(&mut self, client_ids: Vec<ClientId>) {
        match self {
            NetworkTarget::All => {
                *self = NetworkTarget::AllExcept(client_ids);
            }
            NetworkTarget::AllExceptSingle(existing_client_id) => {
                let mut new_excluded_ids = HashSet::from_iter(client_ids.clone());
                new_excluded_ids.insert(*existing_client_id);
                *self = NetworkTarget::AllExcept(Vec::from_iter(new_excluded_ids));
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
            NetworkTarget::Single(client_id) => {
                if client_ids.contains(client_id) {
                    *self = NetworkTarget::None;
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

// TODO: Right now we use the approach that we add an extra component to the Protocol of components to be replicated.
//  that's pretty dangerous because it's now hard for the user to derive new traits.
//  let's think of another approach later.
// NOTE: we do not map entities for this component, we want to receive the entities as is

/// Indicates that an entity was pre-predicted
#[derive(Component)]
pub struct PrePredicted;
#[derive(Component, MessageInternal, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ShouldBePredicted {
    // TODO: rename this?
    //  - also the server already gets the client entity in the message, so it's a waste of space...
    //  - maybe use a different component: ClientToServer -> Prespawned (None)
    //  - ServerToClient -> Prespawned (entity)
    // if this is set, the predicted entity has been pre-spawned on the client
    pub(crate) client_entity: Option<Entity>,
    // this is set by the server to know which client did the pre-prediction
    pub(crate) client_id: Option<ClientId>,
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

    #[test]
    fn test_intersection() {
        let mut target = NetworkTarget::All;
        target.intersection(NetworkTarget::AllExcept(vec![1, 2]));
        assert_eq!(target, NetworkTarget::AllExcept(vec![1, 2]));

        target = NetworkTarget::AllExcept(vec![0]);
        target.intersection(NetworkTarget::AllExcept(vec![0, 1]));
        assert!(matches!(target, NetworkTarget::AllExcept(_)));

        if let NetworkTarget::AllExcept(ids) = target {
            assert!(ids.contains(&0));
            assert!(ids.contains(&1));
        }

        target = NetworkTarget::AllExcept(vec![0, 1]);
        target.intersection(NetworkTarget::Only(vec![0, 2]));
        assert_eq!(target, NetworkTarget::Only(vec![2]));

        target = NetworkTarget::Only(vec![0, 1]);
        target.intersection(NetworkTarget::Only(vec![0, 2]));
        assert_eq!(target, NetworkTarget::Only(vec![0]));

        target = NetworkTarget::Only(vec![0, 1]);
        target.intersection(NetworkTarget::AllExcept(vec![0, 2]));
        assert_eq!(target, NetworkTarget::Only(vec![1]));

        target = NetworkTarget::None;
        target.intersection(NetworkTarget::AllExcept(vec![0, 2]));
        assert_eq!(target, NetworkTarget::None);
    }
}
