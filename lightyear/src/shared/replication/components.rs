//! Components used for replication

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::{Component, Entity, Reflect};
use bevy::time::{Timer, TimerMode};
use serde::{Deserialize, Serialize};

use crate::connection::id::ClientId;
use crate::protocol::component::ComponentKind;
use crate::serialize::reader::{ReadInteger, Reader};
use crate::serialize::{SerializationError, ToBytes};
use crate::serialize::writer::WriteInteger;

/// Marker that indicates that this entity is to be replicated.
///
/// This is not confused with `Replicating` which is only present on the entity when the entity
/// is currently being replicated. (removing `Replicating` pauses replication updates).
///
/// `ReplicationMarker` is required by `ReplicateToServer` and `ReplicationTarget`
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
#[require(ReplicationGroup, Replicating)]
pub struct ReplicationMarker;

/// Marker component that indicates that the entity was initially spawned via replication
/// (it was being replicated from a remote world)
///
/// The component is added once and is then never modified anymore
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct InitialReplicated {
    /// The peer that originally spawned the entity
    /// If None, it's the server.
    pub from: Option<ClientId>,
}

impl InitialReplicated {
    /// For client->server replication, identify the client that replicated this entity to the server
    pub fn client_id(&self) -> ClientId {
        self.from.expect("expected a client id")
    }
}

/// Marker component that indicates that the entity is being replicated
/// from a remote world.
///
/// The component only exists while the peer does not have authority over
/// the entity.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct Replicated {
    /// The peer that is actively replicating the entity
    /// If None, it's the server.
    pub from: Option<ClientId>,
}

impl Replicated {
    /// For client->server replication, identify the client that replicated this entity to the server
    pub fn client_id(&self) -> ClientId {
        self.from.expect("expected a client id")
    }
}

/// Marker component to indicate that the entity is under the control of the local peer
#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Controlled;

/// Marker component to indicate that updates for this entity are being replicated.
///
/// If this component gets removed, the replication will pause.
#[derive(Component, Clone, Copy, Default, PartialEq, Debug, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Replicating;

/// Keeps track of the last known state of a component, so that we can compute
/// the delta between the old and new state.
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct Cached<C> {
    pub value: C,
}

/// Defines the target entity for the replication.
///
/// This can be used if you want to replicate this entity on an entity that already
/// exists in the remote world.
///
/// This component is not part of the `Replicate` bundle as this is very infrequent.
#[derive(Component, Default, Clone, Copy, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub enum TargetEntity {
    /// Spawn a new entity on the remote peer
    #[default]
    Spawn,
    /// Instead of spawning a new entity, we will apply the replication updates
    /// to the existing remote entity
    Preexisting(Entity),
}

/// Marker component that defines how the hierarchy of an entity (parent/children) should be replicated.
///
/// When `DisableReplicateHierarchy` is added to an entity, we will stop replicating their children.
///
/// If the component is added on an entity with `Replicate`, it's children will be replicated using
/// the same replication settings as the Parent.
/// This is achieved via the marker component `ReplicateLikeParent` added on each child.
/// You can remove the `ReplicateLikeParent` component to disable this on a child entity. You can then
/// add the replication components on the child to replicate it independently from the parents.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct DisableReplicateHierarchy;

// - each entity has a ReplicateLike(entity)
//   - the entity can be itself or another one (usually a parent)
// - when we spawn Replicate, etc., we insert a ReplicateLike(itself)
//   - maybe this is not needed, and in the replication system we have 2 steps; one for ReplicateLike and one for entities that have Replication?
// - in the replication systems, we iterate through all the ReplicateLike(entity), fetch the components on the 'like' entity, and then go
//   - if the entity itself has a replication component, we will use that instead of the one from ReplicateLike
// - if we add DisableReplicateHierarchy, we remove ReplicateLike from all children (but not from itself)
// - when we remove ReplicateLike, we remove ReplicateLike recursively in all children as well

// TODO: do we need this? or do we just check if delta compression fn is present in the registry?
/// If this component is present, the component will be replicated via delta-compression.
///
/// Instead of sending the full component every time, we will only send the diffs between the old
/// and new state.
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct DeltaCompression {
    // we use a Vec instead of a HashSet to go faster, I doubt there would be many cases
    // where we have duplicate kinds here
    kinds: Vec<ComponentKind>,
}

impl DeltaCompression {
    pub fn add<C: Component>(mut self) -> Self {
        self.kinds.push(ComponentKind::of::<C>());
        self
    }

    pub fn remove<C: Component>(mut self) -> Self {
        self.kinds.retain(|kind| *kind != ComponentKind::of::<C>());
        self
    }

    pub fn enabled<C: Component>(&self) -> bool {
        self.enabled_kind(ComponentKind::of::<C>())
    }

    pub(crate) fn enabled_kind(&self, kind: ComponentKind) -> bool {
        self.kinds.contains(&kind)
    }
}

// TODO: maybe this can be merged with OverrideTargetComponent?
//  we could think that Target=None means Disabled?
/// If this component is present, we won't replicate the component
///
/// (By default, all components that are present in the [`ComponentRegistry`](crate::prelude::ComponentRegistry) will be replicated.)
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct DisabledComponents {
    /// If `disable_all` is true, all components are disabled for replication. Only the components in `enabled` will be replicated
    enabled: Vec<ComponentKind>,
    /// if True, all components are disabled for replication. Only the components in `enabled` will be replicated
    disable_all: bool,
    /// If `disable_all` is false, all components are enabled for replication. Only the components in `disabled` will not be replicated
    disabled: Vec<ComponentKind>,
}

impl DisabledComponents {
    /// Returns true if a component is enabled for replication
    pub(crate) fn enabled_kind(&self, kind: ComponentKind) -> bool {
        if self.disable_all {
            return self.enabled.contains(&kind);
        }
        !self.disabled.contains(&kind)
    }

    /// Returns true if a component is enabled for replication
    pub(crate) fn enabled<C: Component>(&self) -> bool {
        self.enabled_kind(ComponentKind::of::<C>())
    }

    /// Disables the replication of a component
    pub fn disable<C: Component>(mut self) -> Self {
        self.enabled.retain(|c| c != &ComponentKind::of::<C>());
        self.disabled.push(ComponentKind::of::<C>());
        self
    }

    /// Enables the replication of a component
    pub fn enable<C: Component>(mut self) -> Self {
        self.disabled.retain(|c| c != &ComponentKind::of::<C>());
        self.enabled.push(ComponentKind::of::<C>());
        self
    }

    /// Disables all components for replication. Only the components that are explicitly enabled will be replicated
    pub fn disable_all(mut self) -> Self {
        self.disable_all = true;
        self.disabled.clear();
        self.enabled.clear();
        self
    }

    /// Enables all components for replication. Only the components that are explicitly disabled will not be replicated
    pub fn enable_all(mut self) -> Self {
        self.disable_all = false;
        self.enabled.clear();
        self.disabled.clear();
        self
    }
}

/// Component that can be used to specify which components we will only send inserts/removals
/// but not component updates. The component will only get replicated once at entity spawn.
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ReplicateOnce {
    // we use a Vec instead of a HashSet to go faster, I doubt there would be many cases
    // where we have duplicate kinds here
    kinds: Vec<ComponentKind>,
}

impl ReplicateOnce {
    pub fn add<C: Component>(mut self) -> Self {
        self.add_mut::<C>();
        self
    }

    pub fn add_mut<C: Component>(&mut self) {
        self.kinds.push(ComponentKind::of::<C>());
    }

    pub fn remove<C: Component>(mut self) -> Self {
        self.remove_mut::<C>();
        self
    }

    pub fn remove_mut<C: Component>(&mut self) {
        self.kinds.retain(|kind| *kind != ComponentKind::of::<C>());
    }

    pub fn enabled<C: Component>(&self) -> bool {
        self.enabled_kind(ComponentKind::of::<C>())
    }

    pub(crate) fn enabled_kind(&self, kind: ComponentKind) -> bool {
        self.kinds.contains(&kind)
    }
}

#[derive(Debug, Default, Copy, Clone, PartialEq, Reflect)]
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

/// Component to specify the replication group of an entity
///
/// If multiple entities are part of the same replication group, they will be sent together in the same message.
/// It is guaranteed that these entities will be updated at the same time on the remote world.
#[derive(Component, Debug, Clone, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ReplicationGroup {
    id_builder: ReplicationGroupIdBuilder,
    /// the priority of the accumulation group
    /// (priority will get reset to this value every time a message gets sent successfully)
    base_priority: f32,
    /// Keep track of whether we should send replication updates for this group.
    ///
    /// See [`ReplicationGroup::set_send_frequency`] for more information.
    pub(crate) send_frequency: Option<Timer>,
    /// Is true if we should send replication updates for this group.
    ///
    /// The interaction with `send_frequency` is as follows:
    /// Time:               0    10   20    30    40    50    60    70    80    90    100
    /// GroupTimer(30ms):   X               X                 X                 X
    /// SendInterval(20ms): X          X          X           X           X           X
    ///
    /// At 40ms, 60ms and 100ms, we will buffer the replication updates for the group.
    /// (We do not buffer the updates exactly at 30ms, 60ms, 90ms; instead we wait for the next send_interval.
    /// This is to avoid having to track the send_tick for each replication group separately)
    // TODO: maybe buffer the updates exactly at 30ms, 60ms, 90ms and include the send_tick in the message?
    pub(crate) should_send: bool,
}

impl Default for ReplicationGroup {
    fn default() -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::FromEntity,
            base_priority: 1.0,
            send_frequency: None,
            should_send: true,
        }
    }
}

impl ReplicationGroup {
    pub const fn new_from_entity() -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::FromEntity,
            base_priority: 1.0,
            send_frequency: None,
            should_send: true,
        }
    }

    pub const fn new_id(id: u64) -> Self {
        Self {
            id_builder: ReplicationGroupIdBuilder::Group(id),
            base_priority: 1.0,
            send_frequency: None,
            should_send: true,
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

    /// Sets the send frequency for this [`ReplicationGroup`]
    ///
    /// Any replication updates related to this group will only be buffered at the specified frequency.
    /// It is INCORRECT to set the send_frequency to be more frequent than the sender's send_interval.
    ///
    /// This can be useful to send updates for a group of entities less frequently than the default send_interval.
    /// For example the send_interval could be 30Hz, but you could set the send_frequency to 10Hz for a group of entities
    /// to buffer updates less frequently.
    pub fn set_send_frequency(mut self, send_frequency: core::time::Duration) -> Self {
        self.send_frequency = Some(Timer::new(send_frequency, TimerMode::Repeating));
        self
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect)]
pub struct ReplicationGroupId(pub u64);


// Re-use the Entity serialization since ReplicationGroupId are often entities
impl ToBytes for ReplicationGroupId {
    fn bytes_len(&self) -> usize {
        8
        // TODO: if it's a valid entity (generation > 0 and high-bit is 0)
        //  optimize by serializing as an entity!
        // Entity::try_from_bits(self.0).map_or_else(|_| 8, |entity| entity.bytes_len())
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        // Entity::try_from_bits(self.0).map_or_else(|_| {)
        //     buffer.write_u64(self.0)?;
        //     Ok(())
        // }, |entity| {
        //     entity.to_bytes(buffer)
        // })?;
        buffer.write_u64(self.0)?;
        // Entity::to_bytes(&Entity::from_bits(self.0), buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        Ok(Self(buffer.read_u64()?))
        // let entity = Entity::from_bytes(buffer)?;
        // Ok(Self(entity.to_bits()))
    }
}

// NOTE: we don't add a #[require(ReplicateToClient)] attribute here
//  so that it's possible to override the NetworkRelevanceMode for a ReplicateLike entity
#[derive(Component, Clone, Copy, Default, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub enum NetworkRelevanceMode {
    /// We will replicate this entity to the clients specified in the `replication_target`.
    /// On top of that, we will apply interest management logic to determine which clients should receive the entity
    ///
    /// You can use [`gain_relevance`](crate::prelude::server::RelevanceManager::gain_relevance) and [`lose_relevance`](crate::prelude::server::RelevanceManager::lose_relevance)
    /// to control the network relevance of entities.
    ///
    /// You can also use the [`RoomManager`](crate::prelude::server::RoomManager) if you want to use rooms to control network relevance.
    ///
    /// (the client still needs to be included in the [`NetworkTarget`], the room is simply an additional constraint)
    InterestManagement,
    /// We will replicate this entity to the client specified in the `replication_target`, without
    /// running any additional interest management logic
    #[default]
    All,
}

/// Marker component that tells the client to spawn an Interpolated entity
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ShouldBeInterpolated;

/// Indicates that an entity was pre-predicted
// NOTE: we do not map entities for this component, we want to receive the entities as is
//  because we already do the mapping at other steps
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct PrePredicted {
    pub(crate) confirmed_entity: Option<Entity>,
}

/// Marker component that tells the client to spawn a Predicted entity
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct ShouldBePredicted;
