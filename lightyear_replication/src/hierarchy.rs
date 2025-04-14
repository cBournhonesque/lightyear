//! This module is responsible for making sure that parent-children hierarchies are replicated correctly.

use crate::components::{DisableReplicateHierarchy, ReplicationMarker};
use crate::plugin::ReplicationSet;
use crate::prelude::PrePredicted;
use crate::send::ReplicationBufferSet;
use bevy::ecs::entity::MapEntities;
use bevy::ecs::reflect::ReflectMapEntities;
use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use bevy::reflect::GetTypeRegistration;
use core::fmt::{Debug, Formatter};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use tracing::trace;

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
pub struct RelationshipSync<R: Relationship> {
    entity: Option<Entity>,
    #[reflect(ignore)]
    marker: core::marker::PhantomData<R>,
}

// We implement these traits manually because R might not have them
impl<R: Relationship> Default for RelationshipSync<R> {
    fn default() -> Self {
        Self {
            entity: None,
            marker: core::marker::PhantomData,
        }
    }
}

impl<R: Relationship> Clone for RelationshipSync<R> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<R: Relationship> Copy for RelationshipSync<R> {}

impl<R: Relationship> PartialEq for RelationshipSync<R> {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity
    }
}

impl<R: Relationship> Debug for RelationshipSync<R> {
    fn fmt(&self, f: &mut Formatter<'_>) -> core::fmt::Result {
        write!(f, "RelationshipSync {{ entity: {:?} }}", self.entity)
    }
}

impl<R: Relationship> From<Option<Entity>> for RelationshipSync<R> {
    fn from(value: Option<Entity>) -> Self {
        Self {
            entity: value,
            marker: core::marker::PhantomData,
        }
    }
}

impl<R: Relationship> MapEntities for RelationshipSync<R> {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        if let Some(entity) = &mut self.entity {
            *entity = entity_mapper.get_mapped(*entity);
        }
    }
}

/// Plugin that lets you send replication updates for a given [`Relationship`] `R`
pub struct RelationshipSendPlugin<R> {
    _marker: core::marker::PhantomData<R>,
}

impl<R: Relationship> Default for RelationshipSendPlugin<R> {
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<R: Relationship> RelationshipSendPlugin<R> {
    /// If the relationship changes (a new Relationship component was inserted)
    /// or if RelationshipSync is inserted,
    /// update the RelationshipSync to match the parent value
    fn handle_parent_insert(
        trigger: Trigger<OnAdd, (R, RelationshipSync<R>)>,
        // include filter to make sure that this is running on the send side
        mut query: Query<
            (&R, &mut RelationshipSync<R>),
            Or<(With<ReplicationMarker>, With<ReplicateLike>)>,
        >,
    ) {
        if let Ok((parent, mut parent_sync)) = query.get_mut(trigger.target()) {
            parent_sync.set_if_neq(Some(parent.get()).into());
        }
    }

    /// Update RelationshipSync if the Relationship has been removed
    fn handle_parent_remove(
        trigger: Trigger<OnRemove, R>,
        // include filter to make sure that this is running on the send side
        mut hierarchy: Query<
            &mut RelationshipSync<R>,
            Or<(With<ReplicationMarker>, With<ReplicateLike>)>,
        >,
    ) {
        if let Ok(mut parent_sync) = hierarchy.get_mut(trigger.target()) {
            parent_sync.entity = None;
        }
    }
}

impl<R: Relationship> Plugin for RelationshipSendPlugin<R> {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::handle_parent_insert);
        app.add_observer(Self::handle_parent_remove);
    }
}

/// Plugin that lets you apply replication updates for a given [`Relationship`] `R`
pub struct RelationshipReceivePlugin<R> {
    _marker: core::marker::PhantomData<R>,
}

impl<R> Default for RelationshipReceivePlugin<R> {
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<R: Relationship + Debug> RelationshipReceivePlugin<R> {
    /// Update hierarchy on the receive side if RelationshipSync changed
    fn update_parent(
        mut commands: Commands,
        hierarchy: Query<
            (Entity, &RelationshipSync<R>, Option<&R>),
            // We add `Without<ReplicationMarker>` to guarantee that this is running for replicated entities.
            // With<Replicated> doesn't work because PrePredicted entities on the server side remove `Replicated`
            // via an observer. Maybe `With<InitialReplicated>` would work.
            (Changed<RelationshipSync<R>>, Without<ReplicationMarker>),
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

impl<R: Relationship + Debug + GetTypeRegistration + TypePath> Plugin
    for RelationshipReceivePlugin<R>
{
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<RelationshipSync<R>>();

        // TODO: does this work for client replication? (client replicating to other clients via the server?)
        // when we receive a RelationshipSync update from the remote, update the hierarchy
        app.add_systems(
            PreUpdate,
            Self::update_parent
                .after(ReplicationSet::Receive)
                // // we want update_parent to run in the same frame that ParentSync is propagated
                // // to the predicted/interpolated entities
                // .after(PredictionSet::Sync)
                // .after(InterpolationSet::SpawnHistory),
        );
    }
}

/// Marker component that indicates that this entity should be replicated similarly to the entity
/// contained in the component.
///
/// This will be inserted automatically
// TODO: should we make this immutable?
#[derive(Component, Clone, Copy, MapEntities, Reflect, PartialEq, Debug)]
#[reflect(
    Component,
    MapEntities,
    PartialEq,
    Debug
)]
pub struct ReplicateLike(pub(crate) Entity);

/// Plugin that helps lightyear propagate replication components through the ChildOf relationship.
///
/// The main idea is this:
/// - when `ReplicationMarker` is added, we will add a `ReplicateLike` component to all children
///   - we skip any child that has DisableReplicateHierarchy and its descendants
///   - we also skip any child that has `ReplicationMarker` and its descendants, because those children
///     will want to be replicated according to that child's replication components
/// - in the replication send system, either an entity has `ReplicationMarker` and we use its replication
///   components to determine how we do the sync. Or it could have the `ReplicateLike(root)` component and
///   we will use the `root` entity's replication components to determine how the replication will happen.
///   Any replication component (`OverrideTarget`, etc.) can be added on the child entity to override the
///   behaviour only for that child
/// - this is mainly useful for replicating visibility components through the hierarchy. Instead of having to
///   add all the child entities to a room, or propagating the `CachedNetworkRelevance` through the hierarchy,
///   the child entity can just use the root's `CachedNetworkRelevance` value
///
/// Note that currently propagating the replication components and propagating `ChildOfSync` (which helps you
/// replicate the `ChildOf` relationship) have the same logic. They use the same `DisableReplicateHierarchy` to
/// determine when to stop the propagation.
#[derive(Default)]
pub struct HierarchySendPlugin;


impl Plugin for HierarchySendPlugin {
    fn build(&self, app: &mut App) {
        // propagate ReplicateLike
        // app.add_observer(Self::propagate_replicate_like_children_updated);
        // app.add_observer(Self::propagate_replicate_like_replication_marker_added);
        app.add_observer(Self::propagate_replicate_like_replication_marker_removed);
        app.add_systems(
            PostUpdate,
            Self::propagate_through_hierarchy
                // update replication components before we actually run the Buffer systems
                .in_set(ReplicationBufferSet::BeforeBuffer)
        );
    }
}

impl HierarchySendPlugin {
    /// Propagate certain replication components through the hierarchy.
    /// - If new children are added, `ReplicationMarker` is added, `PrePredicted` is added, we recursively
    ///   go through the descendants and add `ReplicateLike`, `ChildOfSync`, ... if the child does not have
    ///   `DisableReplicateHierarchy` or `ReplicationMarker` already
    /// - We run this as a system and not an observer because observers cannot handle Children updates very well
    ///   (if we trigger on ChildOf being added, there is no flush between the ChildOf OnAdd hook and the observer
    ///   so the `&Children` query won't be updated (or the component will not exist on the parent yet)
    fn propagate_through_hierarchy(
        mut commands: Commands,
        root_query: Query<
            (Entity, Has<PrePredicted>),
            (
                With<ReplicationMarker>,
                Without<DisableReplicateHierarchy>,
                With<Children>,
                Or<(
                    Changed<Children>,
                    Added<PrePredicted>,
                    Added<ReplicationMarker>,
                )>,
            ),
        >,
        children_query: Query<&Children>,
        // exclude those that have `ReplicationMarker` (as we don't want to overwrite the `ReplicateLike` component
        // for their descendants, and we don't want to add `ReplicateLike` on them)
        child_filter: Query<
            (),
            (
                Without<DisableReplicateHierarchy>,
                Without<ReplicationMarker>,
            ),
        >,
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
                        commands
                            .entity(child)
                            .insert((ReplicateLike(root), ChildOfSync::from(Some(parent))));
                        if pre_predicted {
                            commands.entity(child).insert(PrePredicted::default());
                        }
                        stack.push(child);
                    }
                }
            }
        })
    }

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
        child_filter: Query<
            (),
            (
                Without<DisableReplicateHierarchy>,
                Without<ReplicationMarker>,
            ),
        >,
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
                    // TODO: should we buffer those inside a SmallVec for batch insert?
                    // we also insert RelationshipSync on the descendants
                    commands
                        .entity(child)
                        .insert((ReplicateLike(root), ChildOfSync::from(Some(parent))));
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
                    commands
                        .entity(child)
                        .remove::<(ReplicateLike, ChildOfSync)>();
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
}