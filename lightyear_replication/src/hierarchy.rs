//! This module is responsible for making sure that parent-children hierarchies are replicated correctly.

use crate::prelude::{PrePredicted, Replicate, ReplicationBufferSet};
use crate::registry::registry::AppComponentExt;
use alloc::vec::Vec;
use bevy_app::prelude::*;
use bevy_ecs::entity::MapEntities;
use bevy_ecs::prelude::*;
use bevy_ecs::reflect::ReflectMapEntities;
use bevy_reflect::Reflect;
use core::fmt::Debug;
use smallvec::SmallVec;
use tracing::trace;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum RelationshipSet {
    // PreUpdate
    Receive,
    // PostUpdate
    Send,
}

/// When the `DisableReplicateHierarchy` marker component is added to an entity, we will stop replicating their children.
///
/// If the component is added on an entity with `Replicate`, it's children will be replicated using
/// the same replication settings as the Parent.
/// This is achieved via the marker component `ReplicateLikeParent` added on each child.
/// You can remove the `ReplicateLikeParent` component to disable this on a child entity. You can then
/// add the replication components on the child to replicate it independently from the parents.
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect)]
#[reflect(Component)]
pub struct DisableReplicateHierarchy;

/// Marker component that indicates that this entity should be replicated similarly to the entity
/// contained in the component.
///
/// This will be inserted automatically on all children of an entity that has `Replicate`
#[derive(Component, Clone, MapEntities, Copy, Reflect, PartialEq, Debug)]
#[relationship(relationship_target=ReplicateLikeChildren)]
#[reflect(Component, MapEntities, PartialEq, Debug)]
pub struct ReplicateLike {
    #[entities]
    pub root: Entity,
}

#[derive(Component, Debug, Reflect)]
#[relationship_target(relationship=ReplicateLike, linked_spawn)]
#[reflect(Component)]
pub struct ReplicateLikeChildren(Vec<Entity>);

/// Plugin that helps lightyear propagate replication components through the ChildOf relationship.
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
///
/// Note that currently propagating the replication components and propagating `ChildOfSync` (which helps you
/// replicate the `ChildOf` relationship) have the same logic. They use the same `DisableReplicateHierarchy` to
/// determine when to stop the propagation.
#[derive(Default)]
pub struct HierarchySendPlugin;

impl Plugin for HierarchySendPlugin {
    fn build(&self, app: &mut App) {
        app.register_component::<ChildOf>()
            .add_component_map_entities();

        // propagate ReplicateLike
        app.add_observer(Self::propagate_replicate_like_replication_marker_removed);
        app.add_systems(
            PostUpdate,
            Self::propagate_through_hierarchy
                // update replication components before we actually run the Buffer systems
                .in_set(ReplicationBufferSet::BeforeBuffer),
        );
    }
}

impl HierarchySendPlugin {
    /// Propagate certain replication components through the hierarchy.
    /// - If new children are added, `Replicate` is added, `PrePredicted` is added, we recursively
    ///   go through the descendants and add `ReplicateLike`, `ChildOfSync`, ... if the child does not have
    ///   `DisableReplicateHierarchy` or `Replicate` already
    /// - We run this as a system and not an observer because observers cannot handle Children updates very well
    ///   (if we trigger on ChildOf being added, there is no flush between the ChildOf OnAdd hook and the observer
    ///   so the `&Children` query won't be updated (or the component will not exist on the parent yet)
    fn propagate_through_hierarchy(
        mut commands: Commands,
        root_query: Query<
            (Entity, Has<PrePredicted>),
            (
                With<Replicate>,
                Without<DisableReplicateHierarchy>,
                With<Children>,
                Or<(Changed<Children>, Added<PrePredicted>, Added<Replicate>)>,
            ),
        >,
        children_query: Query<&Children>,
        // exclude those that have `Replicate` (as we don't want to overwrite the `ReplicateLike` component
        // for their descendants, and we don't want to add `ReplicateLike` on them)
        child_filter: Query<(), (Without<DisableReplicateHierarchy>, Without<Replicate>)>,
    ) {
        root_query.iter().for_each(|(root, pre_predicted)| {
            // we go through all the descendants (instead of just the children) so that the root is added
            // and we don't need to search for the root ancestor in the replication systems
            let mut stack = SmallVec::<[Entity; 8]>::new();
            stack.push(root);
            while let Some(parent) = stack.pop() {
                for child in children_query.relationship_sources(parent) {
                    if let Ok(()) = child_filter.get(child) {
                        // TODO: should we buffer those inside a SmallVec for batch insert?
                        trace!("Adding ReplicateLike to child {child:?} with root {root:?}. PrePredicted: {pre_predicted:?}");
                        commands
                            .entity(child)
                            .insert(ReplicateLike { root });
                        if pre_predicted {
                            trace!("Adding PrePredicted to child {child:?} with root {root:?}");
                            commands.entity(child).insert(PrePredicted::default());
                        }
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
        trigger: Trigger<OnRemove, Replicate>,
        root_query: Query<
            (),
            (
                With<Children>,
                Without<DisableReplicateHierarchy>,
                With<Replicate>,
            ),
        >,
        children_query: Query<&Children>,
        // exclude those that have `Replicate` (as we don't want to remove the `ReplicateLike` component
        // for their descendants)
        child_filter: Query<(), (Without<Replicate>, With<ReplicateLike>)>,
        mut commands: Commands,
    ) {
        let root = trigger.target();
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

    use crate::send::components::Replicate;
    use alloc::vec;

    fn setup_hierarchy() -> (App, Entity, Entity, Entity) {
        let mut app = App::default();
        app.add_plugins(HierarchySendPlugin);
        let grandparent = app.world_mut().spawn_empty().id();
        let parent = app.world_mut().spawn(ChildOf(grandparent)).id();
        let child = app.world_mut().spawn(ChildOf(parent)).id();
        (app, grandparent, parent, child)
    }

    /// Check that ReplicateLike propagation works correctly when Children gets updated
    /// on an entity that has ReplicationMarker
    #[test]
    fn propagate_replicate_like_children_updated() {
        let mut app = App::default();
        app.add_plugins(HierarchySendPlugin);

        let grandparent = app.world_mut().spawn(Replicate::manual(vec![])).id();
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

    /// Check that ReplicateLike propagation works correctly when ReplicationMarker gets added
    /// on an entity that already has children
    #[test]
    fn propagate_replicate_like_replication_marker_added() {
        let mut app = App::default();
        app.add_plugins(HierarchySendPlugin);

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
