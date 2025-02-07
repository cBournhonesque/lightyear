//! This module is responsible for making sure that parent-children hierarchies are replicated correctly.

use crate::client::replication::send::ReplicateToServer;
use crate::prelude::client::{InterpolationSet, PredictionSet};
use crate::prelude::server::ControlledBy;
use crate::prelude::{
    NetworkRelevanceMode, PrePredicted, Replicated, Replicating, ReplicationGroup,
};
use crate::server::replication::send::ReplicationTarget;
use crate::server::replication::send::SyncTarget;
use crate::shared::replication::authority::{AuthorityPeer, HasAuthority};
use crate::shared::replication::components::{DisableReplicateHierarchy, ReplicationMarker};
use crate::shared::replication::{ReplicationPeer, ReplicationSend};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};
use bevy::ecs::component::{ComponentHooks, HookContext, Immutable, StorageType};
use bevy::ecs::entity::{MapEntities, VisitEntities, VisitEntitiesMut};
use bevy::ecs::reflect::{ReflectMapEntities, ReflectVisitEntities, ReflectVisitEntitiesMut};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

// /// This component can be added to an entity to replicate the entity's hierarchy to the remote world.
// /// The `ParentSync` component will be updated automatically when the `ChildOf` component changes,
// /// and the entity's hierarchy will automatically be updated when the `ParentSync` component changes.
// ///
// /// Updates entity's `ChildOf` component on change.
// /// Removes the parent if `None`.
// #[derive(Component, Default, Reflect, Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
// #[reflect(Component)]
// pub struct ParentSync(Option<Entity>);
//
// impl MapEntities for ParentSync {
//     fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
//         if let Some(entity) = &mut self.0 {
//             *entity = entity_mapper.map_entity(*entity);
//         }
//     }
// }

pub struct HierarchySendPlugin<R> {
    _marker: std::marker::PhantomData<R>,
}

impl<R> Default for HierarchySendPlugin<R> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

/// Marker component that indicates that this entity should be replicated similarly to the entity
/// contained in the component.
///
/// This will be inserted automaticallyk
// TODO: should we make this immutable?
#[derive(Component, Clone, Copy, VisitEntities, VisitEntitiesMut, Reflect, PartialEq, Debug)]
#[reflect(
    Component,
    MapEntities,
    VisitEntities,
    VisitEntitiesMut,
    PartialEq,
    Debug
)]
pub struct ReplicateLike(pub(crate) Entity);

// impl Component for ReplicateLike {
//     const STORAGE_TYPE: StorageType = Default::default();
//     type Mutability = Immutable;
//
//     fn register_component_hooks(hooks: &mut ComponentHooks) {
//         // when ReplicateLike is removed, we remove it from all children as well
//         hooks.on_remove(|world: DeferredWorld, ctx: HookContext| {
//
//         });
//     }
// }

/// If `ReplicateLike` is added on an entity that has `ReplicationMarker` (i.e. has the replication components)
/// then we add `ReplicateLike(root)` on all the descendants.
///
/// Note that this doesn't happen if the `DisableReplicateHierarchy` is present.
///
/// If a child entity already has the `ReplicationMarker` component, we ignore it and its descendants.
fn propagate_replicate_like_replication_marker_added(
    // do the propagation if either ReplicationMarker is added OR Children is inserted
    // we can't directly use an observer to see if Children is updated, so we have another
    // trigger if ChildOf is inserted!
    // (The Children trigger is still necessary, because when ChildOf is inserted,
    // the other observer runs before Children has been added to the parent entity, so this function
    // returns early)
    trigger: Trigger<OnInsert, (ReplicationMarker, Children)>,
    root_query: Query<
        (),
        (
            With<Children>,
            Without<DisableReplicateHierarchy>,
            With<ReplicationMarker>,
        ),
    >,
    children_query: Query<&Children>,
    // exclude those that have `ReplicationMarker` (as we don't want to overwrite the `ReplicateLike` component
    // for their descendants, and we don't want to add `ReplicateLike` on them)
    child_filter: Query<Has<DisableReplicateHierarchy>, Without<ReplicationMarker>>,
    mut commands: Commands,
) {
    propagate_replicate_like(
        trigger.target(),
        &root_query,
        &children_query,
        &child_filter,
        &mut commands,
    );
}

/// If `ReplicateLike` is added on an entity that has `ReplicationMarker` (i.e. has the replication components)
/// then we add `ReplicateLike(root)` on all the descendants.
///
/// Note that this doesn't happen if the `DisableReplicateHierarchy` is present.
///
/// If a child entity already has the `ReplicationMarker` component, we ignore it and its descendants.
fn propagate_replicate_like_children_updated(
    // do the propagation if Children is updated on an entity that has ReplicationMarker
    // we can't directly use an observer to see if Children is updated, so instead trigger on ChildOf
    trigger: Trigger<OnAdd, ChildOf>,
    parent_query: Query<(), (Without<DisableReplicateHierarchy>, With<ReplicationMarker>)>,
    child_of_query: Query<&ChildOf>,
    // root_query: Query<(), (With<Children>, Without<DisableReplicateHierarchy>, With<ReplicationMarker>)>,
    // children_query: Query<&Children>,
    // // exclude those that have `ReplicationMarker` (as we don't want to overwrite the `ReplicateLike` component
    // // for their descendants, and we don't want to add `ReplicateLike` on them)
    // child_filter: Query<Has<DisableReplicateHierarchy>, Without<ReplicationMarker>>,
    mut commands: Commands,
) {
    let root = child_of_query.related(trigger.target()).unwrap();
    if let Ok(()) = parent_query.get(root) {
        // We cannot directly run the observer here because it will run right after the OnAdd hook,
        // but without any flushes so the Children component won't have been updated
        //
        // Instead, as a hack, we insert ReplicationMarker to trigger the other propagate observer
        commands.entity(root).insert(ReplicationMarker);
    }

    // propagate_replicate_like(
    //     root,
    //     &root_query,
    //     &children_query,
    //     &child_filter,
    //     &mut commands);
}

// hook: OnAdd Child1
// queue insert Children
// observer: queue insert ReplicationMarker Child1
// flush
// Children inserted
// hook: OnAdd Child2
// h

fn propagate_replicate_like(
    root: Entity,
    root_query: &Query<
        (),
        (
            With<Children>,
            Without<DisableReplicateHierarchy>,
            With<ReplicationMarker>,
        ),
    >,
    children_query: &Query<&Children>,
    // exclude those that have `ReplicationMarker` (as we don't want to overwrite the `ReplicateLike` component
    // for their descendants, and we don't want to add `ReplicateLike` on them)
    child_filter: &Query<Has<DisableReplicateHierarchy>, Without<ReplicationMarker>>,
    commands: &mut Commands,
) {
    // if `DisableReplicateHierarchy` is present, return early since we don't need to propagate `ReplicateLike`
    let Ok(()) = root_query.get(root) else { return };
    let children = children_query.get(root).unwrap();
    // we go through all the descendants (instead of just the children) so that the root is added
    // and we don't need to search for the root ancestor in the replication systems
    let mut stack = SmallVec::<[Entity; 8]>::new();
    stack.push(root);
    while let Some(parent) = stack.pop() {
        for child in children_query.relationship_sources(parent) {
            if let Ok(disable_hierarchy) = child_filter.get(child) {
                // TODO: should we buffer those inside a SmallVec for batch insert?
                // we always add ReplicateLike to the child, but we keep going recursively only if
                // it does not have `DisableReplicateHierarchy`
                commands.entity(child).insert(ReplicateLike(root));
                if !disable_hierarchy {
                    stack.push(child);
                }
            }
        }
    }
}

impl<R: ReplicationSend> HierarchySendPlugin<R> {
    // /// Update ParentSync if the hierarchy changed
    // /// (run this in post-update before replicating, to account for any hierarchy changed initiated by the user)
    // ///
    // /// This only runs on the sending side
    // fn update_parent_sync(
    //     mut query: Query<(Ref<ChildOf>, &mut ParentSync), With<ReplicateHierarchy>>,
    // ) {
    //     for (parent, mut parent_sync) in query.iter_mut() {
    //         if parent.is_changed() || parent_sync.is_added() {
    //             trace!(
    //                 ?parent,
    //                 ?parent_sync,
    //                 "Update parent sync because hierarchy has changed"
    //             );
    //             parent_sync.set_if_neq(ParentSync(Some(**parent)));
    //         }
    //     }
    // }

    // /// Update ParentSync if the parent has been removed
    // ///
    // /// This only runs on the sending side
    // fn handle_parent_remove(
    //     trigger: Trigger<OnRemove, ChildOf>,
    //     mut hierarchy: Query<&mut ParentSync, With<ReplicateHierarchy>>,
    // ) {
    //     if let Ok(mut parent_sync) = hierarchy.get_mut(trigger.target()) {
    //         parent_sync.0 = None;
    //     }
    // }
}

impl<R: ReplicationSend> Plugin for HierarchySendPlugin<R> {
    fn build(&self, app: &mut App) {
        app.add_observer(propagate_replicate_like_children_updated);
        app.add_observer(propagate_replicate_like_replication_marker_added);
        // app.add_systems(
        //     PostUpdate,
        //     (Self::propagate_replicate, Self::update_parent_sync)
        //         .chain()
        //         // we don't need to run these every frame, only every send_interval
        //         .in_set(InternalReplicationSet::<R::SetMarker>::SendMessages)
        //         // run before the replication-send systems
        //         .before(InternalReplicationSet::<R::SetMarker>::All),
        // );
    }
}

pub struct HierarchyReceivePlugin<R> {
    _marker: std::marker::PhantomData<R>,
}

impl<R> Default for HierarchyReceivePlugin<R> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<R> HierarchyReceivePlugin<R> {
    // /// Update parent/children hierarchy if parent_sync changed
    // ///
    // /// This only runs on the receiving side
    // fn update_parent(
    //     mut commands: Commands,
    //     hierarchy: Query<
    //         (Entity, &ParentSync, Option<&ChildOf>),
    //         (Changed<ParentSync>, Without<ReplicationTarget>),
    //     >,
    // ) {
    //     for (entity, parent_sync, parent) in hierarchy.iter() {
    //         trace!(
    //             "update_parent: entity: {:?}, parent_sync: {:?}, parent: {:?}",
    //             entity,
    //             parent_sync,
    //             parent
    //         );
    //         if let Some(new_parent) = parent_sync.0 {
    //             if parent.filter(|&parent| **parent == new_parent).is_none() {
    //                 commands.entity(entity).insert(ChildOf(new_parent));
    //             }
    //         } else if parent.is_some() {
    //             commands.entity(entity).remove::<ChildOf>();
    //         }
    //     }
    // }
}

impl<R: ReplicationPeer> Plugin for HierarchyReceivePlugin<R> {
    fn build(&self, app: &mut App) {
        // // REFLECTION
        // app.register_type::<ParentSync>();
        //
        // // TODO: does this work for client replication? (client replicating to other clients via the server?)
        // // when we receive a ParentSync update from the remote, update the hierarchy
        // app.add_systems(
        //     PreUpdate,
        //     Self::update_parent
        //         .after(InternalMainSet::<R::SetMarker>::Receive)
        //         // we want update_parent to run in the same frame that ParentSync is propagated
        //         // to the predicted/interpolated entities
        //         .after(PredictionSet::SpawnHistory)
        //         .after(InterpolationSet::SpawnHistory),
        // );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bevy::prelude::Entity;

    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;

    fn setup_hierarchy() -> (BevyStepper, Entity, Entity, Entity) {
        let mut stepper = BevyStepper::default();
        let child = stepper
            .server_app
            .world_mut()
            .spawn(ComponentSyncModeOnce(0.0))
            .id();
        let parent = stepper
            .server_app
            .world_mut()
            .spawn(ComponentSyncModeSimple(0.0))
            .add_child(child)
            .id();
        let grandparent = stepper
            .server_app
            .world_mut()
            .spawn(ComponentSyncModeFull(0.0))
            .add_child(parent)
            .id();
        (stepper, grandparent, parent, child)
    }

    /// Check that ReplicateLike propagation works correctly when Children gets updated
    /// on an entity that has ReplicationMarker
    #[test]
    fn propagate_replicate_like_children_updated() {
        let mut stepper = BevyStepper::default();

        let grandparent = stepper.server_app.world_mut().spawn(ReplicationMarker).id();
        // parent with no ReplicationMarker: ReplicateLike should be propagated
        let child_1 = stepper.server_app.world_mut().spawn_empty().id();
        let parent_1 = stepper
            .server_app
            .world_mut()
            .spawn_empty()
            .add_child(child_1)
            .id();

        // parent with ReplicationMarker: the root ReplicateLike shouldn't be propagated
        // but the intermediary ReplicateLike should be propagated to child 2a
        let child_2a = stepper.server_app.world_mut().spawn_empty().id();
        let child_2b = stepper.server_app.world_mut().spawn(ReplicationMarker).id();
        let child_2c = stepper
            .server_app
            .world_mut()
            .spawn(ReplicateLike(grandparent))
            .id();
        let parent_2 = stepper
            .server_app
            .world_mut()
            .spawn(ReplicationMarker)
            .add_children(&[child_2a, child_2b, child_2c])
            .id();

        // parent has ReplicationMarker and DisableReplicate so ReplicateLike is not propagated
        let child_3a = stepper.server_app.world_mut().spawn_empty().id();
        let child_3b = stepper
            .server_app
            .world_mut()
            .spawn(ReplicateLike(grandparent))
            .id();
        let parent_3 = stepper
            .server_app
            .world_mut()
            .spawn((ReplicationMarker, DisableReplicateHierarchy))
            .add_children(&[child_3a, child_3b])
            .id();

        // parent has DisableReplicate so ReplicateLike is not propagated
        let child_4 = stepper.server_app.world_mut().spawn_empty().id();
        let parent_4 = stepper
            .server_app
            .world_mut()
            .spawn(DisableReplicateHierarchy)
            .add_child(child_4)
            .id();

        // add Children to the entity which already has ReplicationMarker
        stepper
            .server_app
            .world_mut()
            .entity_mut(grandparent)
            .add_children(&[parent_1, parent_2, parent_3, parent_4]);

        // flush commands
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(parent_1)
                .unwrap()
                .0,
            grandparent
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(child_1)
                .unwrap()
                .0,
            grandparent
        );

        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(parent_2)
            .is_none());
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(child_2a)
                .unwrap()
                .0,
            parent_2
        );
        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(child_2b)
            .is_none());
        // the Parent overrides the ReplicateLike of child_2c
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(child_2c)
                .unwrap()
                .0,
            parent_2
        );

        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(parent_3)
            .is_none());
        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(child_3a)
            .is_none());
        // the parent had DisableReplicateHierarchy so the existing ReplicateLike is not overwritten
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(child_3b)
                .unwrap()
                .0,
            grandparent
        );

        // DisableReplicateHierarchy means that ReplicateLike is not propagated, but it's still added to
        // the entity itself
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(parent_4)
                .unwrap()
                .0,
            grandparent
        );
        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(child_4)
            .is_none());
    }

    /// Check that ReplicateLike propagation works correctly when ReplicationMarker gets added
    /// on an entity that already has children
    #[test]
    fn propagate_replicate_like_replication_marker_added() {
        let mut stepper = BevyStepper::default();

        let grandparent = stepper.server_app.world_mut().spawn_empty().id();
        // parent with no ReplicationMarker: ReplicateLike should be propagated
        let child_1 = stepper.server_app.world_mut().spawn_empty().id();
        let parent_1 = stepper
            .server_app
            .world_mut()
            .spawn_empty()
            .add_child(child_1)
            .id();

        // parent with ReplicationMarker: the root ReplicateLike shouldn't be propagated
        // but the intermediary ReplicateLike should be propagated to child 2a
        let child_2a = stepper.server_app.world_mut().spawn_empty().id();
        let child_2b = stepper.server_app.world_mut().spawn(ReplicationMarker).id();
        let child_2c = stepper
            .server_app
            .world_mut()
            .spawn(ReplicateLike(grandparent))
            .id();
        let parent_2 = stepper
            .server_app
            .world_mut()
            .spawn(ReplicationMarker)
            .add_children(&[child_2a, child_2b, child_2c])
            .id();

        // parent has ReplicationMarker and DisableReplicate so ReplicateLike is not propagated
        let child_3a = stepper.server_app.world_mut().spawn_empty().id();
        let child_3b = stepper
            .server_app
            .world_mut()
            .spawn(ReplicateLike(grandparent))
            .id();
        let parent_3 = stepper
            .server_app
            .world_mut()
            .spawn((ReplicationMarker, DisableReplicateHierarchy))
            .add_children(&[child_3a, child_3b])
            .id();

        // parent has DisableReplicate so ReplicateLike is not propagated
        let child_4 = stepper.server_app.world_mut().spawn_empty().id();
        let parent_4 = stepper
            .server_app
            .world_mut()
            .spawn(DisableReplicateHierarchy)
            .add_child(child_4)
            .id();

        stepper
            .server_app
            .world_mut()
            .entity_mut(grandparent)
            .add_children(&[parent_1, parent_2, parent_3, parent_4]);
        // add ReplicationMarker to an entity that already has children
        stepper
            .server_app
            .world_mut()
            .entity_mut(grandparent)
            .insert(ReplicationMarker);

        // flush commands
        stepper.frame_step();
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(parent_1)
                .unwrap()
                .0,
            grandparent
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(child_1)
                .unwrap()
                .0,
            grandparent
        );

        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(parent_2)
            .is_none());
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(child_2a)
                .unwrap()
                .0,
            parent_2
        );
        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(child_2b)
            .is_none());
        // the Parent overrides the ReplicateLike of child_2c
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(child_2c)
                .unwrap()
                .0,
            parent_2
        );

        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(parent_3)
            .is_none());
        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(child_3a)
            .is_none());
        // the parent had DisableReplicateHierarchy so the existing ReplicateLike is not overwritten
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(child_3b)
                .unwrap()
                .0,
            grandparent
        );

        // DisableReplicateHierarchy means that ReplicateLike is not propagated, but it's still added to
        // the entity itself
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ReplicateLike>(parent_4)
                .unwrap()
                .0,
            grandparent
        );
        assert!(stepper
            .server_app
            .world()
            .get::<ReplicateLike>(child_4)
            .is_none());
    }

    //
    // #[test]
    // fn test_update_parent() {
    //     let (mut stepper, grandparent, parent, child) = setup_hierarchy();
    //
    //     let replicate = Replicate {
    //         hierarchy: ReplicateHierarchy {
    //             enabled: true,
    //             recursive: false,
    //         },
    //         // make sure that child and parent are replicated in the same group, so that both entities are spawned
    //         // before entity mapping is done
    //         group: ReplicationGroup::new_id(0),
    //         ..default()
    //     };
    //     stepper
    //         .server_app
    //         .world_mut()
    //         .entity_mut(parent)
    //         .insert((replicate.clone(), ParentSync::default()));
    //     stepper
    //         .server_app
    //         .world_mut()
    //         .entity_mut(grandparent)
    //         .insert(replicate.clone());
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // check that the parent got replicated, along with the hierarchy information
    //     let client_grandparent = stepper
    //         .client_app
    //         .world_mut()
    //         .query_filtered::<Entity, With<ComponentSyncModeFull>>()
    //         .get_single(stepper.client_app.world())
    //         .unwrap();
    //     let (client_parent, client_parent_sync, client_parent_component) = stepper
    //         .client_app
    //         .world_mut()
    //         .query_filtered::<(Entity, &ParentSync, &ChildOf), With<ComponentSyncModeSimple>>()
    //         .get_single(stepper.client_app.world())
    //         .unwrap();
    //
    //     assert_eq!(client_parent_sync.0, Some(client_grandparent));
    //     assert_eq!(*client_parent_component.deref(), client_grandparent);
    //
    //     // remove the hierarchy on the sender side
    //     stepper
    //         .server_app
    //         .world_mut()
    //         .entity_mut(parent)
    //         .remove::<ChildOf>();
    //     stepper.frame_step();
    //     stepper.frame_step();
    //     // 1. make sure that parent sync has been updated on the sender side
    //     assert_eq!(
    //         stepper
    //             .server_app
    //             .world_mut()
    //             .entity_mut(parent)
    //             .get::<ParentSync>(),
    //         Some(&ParentSync(None))
    //     );
    //
    //     // 2. make sure that the parent has been removed on the receiver side, and that ParentSync has been updated
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world_mut()
    //             .entity_mut(client_parent)
    //             .get::<ParentSync>(),
    //         Some(&ParentSync(None))
    //     );
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world_mut()
    //             .entity_mut(client_parent)
    //             .get::<ChildOf>(),
    //         None,
    //     );
    //     assert!(stepper
    //         .client_app
    //         .world_mut()
    //         .entity_mut(client_grandparent)
    //         .get::<Children>()
    //         .is_none());
    // }
    //
    // #[test]
    // fn test_propagate_hierarchy() {
    //     // tracing_subscriber::FmtSubscriber::builder()
    //     //     .with_max_level(tracing::Level::ERROR)
    //     //     .init();
    //     let (mut stepper, grandparent, parent, child) = setup_hierarchy();
    //
    //     stepper
    //         .server_app
    //         .world_mut()
    //         .entity_mut(grandparent)
    //         .insert(Replicate::default());
    //
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // 1. check that the parent and child have been replicated
    //     let client_grandparent = stepper
    //         .client_app
    //         .world_mut()
    //         .query_filtered::<Entity, With<ComponentSyncModeFull>>()
    //         .get_single(stepper.client_app.world())
    //         .unwrap();
    //     let client_parent = stepper
    //         .client_app
    //         .world_mut()
    //         .query_filtered::<Entity, With<ComponentSyncModeSimple>>()
    //         .get_single(stepper.client_app.world())
    //         .unwrap();
    //     let client_child = stepper
    //         .client_app
    //         .world_mut()
    //         .query_filtered::<Entity, With<ComponentSyncModeOnce>>()
    //         .get_single(stepper.client_app.world())
    //         .unwrap();
    //
    //     // 2. check that the hierarchies have been replicated
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world_mut()
    //             .entity_mut(client_parent)
    //             .get::<ChildOf>()
    //             .unwrap()
    //             .deref(),
    //         &client_grandparent
    //     );
    //     assert_eq!(
    //         stepper
    //             .client_app
    //             .world_mut()
    //             .entity_mut(client_child)
    //             .get::<ChildOf>()
    //             .unwrap()
    //             .deref(),
    //         &client_parent
    //     );
    //
    //     // 3. check that the replication group has been set correctly
    //     assert_eq!(
    //         stepper
    //             .server_app
    //             .world_mut()
    //             .entity_mut(parent)
    //             .get::<ReplicationGroup>(),
    //         Some(&ReplicationGroup::new_id(grandparent.to_bits()))
    //     );
    //     assert_eq!(
    //         stepper
    //             .server_app
    //             .world_mut()
    //             .entity_mut(child)
    //             .get::<ReplicationGroup>(),
    //         Some(&ReplicationGroup::new_id(grandparent.to_bits()))
    //     );
    // }
    //
    // #[test]
    // fn test_propagate_hierarchy_client_to_server() {
    //     let mut stepper = BevyStepper::default();
    //     let child = stepper
    //         .client_app
    //         .world_mut()
    //         .spawn(ComponentClientToServer(0.0))
    //         .id();
    //     let parent = stepper
    //         .client_app
    //         .world_mut()
    //         .spawn((ComponentSyncModeFull(0.0), client::Replicate::default()))
    //         .add_child(child)
    //         .id();
    //
    //     for _ in 0..10 {
    //         stepper.frame_step();
    //     }
    //
    //     // check that both the parent and the child were replicated
    //     let server_parent = stepper
    //         .server_app
    //         .world_mut()
    //         .query_filtered::<Entity, With<ComponentSyncModeFull>>()
    //         .get_single(stepper.server_app.world())
    //         .expect("parent entity was not replicated");
    //     let server_child = stepper
    //         .server_app
    //         .world_mut()
    //         .query_filtered::<Entity, With<ComponentClientToServer>>()
    //         .get_single(stepper.server_app.world())
    //         .expect("child entity was not replicated");
    //     assert_eq!(
    //         stepper
    //             .server_app
    //             .world()
    //             .get::<ChildOf>(server_child)
    //             .unwrap()
    //             .get(),
    //         server_parent
    //     );
    //     assert_eq!(
    //         stepper
    //             .server_app
    //             .world()
    //             .get::<ParentSync>(server_child)
    //             .unwrap(),
    //         &ParentSync(Some(server_parent))
    //     );
    // }
    //
    // #[test]
    // fn test_remove_child() {
    //     let mut stepper = BevyStepper::default();
    //     let child = stepper
    //         .client_app
    //         .world_mut()
    //         .spawn(ComponentSyncModeFull(0.0))
    //         .id();
    //     let parent = stepper
    //         .client_app
    //         .world_mut()
    //         .spawn((ComponentSyncModeSimple(0.0), client::Replicate::default()))
    //         .add_child(child)
    //         .id();
    //     stepper
    //         .client_app
    //         .world_mut()
    //         .commands()
    //         .entity(child)
    //         .despawn();
    //
    //     for _ in 0..10 {
    //         stepper.frame_step();
    //     }
    //
    //     // check that child was removed
    //     let server_child = stepper
    //         .server_app
    //         .world_mut()
    //         .query_filtered::<Entity, With<ComponentSyncModeFull>>()
    //         .get_single(stepper.server_app.world());
    //     assert!(server_child.is_err());
    // }
    //
    // /// https://github.com/cBournhonesque/lightyear/issues/649
    // /// P1 with child C1
    // /// If you add a new client to the replication target of P1, then both
    // /// P1 and C1 should be replicated to the new client.
    // /// (the issue says that only P1 was replicated)
    // #[test]
    // fn test_new_client_is_added_to_parent() {
    //     let mut stepper = MultiBevyStepper::default();
    //
    //     let c1 = ClientId::Netcode(TEST_CLIENT_ID_1);
    //     let c2 = ClientId::Netcode(TEST_CLIENT_ID_2);
    //
    //     let server_child = stepper.server_app.world_mut().spawn_empty().id();
    //     let server_parent = stepper
    //         .server_app
    //         .world_mut()
    //         .spawn(server::Replicate {
    //             target: ReplicationTarget {
    //                 target: NetworkTarget::Single(c1),
    //             },
    //             ..default()
    //         })
    //         .add_child(server_child)
    //         .id();
    //
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     let c1_child = stepper
    //         .client_app_1
    //         .world()
    //         .resource::<client::ConnectionManager>()
    //         .replication_receiver
    //         .remote_entity_map
    //         .get_local(server_child)
    //         .expect("child entity was not replicated to client 1");
    //     let c1_parent = stepper
    //         .client_app_1
    //         .world()
    //         .resource::<client::ConnectionManager>()
    //         .replication_receiver
    //         .remote_entity_map
    //         .get_local(server_parent)
    //         .expect("parent entity was not replicated to client 1");
    //
    //     // change the replication target to include a new client
    //     stepper
    //         .server_app
    //         .world_mut()
    //         .get_mut::<ReplicationTarget>(server_parent)
    //         .unwrap()
    //         .target = NetworkTarget::Only(vec![c1, c2]);
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // check that both parent and child were replicated to the new client
    //     let c2_child = stepper
    //         .client_app_2
    //         .world()
    //         .resource::<client::ConnectionManager>()
    //         .replication_receiver
    //         .remote_entity_map
    //         .get_local(server_child)
    //         .expect("child entity was not replicated to client 2");
    //     let c2_parent = stepper
    //         .client_app_2
    //         .world()
    //         .resource::<client::ConnectionManager>()
    //         .replication_receiver
    //         .remote_entity_map
    //         .get_local(server_parent)
    //         .expect("parent entity was not replicated to client 2");
    // }
    //
    // /// https://github.com/cBournhonesque/lightyear/issues/547
    // /// Test that when a new child is added to a parent that has ReplicateHierarchy.true
    // /// the child is also replicated to the remote
    // #[test]
    // fn test_propagate_hierarchy_new_child() {
    //     let mut stepper = BevyStepper::default();
    //     let server_parent = stepper
    //         .server_app
    //         .world_mut()
    //         .spawn(server::Replicate::default())
    //         .id();
    //     stepper.frame_step();
    //     stepper.frame_step();
    //     let client_parent = stepper
    //         .client_app
    //         .world()
    //         .resource::<client::ConnectionManager>()
    //         .replication_receiver
    //         .remote_entity_map
    //         .get_local(server_parent)
    //         .expect("parent entity was not replicated to client");
    //
    //     // add a child to the entity
    //     let server_child = stepper.server_app.world_mut().spawn_empty().id();
    //     stepper
    //         .server_app
    //         .world_mut()
    //         .entity_mut(server_parent)
    //         .add_child(server_child);
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // check that Replicate was propagated to the child, and that the child
    //     // was replicated to the client
    //     let client_child = stepper
    //         .client_app
    //         .world()
    //         .resource::<client::ConnectionManager>()
    //         .replication_receiver
    //         .remote_entity_map
    //         .get_local(server_child)
    //         .expect("child entity was not replicated to client");
    // }
}
