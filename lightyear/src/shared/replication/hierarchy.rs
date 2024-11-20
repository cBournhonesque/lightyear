//! This module is responsible for making sure that parent-children hierarchies are replicated correctly.
use crate::client::replication::send::ReplicateToServer;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::prelude::server::ControlledBy;
use crate::prelude::{
    MainSet, NetworkRelevanceMode, PrePredicted, Replicated, Replicating, ReplicationGroup,
};
use crate::server::replication::send::SyncTarget;
use crate::shared::replication::authority::{AuthorityPeer, HasAuthority};
use crate::shared::replication::components::{ReplicateHierarchy, ReplicationTarget};
use crate::shared::replication::{ReplicationPeer, ReplicationSend};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

/// This component can be added to an entity to replicate the entity's hierarchy to the remote world.
/// The `ParentSync` component will be updated automatically when the `Parent` component changes,
/// and the entity's hierarchy will automatically be updated when the `ParentSync` component changes.
///
/// Updates entity's `Parent` component on change.
/// Removes the parent if `None`.
#[derive(Component, Default, Reflect, Clone, Copy, Serialize, Deserialize, Debug, PartialEq)]
#[reflect(Component)]
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
                &ReplicationGroup,
                Ref<ReplicateHierarchy>,
                Option<&PrePredicted>,
                Option<&ReplicationTarget>,
                Option<&ReplicateToServer>,
                Option<&SyncTarget>,
                Option<&ControlledBy>,
                Option<&NetworkRelevanceMode>,
                Option<&HasAuthority>,
                Option<&AuthorityPeer>,
                Has<Replicated>,
            ),
            (
                Without<Parent>,
                With<Children>,
                // TODO also handle when a component is removed, it should be removed for children
                //   maybe do all this via observers?
                Or<(
                    Changed<Children>,
                    Changed<ReplicateHierarchy>,
                    Changed<ReplicationTarget>,
                    Changed<SyncTarget>,
                    Changed<ControlledBy>,
                    Changed<NetworkRelevanceMode>,
                    Changed<AuthorityPeer>,
                )>,
            ),
        >,
        children_query: Query<&Children>,
        child_query: Query<(), (With<ParentSync>, With<Replicating>)>,
    ) {
        for (
            parent_entity,
            parent_group,
            replicate_hierarchy,
            pre_predicted,
            replication_target,
            replicate_to_server,
            sync_target,
            controlled_by,
            visibility_mode,
            has_authority,
            authority_peer,
            is_replicated,
        ) in parent_query.iter()
        {
            if replicate_hierarchy.recursive {
                // iterate through all descendents of the entity
                for child in children_query.iter_descendants(parent_entity) {
                    // TODO: or do we want to propagate any change of any component to the children?
                    // if the child already has ParentSync and Replicating, we don't need to add it again
                    if child_query.get(child).is_ok() {
                        continue;
                    }
                    trace!("Propagate Replicate through hierarchy: adding Replicate on child: {child:?}");
                    let Some(mut child_commands) = commands.get_entity(child) else {
                        continue;
                    };
                    // no need to set the correct parent as it will be set later in the `update_parent_sync` system
                    child_commands.insert((
                        // TODO: should we add replicating?
                        Replicating,
                        // the entire hierarchy is replicated as a single group so we re-use the parent's replication group id
                        parent_group
                            .clone()
                            .set_id(parent_group.group_id(Some(parent_entity)).0),
                        ReplicateHierarchy { recursive: true },
                        ParentSync(None),
                    ));
                    // On the client, we want to add the PrePredicted component to the children
                    // the PrePredicted observer will spawn a corresponding Confirmed entity.
                    //
                    // On the server, we just send the PrePredicted component as is to the client,
                    // (we don't want to overwrite the PrePredicted component on the server)
                    if let Some(pre_predicted) = pre_predicted {
                        // only insert on the child if we are on the client
                        if !is_replicated {
                            commands.entity(child).insert(PrePredicted::default());
                        }
                    }
                    if let Some(replication_target) = replication_target {
                        commands.entity(child).insert(replication_target.clone());
                    }
                    if let Some(replicate_to_server) = replicate_to_server {
                        commands.entity(child).insert(*replicate_to_server);
                    }
                    if let Some(controlled_by) = controlled_by {
                        commands.entity(child).insert(controlled_by.clone());
                    }
                    if let Some(sync_target) = sync_target {
                        commands.entity(child).insert(sync_target.clone());
                    }
                    if let Some(vis) = visibility_mode {
                        commands.entity(child).insert(*vis);
                    }
                    if let Some(has_authority) = has_authority {
                        debug!(
                            "Adding HasAuthority on child: {child:?} (parent: {parent_entity:?})"
                        );
                        commands.entity(child).insert(*has_authority);
                    }
                    if let Some(authority_peer) = authority_peer {
                        commands.entity(child).insert(*authority_peer);
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
    fn handle_parent_remove(
        trigger: Trigger<OnRemove, Parent>,
        mut hierarchy: Query<&mut ParentSync, With<ReplicateHierarchy>>,
    ) {
        if let Ok(mut parent_sync) = hierarchy.get_mut(trigger.entity()) {
            parent_sync.0 = None;
        }
    }
}

impl<R: ReplicationSend> Plugin for HierarchySendPlugin<R> {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::handle_parent_remove);
        app.add_systems(
            PostUpdate,
            (Self::propagate_replicate, Self::update_parent_sync)
                .chain()
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
    /// Update parent/children hierarchy if parent_sync changed
    ///
    /// This only runs on the receiving side
    fn update_parent(
        mut commands: Commands,
        hierarchy: Query<
            (Entity, &ParentSync, Option<&Parent>),
            (Changed<ParentSync>, Without<ReplicationTarget>),
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

impl<R: ReplicationPeer> Plugin for HierarchyReceivePlugin<R> {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<ParentSync>();

        // TODO: does this work for client replication? (client replicating to other clients via the server?)
        // when we receive a ParentSync update from the remote, update the hierarchy
        app.add_systems(
            PreUpdate,
            Self::update_parent
                .after(InternalMainSet::<R::SetMarker>::Receive)
                // NOTE: we're putting this in MainSet::Receive so that users can order
                // their systems after this
                .in_set(MainSet::Receive),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Deref;

    use bevy::hierarchy::{BuildChildren, Children, Parent};
    use bevy::prelude::{default, Entity, With};

    use crate::prelude::client;
    use crate::prelude::server::Replicate;
    use crate::prelude::ReplicationGroup;
    use crate::shared::replication::components::ReplicateHierarchy;
    use crate::shared::replication::hierarchy::ParentSync;
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
            .query_filtered::<Entity, With<ComponentSyncModeFull>>()
            .get_single(stepper.client_app.world())
            .unwrap();
        let (client_parent, client_parent_sync, client_parent_component) = stepper
            .client_app
            .world_mut()
            .query_filtered::<(Entity, &ParentSync, &Parent), With<ComponentSyncModeSimple>>()
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

    #[test]
    fn test_propagate_hierarchy_client_to_server() {
        let mut stepper = BevyStepper::default();
        let child = stepper
            .client_app
            .world_mut()
            .spawn(ComponentSyncModeOnce(0.0))
            .id();
        let parent = stepper
            .client_app
            .world_mut()
            .spawn((ComponentSyncModeSimple(0.0), client::Replicate::default()))
            .add_child(child)
            .id();

        for _ in 0..10 {
            stepper.frame_step();
        }

        // check that both the parent and the child were replicated
        let server_parent = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentSyncModeSimple>>()
            .get_single(stepper.server_app.world())
            .expect("parent entity was not replicated");
        let server_child = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentSyncModeOnce>>()
            .get_single(stepper.server_app.world())
            .expect("child entity was not replicated");
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<Parent>(server_child)
                .unwrap()
                .get(),
            server_parent
        );
        assert_eq!(
            stepper
                .server_app
                .world()
                .get::<ParentSync>(server_child)
                .unwrap(),
            &ParentSync(Some(server_parent))
        );
    }

    #[test]
    fn test_remove_child() {
        let mut stepper = BevyStepper::default();
        let child = stepper
            .client_app
            .world_mut()
            .spawn(ComponentSyncModeFull(0.0))
            .id();
        let parent = stepper
            .client_app
            .world_mut()
            .spawn((ComponentSyncModeSimple(0.0), client::Replicate::default()))
            .add_child(child)
            .id();
        stepper
            .client_app
            .world_mut()
            .commands()
            .entity(child)
            .despawn();

        for _ in 0..10 {
            stepper.frame_step();
        }

        // check that child was removed
        let server_child = stepper
            .server_app
            .world_mut()
            .query_filtered::<Entity, With<ComponentSyncModeFull>>()
            .get_single(stepper.server_app.world());
        assert!(server_child.is_err());
    }
}
