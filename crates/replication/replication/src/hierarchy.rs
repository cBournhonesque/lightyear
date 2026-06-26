//! This module is responsible for making sure that parent-children hierarchies are replicated correctly.
use crate::ReplicationSystems;
use crate::prelude::Replicate;
#[cfg(feature = "interpolation")]
use crate::send::InterpolationTarget;
#[cfg(feature = "prediction")]
use crate::send::PredictionTarget;
use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_ecs::component::Immutable;
use bevy_ecs::entity::MapEntities;
use bevy_ecs::prelude::*;
use bevy_ecs::query::QueryData;
use bevy_ecs::reflect::ReflectMapEntities;
use bevy_ecs::relationship::Relationship;
use bevy_reflect::Reflect;
use bevy_replicon::bytes::Bytes;
use bevy_replicon::postcard_utils;
use bevy_replicon::prelude::{RuleFns, SyncRelatedAppExt};
use bevy_replicon::shared::replication::deferred_entity::DeferredEntity;
use bevy_replicon::shared::replication::registry::ctx::{RemoveCtx, SerializeCtx, WriteCtx};
#[cfg(feature = "client")]
use bevy_replicon::shared::server_entity_map::ServerEntityMap;
use core::fmt::Debug;
use serde::Serialize;
use serde::de::DeserializeOwned;
use smallvec::SmallVec;
use tracing::trace;

#[deprecated(note = "Use RelationshipSystems instead")]
pub type RelationshipSet = RelationshipSystems;
#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RelationshipSystems {
    // PreUpdate
    Receive,
    // PostUpdate
    Send,
}

pub(crate) struct HierarchyPlugin;

/// Client-side placeholder for a replicated [`ChildOf`] whose parent entity
/// has not been mapped yet.
///
/// Replicon's default entity mapping creates a buffered placeholder when a
/// replicated component references an entity that has not appeared in the
/// server-to-client map yet. That is unsafe for relationship components like
/// [`ChildOf`], because inserting the relationship immediately runs Bevy's
/// relationship hooks and leaves Replicon's placeholder buffer alive while the
/// next component in the same entity bundle is decoded.
#[derive(Component)]
pub(crate) struct PendingChildOf {
    server_parent: Entity,
}

impl PendingChildOf {
    fn new(server_parent: Entity) -> Self {
        Self { server_parent }
    }
}

#[derive(QueryData)]
struct PropagationQuery {
    replicate: &'static Replicate,
    #[cfg(feature = "prediction")]
    prediction: Option<&'static PredictionTarget>,
    #[cfg(feature = "interpolation")]
    interpolation: Option<&'static InterpolationTarget>,
}

#[derive(QueryData)]
struct ChildPropagationQuery {
    replicate_like: &'static ReplicateLike,
    replicate: Option<&'static Replicate>,
    #[cfg(feature = "prediction")]
    prediction: Option<&'static PredictionTarget>,
    #[cfg(feature = "interpolation")]
    interpolation: Option<&'static InterpolationTarget>,
}

impl HierarchyPlugin {
    fn propagate_when_replicate_like_added(
        trigger: On<Insert, ReplicateLike>,
        child_query: Query<ChildPropagationQuery>,
        root_query: Query<PropagationQuery>,
        mut commands: Commands,
    ) {
        if let Ok(child) = child_query.get(trigger.entity)
            && let Ok(root_propagation) = root_query.get(child.replicate_like.root)
        {
            if child.replicate.is_none() {
                commands
                    .entity(trigger.entity)
                    .insert(root_propagation.replicate.clone());
            }
            #[cfg(feature = "prediction")]
            if child.prediction.is_none()
                && let Some(prediction) = root_propagation.prediction
            {
                commands.entity(trigger.entity).insert(prediction.clone());
            }
            #[cfg(feature = "interpolation")]
            if child.interpolation.is_none()
                && let Some(interpolation) = root_propagation.interpolation
            {
                commands
                    .entity(trigger.entity)
                    .insert(interpolation.clone());
            }
        }
    }
}

impl Plugin for HierarchyPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::propagate_when_replicate_like_added);
    }
}

/// Serializes the server parent entity targeted by [`ChildOf`].
///
/// Lightyear registers a custom rule for [`ChildOf`] so the receive path can
/// inspect the raw server entity before mapping it. If the parent is not mapped
/// yet, the receiver defers inserting the relationship instead of letting
/// Replicon create a placeholder entity inside the relationship component.
pub(crate) fn serialize_child_of(
    _ctx: &SerializeCtx,
    child_of: &ChildOf,
    message: &mut Vec<u8>,
) -> bevy_ecs::error::Result<()> {
    postcard_utils::entity_to_extend_mut(&child_of.parent(), message)?;
    Ok(())
}

/// Deserializes the raw server parent entity for stale-message consumption.
///
/// The active receive path uses [`write_child_of`] so it can defer insertion
/// until the parent is mapped.
pub(crate) fn deserialize_child_of(
    _ctx: &mut WriteCtx,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<ChildOf> {
    let server_parent = postcard_utils::entity_from_buf(message)?;
    Ok(ChildOf(server_parent))
}

/// Receives [`ChildOf`] without using Replicon's placeholder entity mapper.
///
/// If the parent has already been mapped, this inserts the real Bevy hierarchy
/// relationship. If not, it stores [`PendingChildOf`] and waits for
/// [`resolve_pending_child_of`] to attach the relationship once the parent
/// mapping is available.
pub(crate) fn write_child_of(
    ctx: &mut WriteCtx,
    _rule_fns: &RuleFns<ChildOf>,
    entity: &mut DeferredEntity,
    message: &mut Bytes,
) -> bevy_ecs::error::Result<()> {
    let server_parent = postcard_utils::entity_from_buf(message)?;
    if let Some(&client_parent) = ctx.entity_map.to_client().get(&server_parent) {
        entity.insert(ChildOf(client_parent));
        entity.remove::<PendingChildOf>();
    } else {
        entity.insert(PendingChildOf::new(server_parent));
        entity.remove::<ChildOf>();
    }
    Ok(())
}

pub(crate) fn remove_child_of(_ctx: &mut RemoveCtx, entity: &mut DeferredEntity) {
    entity.remove::<ChildOf>();
    entity.remove::<PendingChildOf>();
}

/// Attach delayed hierarchy relationships once Replicon has mapped the parent.
#[cfg(feature = "client")]
pub(crate) fn resolve_pending_child_of(
    entity_map: Option<Res<ServerEntityMap>>,
    pending: Query<(Entity, &PendingChildOf)>,
    mut commands: Commands,
) {
    let Some(entity_map) = entity_map else {
        return;
    };
    for (entity, pending) in &pending {
        let Some(&client_parent) = entity_map.to_client().get(&pending.server_parent) else {
            continue;
        };
        commands
            .entity(entity)
            .insert(ChildOf(client_parent))
            .remove::<PendingChildOf>();
    }
}

/// When the `DisableReplicateHierarchy` marker component is added to an entity, we will stop replicating their children.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct DisableReplicateHierarchy;

/// Marker component that indicates that this entity should be replicated similarly to the entity
/// contained in the component.
///
/// This will be inserted automatically on all children of an entity that has `Replicate`,
/// unless the parent has a [`DisableReplicateHierarchy`] component.
#[derive(Component, Clone, MapEntities, Copy, Reflect, PartialEq, Debug)]
#[relationship(relationship_target=ReplicateLikeChildren)]
#[reflect(Component, MapEntities, PartialEq, Debug)]
pub struct ReplicateLike {
    #[entities]
    pub root: Entity,
}

/// Relationship target component associated with [`ReplicateLike`]
#[derive(Component, Debug, Reflect)]
#[relationship_target(relationship=ReplicateLike, linked_spawn)]
#[reflect(Component)]
pub struct ReplicateLikeChildren(Vec<Entity>);

/// Plugin that helps lightyear propagate replication components through a relationship.
///
/// The main idea is this:
/// - when `Replicate` is added, we will add a `ReplicateLike` component to all children
///   - we skip any child that have `DisableReplicateHierarchy` and its descendants
///   - we also skip any child that has `Replicate` and its descendants, because those children
///     will want to be replicated according to that child's replication components
/// - in the replication send system, either an entity has `Replicate` and we use its replication
///   components to determine how we do the sync. Or it could have the `ReplicateLike(root)` component and
///   we will use the `root` entity's replication components to determine how the replication will happen.
///   Any replication component (`ComponentReplicationOverrides`, etc.) can be added on the child entity to override the
///   behaviour only for that child
/// - this is mainly useful for replicating visibility components through the hierarchy. Instead of having to
///   add all the child entities to a room, or propagating the `NetworkVisibility` through the hierarchy,
///   the child entity can just use the root's `NetworkVisibility` value
pub struct HierarchySendPlugin<R: Relationship> {
    marker: core::marker::PhantomData<R>,
}

impl<R: Relationship> Default for HierarchySendPlugin<R> {
    fn default() -> Self {
        Self {
            marker: core::marker::PhantomData,
        }
    }
}

impl<
    R: Relationship
        + Component<Mutability = Immutable>
        + PartialEq
        + Clone
        + Serialize
        + DeserializeOwned,
> Plugin for HierarchySendPlugin<R>
{
    fn build(&self, app: &mut App) {
        // Note: app.replicate::<R>() is called in SharedComponentRegistrationPlugin
        // so that FnsIds match between client and server.
        app.sync_related_entities::<R>();

        // propagate ReplicateLike
        app.add_observer(Self::propagate_replicate_like_replication_marker_removed);
        app.add_systems(
            PostUpdate,
            Self::propagate_through_hierarchy.before(ReplicationSystems::Send),
        );
    }
}

impl<R: Relationship> HierarchySendPlugin<R> {
    /// Propagate certain replication components through the hierarchy.
    /// - If new children are added, `Replicate` is added, we recursively
    ///   go through the descendants and add `ReplicateLike`, `ChildOfSync`, ... if the child does not have
    ///   `DisableReplicateHierarchy` or `Replicate` already
    /// - We run this as a system and not an observer because observers cannot handle Children updates very well
    ///   (if we trigger on ChildOf being added, there is no flush between the ChildOf Add hook and the observer
    ///   so the `&Children` query won't be updated (or the component will not exist on the parent yet)
    fn propagate_through_hierarchy(
        mut commands: Commands,
        root_query: Query<
            (Entity, Option<&ReplicateLike>),
            (
                Or<(With<Replicate>, With<ReplicateLike>)>,
                Without<DisableReplicateHierarchy>,
                With<R::RelationshipTarget>,
                Or<(Changed<R::RelationshipTarget>, Added<Replicate>)>,
            ),
        >,
        children_query: Query<&R::RelationshipTarget>,
        // exclude those that have `Replicate` (as we don't want to overwrite the `ReplicateLike` component
        // for their descendants, and we don't want to add `ReplicateLike` on them)
        child_filter: Query<(), (Without<DisableReplicateHierarchy>, Without<Replicate>)>,
    ) {
        root_query
            .iter()
            .for_each(|(mut root, maybe_replicate_like)| {
                // If we are already ReplicateLike another entity, we use it as root
                if let Some(ReplicateLike { root: new_root }) = maybe_replicate_like {
                    root = *new_root;
                }

                // we go through all the descendants (instead of just the children) so that the root is added
                // and we don't need to search for the root ancestor in the replication systems
                let mut stack = SmallVec::<[Entity; 8]>::new();
                stack.push(root);
                while let Some(parent) = stack.pop() {
                    for child in children_query.relationship_sources(parent) {
                        if let Ok(()) = child_filter.get(child) {
                            // TODO: should we buffer those inside a SmallVec for batch insert?
                            trace!("Adding ReplicateLike to child {child:?} with root {root:?}.");
                            commands.entity(child).insert(ReplicateLike { root });
                            stack.push(child);
                        }
                    }
                }
            })
    }

    // TODO: but are the children's despawn replicated? or maybe there's no need because the root's despawned
    //  is replicated, and despawns are recursive
    /// If `Replicate` is removed on an entity that has `Children`
    /// then we remove `ReplicateLike(Entity)` on all the descendants.
    ///
    /// Note that this doesn't happen if the `DisableReplicateHierarchy` is present.
    ///
    /// If a child entity already has the `Replicate` component, we ignore it and its descendants.
    pub(crate) fn propagate_replicate_like_replication_marker_removed(
        trigger: On<Remove, Replicate>,
        root_query: Query<
            (),
            (
                With<R::RelationshipTarget>,
                Without<DisableReplicateHierarchy>,
                With<Replicate>,
            ),
        >,
        children_query: Query<&R::RelationshipTarget>,
        // exclude those that have `Replicate` (as we don't want to remove the `ReplicateLike` component
        // for their descendants)
        child_filter: Query<(), (Without<Replicate>, With<ReplicateLike>)>,
        mut commands: Commands,
    ) {
        let root = trigger.entity;
        // if `DisableReplicateHierarchy` is present, return early since we don't need to propagate `ReplicateLike`
        let Ok(()) = root_query.get(root) else { return };
        let children = children_query.get(root).unwrap();
        // we go through all the descendants (instead of just the children) so that the root is added
        // and we don't need to search for the root ancestor in the replication systems
        let mut stack = SmallVec::<[Entity; 8]>::new();
        stack.push(root);
        while let Some(parent) = stack.pop() {
            for child in children_query.relationship_sources(parent) {
                if let Ok(()) = child_filter.get(child) {
                    stack.push(child);
                    commands.entity(child).try_remove::<ReplicateLike>();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::prelude::Replicate;
    use alloc::vec;
    use bevy_replicon::prelude::{AuthMethod, RepliconSharedPlugin};
    use bevy_replicon::server::ServerPlugin;
    use bevy_state::app::StatesPlugin;

    fn app_with_hierarchy_plugin() -> App {
        let mut app = App::default();
        app.add_plugins(StatesPlugin);
        app.init_resource::<bevy_time::Time>();
        app.add_plugins(RepliconSharedPlugin {
            auth_method: AuthMethod::None,
        });
        app.add_plugins(ServerPlugin::default());
        app.add_plugins(HierarchySendPlugin::<ChildOf>::default());
        app
    }

    /// Check that ReplicateLike propagation works correctly when Children gets updated
    /// on an entity that has ReplicationMarker
    #[test]
    fn propagate_replicate_like_children_updated() {
        let mut app = app_with_hierarchy_plugin();

        let grandparent = app.world_mut().spawn(Replicate::manual(vec![])).id();
        // parent with no ReplicationMarker: ReplicateLike should be propagated
        let grandchild_1 = app.world_mut().spawn_empty().id();
        let child_1 = app.world_mut().spawn_empty().id();
        let parent_1 = app.world_mut().spawn_empty().add_child(child_1).id();

        // parent with ReplicationMarker: the root ReplicateLike shouldn't be propagated
        // but the intermediary ReplicateLike should be propagated to child 2a
        let child_2a = app.world_mut().spawn_empty().id();
        let child_2b = app.world_mut().spawn(Replicate::manual(vec![])).id();
        let child_2c = app
            .world_mut()
            .spawn(ReplicateLike { root: grandparent })
            .id();
        let parent_2 = app
            .world_mut()
            .spawn(Replicate::manual(vec![]))
            .add_children(&[child_2a, child_2b, child_2c])
            .id();

        // parent has Replicate::manual(vec![]) and DisableReplicate so ReplicateLike is not propagated
        let child_3a = app.world_mut().spawn_empty().id();
        let child_3b = app
            .world_mut()
            .spawn(ReplicateLike { root: grandparent })
            .id();
        let parent_3 = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), DisableReplicateHierarchy))
            .add_children(&[child_3a, child_3b])
            .id();

        // parent has DisableReplicate so ReplicateLike is not propagated
        let child_4 = app.world_mut().spawn_empty().id();
        let parent_4 = app
            .world_mut()
            .spawn(DisableReplicateHierarchy)
            .add_child(child_4)
            .id();

        // add Children to the entity which already has Replicate::manual(vec![])
        app.world_mut()
            .entity_mut(grandparent)
            .add_children(&[parent_1, parent_2, parent_3, parent_4]);

        // flush commands
        app.update();

        // Add grandchild which should also get ReplicateLike
        app.world_mut().entity_mut(parent_1).add_child(grandchild_1);

        app.update();

        assert_eq!(
            app.world().get::<ReplicateLike>(parent_1).unwrap().root,
            grandparent
        );
        assert_eq!(
            app.world().get::<ReplicateLike>(child_1).unwrap().root,
            grandparent
        );

        assert!(app.world().get::<ReplicateLike>(parent_2).is_none());
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2a).unwrap().root,
            parent_2
        );
        assert!(app.world().get::<ReplicateLike>(child_2b).is_none());
        // the Parent overrides the ReplicateLike of child_2c
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2c).unwrap().root,
            parent_2
        );

        assert!(app.world().get::<ReplicateLike>(parent_3).is_none());
        assert!(app.world().get::<ReplicateLike>(child_3a).is_none());
        // the parent had DisableReplicateHierarchy so the existing ReplicateLike is not overwritten
        assert_eq!(
            app.world().get::<ReplicateLike>(child_3b).unwrap().root,
            grandparent
        );

        // DisableReplicateHierarchy means that ReplicateLike is not propagated and is not added
        // on the entity itself either
        assert!(app.world().get::<ReplicateLike>(parent_4).is_none());
        assert!(app.world().get::<ReplicateLike>(child_4).is_none());

        // The grandchild should replicate like its parent -> grandparent
        assert_eq!(
            app.world().get::<ReplicateLike>(grandchild_1).unwrap().root,
            grandparent
        );
    }

    /// Check that ReplicateLike propagation works correctly when ReplicationMarker gets added
    /// on an entity that already has children
    #[test]
    fn propagate_replicate_like_replication_marker_added() {
        let mut app = app_with_hierarchy_plugin();

        let grandparent = app.world_mut().spawn_empty().id();
        // parent with no ReplicationMarker: ReplicateLike should be propagated
        let child_1 = app.world_mut().spawn_empty().id();
        let parent_1 = app.world_mut().spawn_empty().add_child(child_1).id();

        // parent with ReplicationMarker: the root ReplicateLike shouldn't be propagated
        // but the intermediary ReplicateLike should be propagated to child 2a
        let child_2a = app.world_mut().spawn_empty().id();
        let child_2b = app.world_mut().spawn(Replicate::manual(vec![])).id();
        let child_2c = app
            .world_mut()
            .spawn(ReplicateLike { root: grandparent })
            .id();
        let parent_2 = app
            .world_mut()
            .spawn(Replicate::manual(vec![]))
            .add_children(&[child_2a, child_2b, child_2c])
            .id();

        // parent has ReplicationMarker and DisableReplicate so ReplicateLike is not propagated
        let child_3a = app.world_mut().spawn_empty().id();
        let child_3b = app
            .world_mut()
            .spawn(ReplicateLike { root: grandparent })
            .id();
        let parent_3 = app
            .world_mut()
            .spawn((Replicate::manual(vec![]), DisableReplicateHierarchy))
            .add_children(&[child_3a, child_3b])
            .id();

        // parent has DisableReplicate so ReplicateLike is not propagated
        let child_4 = app.world_mut().spawn_empty().id();
        let parent_4 = app
            .world_mut()
            .spawn(DisableReplicateHierarchy)
            .add_child(child_4)
            .id();

        app.world_mut()
            .entity_mut(grandparent)
            .add_children(&[parent_1, parent_2, parent_3, parent_4]);
        // add ReplicationMarker to an entity that already has children
        app.world_mut()
            .entity_mut(grandparent)
            .insert(Replicate::manual(vec![]));

        // flush commands
        app.update();
        assert_eq!(
            app.world().get::<ReplicateLike>(parent_1).unwrap().root,
            grandparent
        );
        assert_eq!(
            app.world().get::<ReplicateLike>(child_1).unwrap().root,
            grandparent
        );

        assert!(app.world().get::<ReplicateLike>(parent_2).is_none());
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2a).unwrap().root,
            parent_2
        );
        assert!(app.world().get::<ReplicateLike>(child_2b).is_none());
        // the Parent overrides the ReplicateLike of child_2c
        assert_eq!(
            app.world().get::<ReplicateLike>(child_2c).unwrap().root,
            parent_2
        );

        assert!(app.world().get::<ReplicateLike>(parent_3).is_none());
        assert!(app.world().get::<ReplicateLike>(child_3a).is_none());
        // the parent had DisableReplicateHierarchy so the existing ReplicateLike is not overwritten
        assert_eq!(
            app.world().get::<ReplicateLike>(child_3b).unwrap().root,
            grandparent
        );

        // DisableReplicateHierarchy means that ReplicateLike is not propagated and is not added
        // on the entity itself either
        assert!(app.world().get::<ReplicateLike>(parent_4).is_none());
        assert!(app.world().get::<ReplicateLike>(child_4).is_none());
    }
}
