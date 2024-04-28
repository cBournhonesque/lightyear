//! This module is responsible for making sure that parent-children hierarchies are replicated correctly.
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prelude::ReplicationGroup;
use crate::shared::replication::components::Replicate;
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

/// This component can be added to an entity to replicate the entity's hierarchy to the remote world.
/// The `ParentSync` component will be updated automatically when the `Parent` component changes,
/// and the entity's hierarchy will automatically be updated when the `ParentSync` component changes.
///
/// Updates entity's `Parent` component on change.
/// Removes the parent if `None`.
#[derive(Component, Default, Reflect, Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
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
        parent_query: Query<(Entity, Ref<Replicate>), (Without<Parent>, With<Children>)>,
        children_query: Query<&Children>,
    ) {
        for (parent_entity, replicate) in parent_query.iter() {
            // TODO: we only want to do this if the `replicate_hierarchy` field has changed, not other fields!
            //  maybe use a different component?
            if replicate.is_changed() && replicate.replicate_hierarchy {
                // iterate through all descendents of the entity
                for child in children_query.iter_descendants(parent_entity) {
                    let mut replicate = replicate.clone();
                    // the entire hierarchy is replicated as a single group, that uses the parent's entity as the group id
                    replicate.replication_group = ReplicationGroup::new_id(parent_entity.to_bits());
                    // no need to set the correct parent as it will be set later in the `update_parent_sync` system
                    commands.entity(child).insert((replicate, ParentSync(None)));
                }
            }
        }
    }

    /// Update ParentSync if the hierarchy changed
    /// (run this in post-update before replicating, to account for any hierarchy changed initiated by the user)
    ///
    /// This only runs on the sending side
    fn update_parent_sync(mut query: Query<(Ref<Parent>, &mut ParentSync), With<Replicate>>) {
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
        mut hierarchy: Query<&mut ParentSync, With<Replicate>>,
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
                .in_set(InternalMainSet::<R::SetMarker>::Send)
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

impl<R: ReplicationSend> HierarchyReceivePlugin<R> {
    /// Update parent/children hierarchy if parent_sync changed
    ///
    /// This only runs on the receiving side
    fn update_parent(
        mut commands: Commands,
        hierarchy: Query<
            (Entity, &ParentSync, Option<&Parent>),
            (Changed<ParentSync>, Without<Replicate>),
        >,
    ) {
        for (entity, parent_sync, parent) in hierarchy.iter() {
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

impl<R: ReplicationSend> Plugin for HierarchyReceivePlugin<R> {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<ParentSync>();

        // TODO: does this work for client replication? (client replicating to other clients via the server?)
        // when we receive a ParentSync update from the remote, update the hierarchy
        app.add_systems(
            PreUpdate,
            Self::update_parent.after(InternalMainSet::<R::SetMarker>::Receive),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use bevy::hierarchy::{BuildWorldChildren, Children, Parent};
    use bevy::prelude::{default, Entity, With};

    use crate::prelude::{Replicate, ReplicationGroup};
    use crate::shared::replication::hierarchy::ParentSync;
    use crate::tests::protocol::*;
    use crate::tests::stepper::{BevyStepper, Step};

    fn setup_hierarchy() -> (BevyStepper, Entity, Entity, Entity) {
        let mut stepper = BevyStepper::default();
        let child = stepper.server_app.world.spawn(Component3(0.0)).id();
        let parent = stepper
            .server_app
            .world
            .spawn(Component2(0.0))
            .add_child(child)
            .id();
        let grandparent = stepper
            .server_app
            .world
            .spawn(Component1(0.0))
            .add_child(parent)
            .id();
        (stepper, grandparent, parent, child)
    }

    #[test]
    fn test_update_parent() {
        let (mut stepper, grandparent, parent, child) = setup_hierarchy();

        let replicate = Replicate {
            replicate_hierarchy: false,
            // make sure that child and parent are replicated in the same group, so that both entities are spawned
            // before entity mapping is done
            replication_group: ReplicationGroup::new_id(0),
            ..default()
        };
        stepper
            .server_app
            .world
            .entity_mut(parent)
            .insert((replicate.clone(), ParentSync::default()));
        stepper
            .server_app
            .world
            .entity_mut(grandparent)
            .insert(replicate.clone());
        stepper.frame_step();
        stepper.frame_step();

        // check that the parent got replicated, along with the hierarchy information
        let client_grandparent = stepper
            .client_app
            .world
            .query_filtered::<Entity, With<Component1>>()
            .get_single(&stepper.client_app.world)
            .unwrap();
        let (client_parent, client_parent_sync, client_parent_component) = stepper
            .client_app
            .world
            .query_filtered::<(Entity, &ParentSync, &Parent), With<Component2>>()
            .get_single(&stepper.client_app.world)
            .unwrap();

        assert_eq!(client_parent_sync.0, Some(client_grandparent));
        assert_eq!(*client_parent_component.deref(), client_grandparent);

        // remove the hierarchy on the sender side
        stepper.server_app.world.entity_mut(parent).remove_parent();
        stepper.frame_step();
        stepper.frame_step();
        // 1. make sure that parent sync has been updated on the sender side
        assert_eq!(
            stepper
                .server_app
                .world
                .entity_mut(parent)
                .get::<ParentSync>(),
            Some(&ParentSync(None))
        );

        // 2. make sure that the parent has been removed on the receiver side, and that ParentSync has been updated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity_mut(client_parent)
                .get::<ParentSync>(),
            Some(&ParentSync(None))
        );
        assert_eq!(
            stepper
                .client_app
                .world
                .entity_mut(client_parent)
                .get::<Parent>(),
            None,
        );
        assert!(stepper
            .client_app
            .world
            .entity_mut(client_grandparent)
            .get::<Children>()
            .is_none());
    }

    #[test]
    fn test_propagate_hierarchy() {
        let (mut stepper, grandparent, parent, child) = setup_hierarchy();

        stepper
            .server_app
            .world
            .entity_mut(grandparent)
            .insert(Replicate::default());

        stepper.frame_step();
        stepper.frame_step();

        // 1. check that the parent and child have been replicated
        let client_grandparent = stepper
            .client_app
            .world
            .query_filtered::<Entity, With<Component1>>()
            .get_single(&stepper.client_app.world)
            .unwrap();
        let client_parent = stepper
            .client_app
            .world
            .query_filtered::<Entity, With<Component2>>()
            .get_single(&stepper.client_app.world)
            .unwrap();
        let client_child = stepper
            .client_app
            .world
            .query_filtered::<Entity, With<Component3>>()
            .get_single(&stepper.client_app.world)
            .unwrap();

        // 2. check that the hierarchies have been replicated
        assert_eq!(
            stepper
                .client_app
                .world
                .entity_mut(client_parent)
                .get::<Parent>()
                .unwrap()
                .deref(),
            &client_grandparent
        );
        assert_eq!(
            stepper
                .client_app
                .world
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
                .world
                .entity_mut(client_parent)
                .get::<Replicate>(),
            Some(&Replicate {
                replication_group: ReplicationGroup::new_id(grandparent.to_bits()),
                ..Default::default()
            })
        );
        assert_eq!(
            stepper
                .server_app
                .world
                .entity_mut(client_child)
                .get::<Replicate>(),
            Some(&Replicate {
                replication_group: ReplicationGroup::new_id(grandparent.to_bits()),
                ..Default::default()
            })
        );
    }
}
