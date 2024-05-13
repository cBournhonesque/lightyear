//! Components used for replication
use bevy::ecs::entity::MapEntities;
use bevy::ecs::query::QueryFilter;
use bevy::ecs::system::SystemParam;
use bevy::prelude::{Bundle, Component, Entity, EntityMapper, Or, Query, Reflect, With};
use bevy::utils::{HashMap, HashSet};
use serde::{Deserialize, Serialize};
use tracing::trace;

use bitcode::{Decode, Encode};

use crate::channel::builder::Channel;
use crate::client::components::SyncComponent;
use crate::connection::id::ClientId;
use crate::prelude::ParentSync;
use crate::protocol::component::{ComponentKind, ComponentNetId, ComponentRegistry};
use crate::server::visibility::immediate::{ClientVisibility, VisibilityManager};
use crate::shared::replication::network_target::NetworkTarget;

/// Marker component that indicates that the entity was spawned via replication
/// (it is being replicated from a remote world)
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[component(storage = "SparseSet")]
pub struct Replicated;

/// Component inserted to each replicable entities, to detect when they are despawned
#[derive(Component, Clone, Copy)]
#[component(storage = "SparseSet")]
pub(crate) struct DespawnTracker;

/// Marker component to indicate that the entity is under the control of the local peer
#[derive(Component, Clone, Copy, PartialEq, Debug, Reflect, Serialize, Deserialize)]
#[component(storage = "SparseSet")]
pub struct Controlled;

/// Component that indicates that an entity should be replicated. Added to the entity when it is spawned
/// in the world that sends replication updates.
#[derive(Bundle, Clone, PartialEq, Debug, Reflect)]
pub struct Replicate {
    /// Which clients should this entity be replicated to
    pub replication_target: ReplicationTarget,
    /// Which client(s) control this entity?
    pub controlled_by: ControlledBy,
    /// How do we control the visibility of the entity?
    pub visibility: VisibilityMode,
    /// The replication group defines how entities are grouped (sent as a single message) for replication.
    ///
    /// After the entity is first replicated, the replication group of the entity should not be modified.
    /// (but more entities can be added to the replication group)
    // TODO: currently, if the host removes Replicate, then the entity is not removed in the remote
    //  it just keeps living but doesn't receive any updates. Should we make this configurable?
    pub group: ReplicationGroup,
    /// How should the hierarchy of the entity (parents/children) be replicated?
    pub hierarchy: ReplicateHierarchy,
}

#[derive(SystemParam)]
struct ReplicateSystemParam<'w, 's> {
    query: Query<
        'w,
        's,
        (
            &'static ReplicationTarget,
            &'static ControlledBy,
            &'static VisibilityMode,
            &'static ReplicationGroup,
            &'static ReplicateHierarchy,
            &'static TargetEntity,
        ),
    >,
}

/// Component that indicates which clients the entity should be replicated to.
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
pub struct ReplicationTarget {
    /// Which clients should this entity be replicated to
    pub replication: NetworkTarget,
    /// Which clients should predict this entity (unused for client to server replication)
    pub prediction: NetworkTarget,
    /// Which clients should interpolate this entity (unused for client to server replication)
    pub interpolation: NetworkTarget,
}

impl Default for ReplicationTarget {
    fn default() -> Self {
        Self {
            replication: NetworkTarget::All,
            prediction: NetworkTarget::None,
            interpolation: NetworkTarget::None,
        }
    }
}

/// Component storing metadata about which clients have control over the entity
///
/// This is only used for server to client replication.
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect)]
pub struct ControlledBy {
    /// Which client(s) control this entity?
    pub target: NetworkTarget,
}

impl ControlledBy {
    /// Returns true if the entity is controlled by the specified client
    pub fn targets(&self, client_id: &ClientId) -> bool {
        self.target.targets(client_id)
    }
}

/// Component to have more fine-grained control over the visibility of an entity
/// (which clients do we replicate this entity to?)
///
/// This has no effect for client to server replication.
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
pub struct Visibility {
    /// Control if we do fine-grained or coarse-grained visibility
    mode: VisibilityMode,
    // TODO: should we store the visibility cache here if visibility_mode = InterestManagement?
}

/// Defines the target entity for the replication.
///
/// This can be used if you want to replicate this entity on an entity that already
/// exists in the remote world.
///
/// This component is not part of the `Replicate` bundle as this is very infrequent.
#[derive(Component, Default, Clone, Copy, Debug, PartialEq, Reflect)]
pub enum TargetEntity {
    /// Spawn a new entity on the remote peer
    #[default]
    Spawn,
    /// Instead of spawning a new entity, we will apply the replication updates
    /// to the existing remote entity
    Preexisting(Entity),
}

/// Component that defines how the hierarchy of an entity (parent/children) should be replicated
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect)]
pub struct ReplicateHierarchy {
    /// If true, recursively add `Replicate` and `ParentSync` components to all children to make sure they are replicated
    /// If false, you can still replicate hierarchies, but in a more fine-grained manner. You will have to add the `Replicate`
    /// and `ParentSync` components to the children yourself
    pub recursive: bool,
}

impl Default for ReplicateHierarchy {
    fn default() -> Self {
        Self { recursive: true }
    }
}

// TODO: should these be sparse set or not?
/// If this component is present, we won't replicate the component
///
/// (By default, all components that are present in the [`ComponentRegistry`] will be replicated.)
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect)]
#[component(storage = "SparseSet")]
pub struct DisabledComponent<C> {
    _marker: std::marker::PhantomData<C>,
}

impl<C> Default for DisabledComponent<C> {
    fn default() -> Self {
        Self {
            _marker: Default::default(),
        }
    }
}

/// If this component is present, we will replicate only the inserts/removals of the component,
/// not the updates (i.e. the component will get only replicated once at entity spawn)
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect)]
#[component(storage = "SparseSet")]
pub struct ReplicateOnceComponent<C> {
    _marker: std::marker::PhantomData<C>,
}

impl<C> Default for ReplicateOnceComponent<C> {
    fn default() -> Self {
        Self {
            _marker: Default::default(),
        }
    }
}

// TODO: maybe have 3 fields:
//  - target
//  - override replication_target: bool (if true, we will completely override the replication target. If false, we do the intersection)
//  - override visibility: bool (if true, we will completely override the visibility. If false, we do the intersection)
/// This component lets you override the replication target for a specific component
#[derive(Component, Clone, Debug, PartialEq, Reflect)]
pub struct OverrideTargetComponent<C> {
    pub target: NetworkTarget,
    _marker: std::marker::PhantomData<C>,
}

impl<C> OverrideTargetComponent<C> {
    pub fn new(target: NetworkTarget) -> Self {
        Self {
            target,
            _marker: Default::default(),
        }
    }
}

impl Replicate {
    pub(crate) fn group_id(&self, entity: Option<Entity>) -> ReplicationGroupId {
        self.group.group_id(entity)
    }

    /// Returns true if the entity is controlled by the specified client
    pub fn is_controlled_by(&self, client_id: &ClientId) -> bool {
        self.controlled_by.targets(client_id)
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
#[derive(Component, Debug, Copy, Clone, PartialEq, Reflect)]
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

#[derive(
    Default,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    Reflect,
    Encode,
    Decode,
)]
pub struct ReplicationGroupId(pub u64);

#[derive(Component, Clone, Copy, Default, Debug, PartialEq, Reflect)]
pub enum VisibilityMode {
    /// We will replicate this entity to the clients specified in the `replication_target`.
    /// On top of that, we will apply interest management logic to determine which clients should receive the entity
    ///
    /// You can use [`gain_visibility`](VisibilityManager::gain_visibility) and [`lose_visibility`](VisibilityManager::lose_visibility)
    /// to control the visibility of entities.
    /// You can also use the [`RoomManager`](crate::prelude::server::RoomManager)
    ///
    /// (the client still needs to be included in the [`NetworkTarget`], the room is simply an additional constraint)
    InterestManagement,
    /// We will replicate this entity to the client specified in the `replication_target`, without
    /// running any additional interest management logic
    #[default]
    All,
}

impl Default for Replicate {
    fn default() -> Self {
        Self {
            replication_target: ReplicationTarget::default(),
            controlled_by: ControlledBy::default(),
            visibility: VisibilityMode::default(),
            group: ReplicationGroup::default(),
            hierarchy: ReplicateHierarchy::default(),
        }
    }
}

/// Marker component that tells the client to spawn an Interpolated entity
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Reflect)]
#[component(storage = "SparseSet")]
pub struct ShouldBeInterpolated;

/// Indicates that an entity was pre-predicted
// NOTE: we do not map entities for this component, we want to receive the entities as is
//  because we already do the mapping at other steps
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Reflect)]
#[component(storage = "SparseSet")]
pub struct PrePredicted {
    // if this is set, the predicted entity has been pre-spawned on the client
    pub(crate) client_entity: Option<Entity>,
}

/// Marker component that tells the client to spawn a Predicted entity
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq, Reflect)]
#[component(storage = "SparseSet")]
pub struct ShouldBePredicted;
