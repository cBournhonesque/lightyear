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
use bevy::ecs::component::{ComponentHooks, HookContext, Immutable, Mutable, StorageType};
use bevy::ecs::entity::{MapEntities, VisitEntities, VisitEntitiesMut};
use bevy::ecs::reflect::{ReflectMapEntities, ReflectVisitEntities, ReflectVisitEntitiesMut};
use bevy::ecs::relationship::{Relationship, RelationshipTarget};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::reflect::GetTypeRegistration;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::fmt::{Debug, Formatter};

pub type ChildOfSync = RelationshipSync<ChildOf>;

// TODO: ideally this would not be needed, but Relationship are Immutable component
//  so we would have to update our whole replication/prediction/interpolation code to work on Immutable components
/// This component can be added to an entity to replicate the entity's hierarchy to the remote world.
/// The `ParentSync` component will be updated automatically when the `ChildOf` component changes,
/// and the entity's hierarchy will automatically be updated when the `ParentSync` component changes.
///
/// Updates entity's `ChildOf` component on change.
/// Removes the parent if `None`.
#[derive(Component, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct RelationshipSync<R: Relationship>{
    entity: Option<Entity>,
    #[reflect(ignore)]
    marker: std::marker::PhantomData<R>,
}

// We implement these traits manually because R might not have them
impl<R: Relationship> Default for RelationshipSync<R> {
    fn default() -> Self {
        Self {
            entity: None,
            marker: std::marker::PhantomData,
        }
    }
}

impl<R: Relationship> Clone for RelationshipSync<R> {
    fn clone(&self) -> Self {
        Self {
            entity: self.entity,
            marker: std::marker::PhantomData,
        }
    }
}

impl<R: Relationship> Copy for RelationshipSync<R> {}

impl<R: Relationship> PartialEq for RelationshipSync<R> {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl<R: Relationship> Debug for RelationshipSync<R> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "RelationshipSync {{ entity: {:?} }}", self.entity)
    }
}

impl<R: Relationship> From<Option<Entity>> for RelationshipSync<R> {
    fn from(value: Option<Entity>) -> Self {
        Self {
            entity: value,
            marker: std::marker::PhantomData,
        }
    }
}


impl<R: Relationship> MapEntities for RelationshipSync<R> {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        if let Some(entity) = &mut self.entity {
            *entity = entity_mapper.map_entity(*entity);
        }
    }
}


pub struct HierarchySendPlugin<R> {
    _marker: std::marker::PhantomData<R>,
}

impl<R: Relationship> Default for HierarchySendPlugin<R> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}



impl<R: Relationship> HierarchySendPlugin<R> {

    /// If the relationship changes (a new Relationship component was inserted)
    /// or if RelationshipSync is inserted,
    /// update the RelationshipSync to match the parent value
    fn handle_parent_insert(
        trigger: Trigger<OnAdd, (R, RelationshipSync<R>)>,
        // include filter to make sure that this is running on the send side
        mut query: Query<(&R, &mut RelationshipSync<R>), Or<(With<ReplicationMarker>, With<ReplicateLike>)>>,
    ) {
        if let Ok((parent, mut parent_sync)) = query.get_mut(trigger.target()) {
            parent_sync.set_if_neq(Some(parent.get()).into());
        }
    }

    /// Update RelationshipSync if the Relationship has been removed
    fn handle_parent_remove(
        trigger: Trigger<OnRemove, R>,
        // include filter to make sure that this is running on the send side
        mut hierarchy: Query<&mut RelationshipSync<R>, Or<(With<ReplicationMarker>, With<ReplicateLike>)>>,
    ) {
        if let Ok(mut parent_sync) = hierarchy.get_mut(trigger.target()) {
            parent_sync.entity = None;
        }
    }
}

impl<R: Relationship> Plugin for HierarchySendPlugin<R> {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::handle_parent_insert);
        app.add_observer(Self::handle_parent_remove);
    }
}

pub struct HierarchyReceivePlugin<P, R> {
    _marker: std::marker::PhantomData<(P, R)>,
}

impl<P, R> Default for HierarchyReceivePlugin<P, R> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<P: ReplicationPeer, R: Relationship + Debug> HierarchyReceivePlugin<P, R> {

    /// Update hierarchy on the receive side if RelationshipSync changed
    fn update_parent(
        mut commands: Commands,
        hierarchy: Query<
            (Entity, &RelationshipSync<R>, Option<&R>),
            (Changed<RelationshipSync<R>>, With<Replicated>),
        >,
    ) {
        for (entity, parent_sync, parent) in hierarchy.iter() {
            trace!(
                "update_parent: entity: {:?}, parent_sync: {:?}, parent: {:?}",
                entity,
                parent_sync,
                parent
            );
            if let Some(new_parent) = parent_sync.entity {
                if parent.is_none_or(|p| p.get() != new_parent) {
                    commands.entity(entity).insert(R::from(new_parent));
                }
            } else if parent.is_some() {
                commands.entity(entity).remove::<R>();
            }
        }
    }
}

impl<P: ReplicationPeer, R: Relationship + Debug + GetTypeRegistration + TypePath> Plugin for HierarchyReceivePlugin<P, R> {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<RelationshipSync<R>>();

        // TODO: does this work for client replication? (client replicating to other clients via the server?)
        // when we receive a RelationshipSync update from the remote, update the hierarchy
        app.add_systems(
            PreUpdate,
            Self::update_parent
                .after(InternalMainSet::<P::SetMarker>::Receive)
                // we want update_parent to run in the same frame that ParentSync is propagated
                // to the predicted/interpolated entities
                .after(PredictionSet::SpawnHistory)
                .after(InterpolationSet::SpawnHistory),
        );
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


// TODO: for simplicity we currently conflate replicating the hierarchy component (RelationshipSync<ChildOf>)
//  with propagating the replication components to children. Ideally we would be differentiating between
//  the 2, but in most cases they are the same (you want to propagate the replication components to the children
//  AND replicate the ChildOf relationship)
/// If `ReplicationMarker` is added on an entity that has `Children`
/// then we add `ReplicateLike(root)` on all the descendants.
///
/// Descendants that have `DisableReplicateHierarchy` will be skipped; i.e.
/// we won't add ReplicateLike on them or include `RelationshipSync<ChildOf>` on them, and
/// we won't iterate through their descendants.
///
/// If a child entity already has the `ReplicationMarker` component, we ignore it and its descendants.
pub(crate) fn propagate_replicate_like_replication_marker_added(
    // TODO: if ParentSync is added, should we propagate it?
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
    child_filter: Query<(), (Without<DisableReplicateHierarchy>, Without<ReplicationMarker>)>,
    mut commands: Commands,
) {
    let root = trigger.target();
    // if `DisableReplicateHierarchy` is present, return early since we don't need to propagate `ReplicateLike`
    let Ok(has_parent_sync) = root_query.get(root) else { return };
    let children = children_query.get(root).unwrap();
    // we go through all the descendants (instead of just the children) so that the root is added
    // and we don't need to search for the root ancestor in the replication systems
    let mut stack = SmallVec::<[Entity; 8]>::new();
    stack.push(root);
    while let Some(parent) = stack.pop() {
        for child in children_query.relationship_sources(parent) {
            if let Ok(()) = child_filter.get(child) {
                // TODO: should we buffer those inside a SmallVec for batch insert?
                // we also insert RelationshipSync on the descendants
                dbg!(&child);
                commands.entity(child).insert(
                    (ReplicateLike(root), ChildOfSync::from(Some(parent)))
                );
                stack.push(child);
            }
        }
    }
}

/// If `ReplicationMarker` is removed on an entity that has `Children`
/// then we remove `ReplicateLike(Entity)` on all the descendants.
///
/// Note that this doesn't happen if the `DisableReplicateHierarchy` is present.
///
/// If a child entity already has the `ReplicationMarker` component, we ignore it and its descendants.
pub(crate) fn propagate_replicate_like_replication_marker_removed(
    trigger: Trigger<OnRemove, ReplicationMarker>,
    root_query: Query<
        (),
        (
            With<Children>,
            Without<DisableReplicateHierarchy>,
            With<ReplicationMarker>,
        ),
    >,
    children_query: Query<&Children>,
    // exclude those that have `ReplicationMarker` (as we don't want to remove the `ReplicateLike` component
    // for their descendants)
    child_filter: Query<(), Without<ReplicationMarker>>,
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
                commands.entity(child).remove::<(ReplicateLike, ChildOfSync)>();
            }
        }
    }

}

/// If `ReplicateLike` is added on an entity that has `ReplicationMarker` (i.e. has the replication components)
/// then we add `ReplicateLike(root)` on all the descendants.
///
/// Note that this doesn't happen if the `DisableReplicateHierarchy` is present.
///
/// If a child entity already has the `ReplicationMarker` component, we ignore it and its descendants.
pub(crate) fn propagate_replicate_like_children_updated(
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




#[cfg(test)]
mod tests {
    use super::*;
    use crate::prelude::server::Replicate;
    use crate::prelude::{ClientConnectionManager, ClientId, ClientReplicate, NetworkTarget};
    use crate::shared::replication::components::ReplicationGroupId;
    use crate::tests::multi_stepper::{MultiBevyStepper, TEST_CLIENT_ID_1, TEST_CLIENT_ID_2};
    use crate::tests::protocol::*;
    use crate::tests::stepper::BevyStepper;
    use bevy::prelude::Entity;

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


    #[test]
    fn test_update_parent() {
        let (mut stepper, grandparent, parent, child) = setup_hierarchy();

        let replicate = Replicate {
            ..default()
        };
        // disable propagation to the child, so the child won't have ReplicateLike or RelationshipSync
        stepper
            .server_app
            .world_mut()
            .entity_mut(child)
            .insert(DisableReplicateHierarchy);
        // add Replicate, which should propagate the RelationshipSync and ReplicateLike through the hierarchy
        stepper
            .server_app
            .world_mut()
            .entity_mut(grandparent)
            .insert(replicate.clone());
        stepper.frame_step();
        stepper.frame_step();

        // check that the parent got replicated, along with the hierarchy information
        let client_grandparent = stepper
            .client_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentSyncModeFull>>()
            .get_single(stepper.client_app.world())
            .unwrap();
        let (client_parent, client_parent_sync, client_parent_component) = stepper
            .client_app
            .world_mut()
            .query_filtered::<(Entity, &ChildOfSync, &ChildOf), With<ComponentSyncModeSimple>>()
            .get_single(stepper.client_app.world())
            .unwrap();

        assert_eq!(client_parent_sync.entity, Some(client_grandparent));
        assert_eq!(client_parent_component.get(), client_grandparent);

        // check that the child did not get replicated
        assert!(stepper.server_app.world().get::<ChildOfSync>(child).is_none());
        assert!(stepper.server_app.world().get::<ReplicateLike>(child).is_none());

        // remove the hierarchy on the sender side
        stepper
            .server_app
            .world_mut()
            .entity_mut(parent)
            .remove::<ChildOf>();
        let replicate_like = stepper
            .server_app
            .world_mut()
            .get::<ReplicateLike>(parent);
        stepper.frame_step();
        stepper.frame_step();
        // 1. make sure that parent sync has been updated on the sender side
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .entity_mut(parent)
                .get::<ChildOfSync>(),
            Some(&ChildOfSync::from(None))
        );

        // 2. make sure that the parent has been removed on the receiver side, and that ParentSync has been updated
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_parent)
                .get::<ChildOfSync>(),
            Some(&ChildOfSync::from(None))
        );
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_parent)
                .get::<ChildOf>(),
            None,
        );
        assert!(stepper
            .client_app
            .world_mut()
            .entity_mut(client_grandparent)
            .get::<Children>()
            .is_none());
    }

    #[test]
    fn test_propagate_hierarchy() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let (mut stepper, grandparent, parent, child) = setup_hierarchy();

        stepper
            .server_app
            .world_mut()
            .entity_mut(grandparent)
            .insert(Replicate::default());

        stepper.frame_step();
        stepper.frame_step();

        let a = stepper.server_app.world().get::<ReplicateLike>(parent);
        let b = stepper.server_app.world().get::<ChildOfSync>(parent);
        dbg!(a, b);
        let a = stepper.server_app.world().get::<ReplicateLike>(child);
        let b = stepper.server_app.world().get::<ChildOfSync>(child);
        dbg!(a, b);

        // 1. check that the parent and child have been replicated
        let client_grandparent = stepper
            .client_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentSyncModeFull>>()
            .get_single(stepper.client_app.world())
            .unwrap();
        let client_parent = stepper
            .client_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentSyncModeSimple>>()
            .get_single(stepper.client_app.world())
            .unwrap();
        let client_child = stepper
            .client_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentSyncModeOnce>>()
            .get_single(stepper.client_app.world())
            .unwrap();

        // 2. check that the hierarchies have been replicated
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_parent)
                .get::<ChildOf>()
                .unwrap()
                .get(),
            client_grandparent
        );
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_child)
                .get::<ChildOf>()
                .unwrap()
                .get(),
            client_parent
        );

        // 3. check that the replication group has been set correctly
        // (all 3 entities have been sent in the same group)
        let group_id = ReplicationGroupId(grandparent.to_bits());
            assert_eq!(stepper
                .client_app
                .world()
                .resource::<ClientConnectionManager>()
                .replication_receiver
                .group_channels
                .get(&group_id)
                .unwrap()
                .local_entities
                .len(), 3);
    }

    #[test]
    fn test_propagate_hierarchy_client_to_server() {
        let mut stepper = BevyStepper::default();
        let child = stepper
            .client_app
            .world_mut()
            .spawn(ComponentClientToServer(0.0))
            .id();
        let parent = stepper
            .client_app
            .world_mut()
            .spawn((ComponentSyncModeFull(0.0), ClientReplicate::default()))
            .add_child(child)
            .id();

        for _ in 0..10 {
            stepper.frame_step();
        }

        // check that both the parent and the child were replicated
        let server_parent = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentSyncModeFull>>()
            .get_single(stepper.server_app.world())
            .expect("parent entity was not replicated");
        let server_child = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentClientToServer>>()
            .get_single(stepper.server_app.world())
            .expect("child entity was not replicated");
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ChildOf>(server_child)
                .unwrap()
                .get(),
            server_parent
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ChildOfSync>(server_child)
                .unwrap(),
            &ChildOfSync::from(Some(server_parent))
        );
    }


    /// https://github.com/cBournhonesque/lightyear/issues/649
    /// P1 with child C1
    /// If you add a new client to the replication target of P1, then both
    /// P1 and C1 should be replicated to the new client.
    /// (the issue says that only P1 was replicated)
    #[test]
    fn test_new_client_is_added_to_parent() {
        let mut stepper = MultiBevyStepper::default();

        let c1 = ClientId::Netcode(TEST_CLIENT_ID_1);
        let c2 = ClientId::Netcode(TEST_CLIENT_ID_2);

        let server_child = stepper.server_app.world_mut().spawn_empty().id();
        let server_parent = stepper
            .server_app
            .world_mut()
            .spawn(Replicate {
                target: ReplicationTarget {
                    target: NetworkTarget::Single(c1),
                },
                ..default()
            })
            .add_child(server_child)
            .id();

        stepper.frame_step();
        stepper.frame_step();

        let c1_child = stepper
            .client_app_1
            .world()
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_child)
            .expect("child entity was not replicated to client 1");
        let c1_parent = stepper
            .client_app_1
            .world()
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_parent)
            .expect("parent entity was not replicated to client 1");

        // change the replication target to include a new client
        stepper
            .server_app
            .world_mut()
            .get_mut::<ReplicationTarget>(server_parent)
            .unwrap()
            .target = NetworkTarget::Only(vec![c1, c2]);
        stepper.frame_step();
        stepper.frame_step();

        // check that both parent and child were replicated to the new client
        let c2_child = stepper
            .client_app_2
            .world()
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_child)
            .expect("child entity was not replicated to client 2");
        let c2_parent = stepper
            .client_app_2
            .world()
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_parent)
            .expect("parent entity was not replicated to client 2");
    }


    /// https://github.com/cBournhonesque/lightyear/issues/547
    /// Test that when a new child is added to a parent
    /// the child is also replicated to the remote
    #[test]
    fn test_propagate_hierarchy_new_child() {
        let mut stepper = BevyStepper::default();
        let server_parent = stepper
            .server_app
            .world_mut()
            .spawn(Replicate::default())
            .id();
        stepper.frame_step();
        stepper.frame_step();
        let client_parent = stepper
            .client_app
            .world()
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_parent)
            .expect("parent entity was not replicated to client");

        // add a child to the entity
        let server_child = stepper.server_app.world_mut().spawn_empty().id();
        stepper
            .server_app
            .world_mut()
            .entity_mut(server_parent)
            .add_child(server_child);
        dbg!(&server_parent, &server_child);
        stepper.frame_step();
        let b = stepper.server_app.world().get::<ReplicateLike>(server_child);
        dbg!(b);
        stepper.frame_step();

        // check that Replicate was propagated to the child, and that the child
        // was replicated to the client
        let client_child = stepper
            .client_app
            .world()
            .resource::<ClientConnectionManager>()
            .replication_receiver
            .remote_entity_map
            .get_local(server_child)
            .expect("child entity was not replicated to client");
    }
}
