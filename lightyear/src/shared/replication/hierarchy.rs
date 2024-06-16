//! This module is responsible for making sure that parent-children hierarchies are replicated correctly.
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prelude::server::ControlledBy;
use crate::prelude::{Replicated, Replicating, ReplicationGroup, VisibilityMode};
use crate::server::replication::send::SyncTarget;
use crate::shared::replication::components::{ReplicateHierarchy, ReplicationTarget};
use crate::shared::replication::{ReplicationPeer, ReplicationSend};
use crate::shared::sets::InternalReplicationSet;

/// This component can be added to an entity to replicate the entity's hierarchy to the remote world.
/// The `ParentSync` component will be updated automatically when the `Parent` component changes,
/// and the entity's hierarchy will automatically be updated when the `ParentSync` component changes.
///
/// Updates entity's `Parent` component on change.
/// Removes the parent if `None`.
#[derive(Component, Default, Reflect, Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
#[component(storage = "SparseSet")]
pub struct ParentSync(Option<Entity>);

impl MapEntities for ParentSync {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        if let Some(entity) = &mut self.0 {
            *entity = entity_mapper.map_entity(*entity);
        }
    }
}

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

impl<R: ReplicationSend> HierarchySendPlugin<R> {
    /// If `replicate.replicate_hierarchy` is true, replicate the entire hierarchy of the entity
    fn propagate_replicate(
        mut commands: Commands,
        // query the root parent of the hierarchy
        parent_query: Query<
            (
                Entity,
                Ref<ReplicateHierarchy>,
                &ReplicationTarget,
                Option<&SyncTarget>,
                Option<&ControlledBy>,
                Option<&VisibilityMode>,
            ),
            (Without<Parent>, With<Children>),
        >,
        children_query: Query<&Children>,
    ) {
        for (
            parent_entity,
            replicate_hierarchy,
            replication_target,
            sync_target,
            controlled_by,
            visibility_mode,
        ) in parent_query.iter()
        {
            if replicate_hierarchy.is_changed() && replicate_hierarchy.recursive {
                // iterate through all descendents of the entity
                for child in children_query.iter_descendants(parent_entity) {
                    trace!("Propagate Replicate through hierarchy: adding Replicate on child: {child:?}");
                    // no need to set the correct parent as it will be set later in the `update_parent_sync` system
                    commands.entity(child).insert((
                        // TODO: should we add replicating?
                        Replicating,
                        replication_target.clone(),
                        // the entire hierarchy is replicated as a single group, that uses the parent's entity as the group id
                        ReplicationGroup::new_id(parent_entity.to_bits()),
                        ReplicateHierarchy { recursive: true },
                        ParentSync(None),
                    ));
                    if let Some(controlled_by) = controlled_by {
                        commands.entity(child).insert(controlled_by.clone());
                    }
                    if let Some(sync_target) = sync_target {
                        commands.entity(child).insert(sync_target.clone());
                    }
                    if let Some(vis) = visibility_mode {
                        commands.entity(child).insert(*vis);
                    }
                }
            }
            // TODO: should we update the parent's replication group? we actually can't.. replication groups
            //  aren't supposed to be updated
        }
    }

    /// Update ParentSync if the hierarchy changed
    /// (run this in post-update before replicating, to account for any hierarchy changed initiated by the user)
    ///
    /// This only runs on the sending side
    fn update_parent_sync(
        mut query: Query<(Ref<Parent>, &mut ParentSync), With<ReplicateHierarchy>>,
    ) {
        for (parent, mut parent_sync) in query.iter_mut() {
            if parent.is_changed() || parent_sync.is_added() {
                trace!(
                    ?parent,
                    ?parent_sync,
                    "Update parent sync because hierarchy has changed"
                );
                parent_sync.set_if_neq(ParentSync(Some(**parent)));
            }
        }
    }

    /// Update ParentSync if the parent has been removed
    ///
    /// This only runs on the sending side
    fn removal_system(
        mut removed_parents: RemovedComponents<Parent>,
        mut hierarchy: Query<&mut ParentSync, With<ReplicateHierarchy>>,
    ) {
        for entity in removed_parents.read() {
            if let Ok(mut parent_sync) = hierarchy.get_mut(entity) {
                parent_sync.0 = None;
            }
        }
    }
}

impl<R: ReplicationSend> Plugin for HierarchySendPlugin<R> {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            (
                (Self::propagate_replicate, Self::update_parent_sync).chain(),
                Self::removal_system,
            )
                // we don't need to run these every frame, only every send_interval
                .in_set(InternalReplicationSet::<R::SetMarker>::SendMessages)
                // run before the replication-send systems
                .before(InternalReplicationSet::<R::SetMarker>::All),
        );
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
    /// On the receiving side, update the hierarchy if ParentSync was changed
    ///
    /// We implement this as an observer because this should be rare, and we don't
    /// want to run a system every frame to check for changes in ParentSync.
    fn on_insert_parent_sync(
        trigger: Trigger<OnInsert, ParentSync>,
        mut commands: Commands,
        hierarchy: Query<(&ParentSync, Option<&Parent>), Without<ReplicationTarget>>,
    ) {
        let entity = trigger.entity();
        dbg!("Received ParentSync");
        if let Ok((parent_sync, parent)) = hierarchy.get(trigger.entity()) {
            trace!(
                "update_parent: entity: {:?}, parent_sync: {:?}, parent: {:?}",
                entity,
                parent_sync,
                parent
            );
            if let Some(new_parent) = parent_sync.0 {
                if parent.filter(|&parent| **parent == new_parent).is_none() {
                    commands.entity(entity).set_parent(new_parent);
                }
            } else if parent.is_some() {
                commands.entity(entity).remove_parent();
            }
        }
    }
}

impl<R: ReplicationPeer> Plugin for HierarchyReceivePlugin<R> {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<ParentSync>();

        // TODO: does this work for client replication? (client replicating to other clients via the server?)
        app.observe(Self::on_insert_parent_sync);
        app.world_mut().spawn(ParentSync { 0: None });
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use bevy::hierarchy::{BuildWorldChildren, Children, Parent};
    use bevy::prelude::{default, Entity, With};

    use crate::prelude::server::Replicate;
    use crate::prelude::ReplicationGroup;
    use crate::shared::replication::components::ReplicateHierarchy;
    use crate::shared::replication::hierarchy::ParentSync;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    fn setup_hierarchy() -> (BevyStepper, Entity, Entity, Entity) {
        let mut stepper = BevyStepper::default();
        let child = stepper.server_app.world_mut().spawn(Component3(0.0)).id();
        let parent = stepper
            .server_app
            .world_mut()
            .spawn(Component2(0.0))
            .add_child(child)
            .id();
        let grandparent = stepper
            .server_app
            .world_mut()
            .spawn(Component1(0.0))
            .add_child(parent)
            .id();
        (stepper, grandparent, parent, child)
    }

    #[test]
    fn test_update_parent() {
        let (mut stepper, grandparent, parent, child) = setup_hierarchy();

        let replicate = Replicate {
            hierarchy: ReplicateHierarchy { recursive: false },
            // make sure that child and parent are replicated in the same group, so that both entities are spawned
            // before entity mapping is done
            group: ReplicationGroup::new_id(0),
            ..default()
        };
        stepper
            .server_app
            .world_mut()
            .entity_mut(parent)
            .insert((replicate.clone(), ParentSync::default()));
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
            .query_filtered::<Entity, With<Component1>>()
            .get_single(stepper.client_app.world())
            .unwrap();
        let (client_parent, client_parent_sync, client_parent_component) = stepper
            .client_app
            .world_mut()
            .query_filtered::<(Entity, &ParentSync, &Parent), With<Component2>>()
            .get_single(stepper.client_app.world())
            .unwrap();

        assert_eq!(client_parent_sync.0, Some(client_grandparent));
        assert_eq!(*client_parent_component.deref(), client_grandparent);

        // remove the hierarchy on the sender side
        stepper
            .server_app
            .world_mut()
            .entity_mut(parent)
            .remove_parent();
        stepper.frame_step();
        stepper.frame_step();
        // 1. make sure that parent sync has been updated on the sender side
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .entity_mut(parent)
                .get::<ParentSync>(),
            Some(&ParentSync(None))
        );

        // 2. make sure that the parent has been removed on the receiver side, and that ParentSync has been updated
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_parent)
                .get::<ParentSync>(),
            Some(&ParentSync(None))
        );
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_parent)
                .get::<Parent>(),
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

        // 1. check that the parent and child have been replicated
        let client_grandparent = stepper
            .client_app
            .world_mut()
            .query_filtered::<Entity, With<Component1>>()
            .get_single(stepper.client_app.world())
            .unwrap();
        let client_parent = stepper
            .client_app
            .world_mut()
            .query_filtered::<Entity, With<Component2>>()
            .get_single(stepper.client_app.world())
            .unwrap();
        let client_child = stepper
            .client_app
            .world_mut()
            .query_filtered::<Entity, With<Component3>>()
            .get_single(stepper.client_app.world())
            .unwrap();

        // 2. check that the hierarchies have been replicated
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_parent)
                .get::<Parent>()
                .unwrap()
                .deref(),
            &client_grandparent
        );
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .entity_mut(client_child)
                .get::<Parent>()
                .unwrap()
                .deref(),
            &client_parent
        );

        // 3. check that the replication group has been set correctly
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .entity_mut(parent)
                .get::<ReplicationGroup>(),
            Some(&ReplicationGroup::new_id(grandparent.to_bits()))
        );
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .entity_mut(child)
                .get::<ReplicationGroup>(),
            Some(&ReplicationGroup::new_id(grandparent.to_bits()))
        );
    }
}
