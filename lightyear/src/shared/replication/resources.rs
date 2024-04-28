//! Module to handle the replication of bevy [`Resource`]s

use crate::_internal::ReplicationSend;
use crate::prelude::Message;
use crate::shared::replication::components::Replicate;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};
use async_compat::CompatExt;
use bevy::app::App;
use bevy::ecs::entity::MapEntities;
use bevy::ecs::system::Command;
use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, EntityMapper, IntoSystemConfigs,
    IntoSystemSetConfigs, Plugin, PostUpdate, PreUpdate, Query, Ref, Res, ResMut, Resource,
    SystemSet, With, World,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use tracing::error;

mod command {
    use super::*;

    pub struct StartReplicateCommand<R> {
        replicate: Replicate,
        _marker: PhantomData<R>,
    }

    impl<R: Resource + Clone> Command for StartReplicateCommand<R> {
        fn apply(self, world: &mut World) {
            if let Ok(entity) = world
                .query_filtered::<Entity, With<ReplicateResource<R>>>()
                .get_single(world)
            {
                world.entity_mut(entity).insert(self.replicate);
            } else {
                world.spawn((ReplicateResource::<R>::default(), self.replicate));
            }
        }
    }

    /// Extension trait to be able to replicate a resource to remote clients via [`Commands`].
    pub trait ReplicateResourceExt {
        /// Start replicating a resource to remote clients.
        ///
        /// Any change to the resource will be replicated to the clients.
        // TODO: we use `Replicate` as argument instead of the simpler `NetworkTarget`
        //  because it helps with type-inference when calling this method.
        //  We can switch to `NetworkTarget` if we remove the `P` bound of `Replicate`.
        fn replicate_resource<R: Resource + Clone>(&mut self, replicate: Replicate);
    }

    impl ReplicateResourceExt for Commands<'_, '_> {
        fn replicate_resource<R: Resource + Clone>(&mut self, replicate: Replicate) {
            self.add(StartReplicateCommand::<R> {
                replicate,
                _marker: PhantomData,
            });
        }
    }

    pub struct StopReplicateCommand<R> {
        _marker: PhantomData<R>,
    }

    impl<R> Default for StopReplicateCommand<R> {
        fn default() -> Self {
            Self {
                _marker: PhantomData,
            }
        }
    }

    impl<R: Resource + Clone> Command for StopReplicateCommand<R> {
        fn apply(self, world: &mut World) {
            if let Ok(entity) = world
                .query_filtered::<Entity, With<ReplicateResource<R>>>()
                .get_single(world)
            {
                // we do not despawn the entity, because that would delete the resource
                world.entity_mut(entity).remove::<Replicate>();
            }
        }
    }

    /// Extension trait to be able to stop replicating a resource to remote clients via [`Commands`].
    pub trait StopReplicateResourceExt {
        /// Stop replicating a resource to remote clients.
        fn stop_replicate_resource<R: Resource + Clone>(&mut self);
    }

    impl StopReplicateResourceExt for Commands<'_, '_> {
        fn stop_replicate_resource<R: Resource + Clone>(&mut self) {
            self.add(StopReplicateCommand::<R>::default());
        }
    }
}
use crate::protocol::BitSerializable;
pub use command::{ReplicateResourceExt, StopReplicateCommand, StopReplicateResourceExt};

/// This component can be added to an entity to start replicating a [`Resource`] to remote clients.
///
/// Currently, resources are cloned to be replicated, so only use this for resources that are
/// cheap-to-clone. (the clone only happens when the resource is modified)
///
/// Only one entity per World should have this component.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReplicateResource<R> {
    resource: Option<R>,
}

impl<R: MapEntities> MapEntities for ReplicateResource<R> {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        if let Some(r) = self.resource.as_mut() {
            r.map_entities(entity_mapper);
        }
    }
}

impl<R> Default for ReplicateResource<R> {
    fn default() -> Self {
        Self { resource: None }
    }
}

pub(crate) mod send {
    use super::*;
    pub(crate) struct ResourceSendPlugin<R> {
        _marker: PhantomData<R>,
    }

    impl<R> Default for ResourceSendPlugin<R> {
        fn default() -> Self {
            Self {
                _marker: PhantomData,
            }
        }
    }

    impl<R: ReplicationSend> Plugin for ResourceSendPlugin<R> {
        fn build(&self, app: &mut App) {
            app.configure_sets(
                PostUpdate,
                // we need to make sure that the resource data is copied to the component before
                // we send the component update
                InternalReplicationSet::<R::SetMarker>::SendResourceUpdates
                    .before(InternalReplicationSet::<R::SetMarker>::SendComponentUpdates),
            );
        }
    }

    pub(crate) fn add_resource_send_systems<S: ReplicationSend, R: Resource + Clone>(
        app: &mut App,
    ) {
        app.add_systems(
            PostUpdate,
            copy_send_resource::<R>
                .in_set(InternalReplicationSet::<S::SetMarker>::SendResourceUpdates),
        );
    }

    fn copy_send_resource<R: Resource + Clone>(
        resource: Option<Res<R>>,
        mut replicating_entity: Query<&mut ReplicateResource<R>, With<Replicate>>,
    ) {
        if replicating_entity.iter().len() > 1 {
            error!(
                "Only one entity per World should have a ReplicateResource<{:?}> component",
                std::any::type_name::<R>()
            );
            return;
        }
        let Some(resource) = resource else {
            // if the resource was removed, remove it from the entity
            if let Ok(mut replicating_entity) = replicating_entity.get_single_mut() {
                if replicating_entity.resource.is_some() {
                    replicating_entity.resource = None;
                }
            }
            return;
        };
        if let Ok(mut replicating_entity) = replicating_entity.get_single_mut() {
            if resource.is_changed() || replicating_entity.is_added() {
                // TODO: we should be able to avoid this clone? we only need the reference to the resource to serialize it
                //  - we could directly serialize the data here and store it in the component
                //  - the component could just be a marker that we need to serialize the resource, and then we have a custom
                //    serialization function that fetches the resource and serializes it?
                replicating_entity.resource = Some(resource.clone());
            }
        }
    }
}

pub(crate) mod receive {
    use super::*;
    use bevy::prelude::RemovedComponents;
    pub(crate) struct ResourceReceivePlugin<R> {
        _marker: PhantomData<R>,
    }

    impl<R> Default for ResourceReceivePlugin<R> {
        fn default() -> Self {
            Self {
                _marker: PhantomData,
            }
        }
    }

    impl<R: ReplicationSend> Plugin for ResourceReceivePlugin<R> {
        fn build(&self, app: &mut App) {
            app.configure_sets(
                PreUpdate,
                InternalReplicationSet::<R::SetMarker>::ReceiveResourceUpdates
                    .after(InternalMainSet::<R::SetMarker>::Receive),
            );
        }
    }

    pub(crate) fn add_resource_receive_systems<S: ReplicationSend, R: Resource + Clone>(
        app: &mut App,
    ) {
        app.add_systems(
            PreUpdate,
            (copy_receive_resource::<R>, handle_despawned_entity::<R>)
                .in_set(InternalReplicationSet::<S::SetMarker>::ReceiveResourceUpdates),
        );
    }

    fn copy_receive_resource<R: Resource + Clone>(
        mut commands: Commands,
        replicating_entity: Query<Ref<ReplicateResource<R>>>,
        resource: Option<ResMut<R>>,
    ) {
        if replicating_entity.iter().len() > 1 {
            error!(
                "Only one entity per World should have a ReplicateResource<{:?}> component",
                std::any::type_name::<R>()
            );
            return;
        }
        if let Ok(replicating_entity) = replicating_entity.get_single() {
            if replicating_entity.is_changed() {
                if let Some(received_value) = &replicating_entity.resource {
                    if let Some(mut resource) = resource {
                        *resource = received_value.clone();
                    } else {
                        commands.insert_resource(received_value.clone());
                    }
                } else if let Some(resource) = resource {
                    commands.remove_resource::<R>();
                }
            }
        }
    }

    /// If the entity that was driving the replication of the resource is despawned (usually when the
    /// client disconnects from the server), despawn the resource
    fn handle_despawned_entity<R: Resource + Clone>(
        mut commands: Commands,
        mut despawned: RemovedComponents<ReplicateResource<R>>,
    ) {
        for despawned_entity in despawned.read() {
            commands.remove_resource::<R>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ReplicateResource, StopReplicateResourceExt};
    use crate::prelude::client::NetworkingState;
    use crate::prelude::{NetworkTarget, Replicate};
    use crate::shared::replication::resources::ReplicateResourceExt;
    use crate::tests::protocol::{Component1, Resource1};
    use crate::tests::stepper::{BevyStepper, Step};
    use bevy::prelude::{Commands, Entity, OnEnter, With};

    #[test]
    fn test_resource_replication_manually() {
        let mut stepper = BevyStepper::default();

        // spawn an entity that can replicate a resource
        let server_entity = stepper
            .server_app
            .world
            .spawn((ReplicateResource::<Resource1>::default(), Component1(1.0)))
            .id();
        // make sure that there is no panic
        stepper.frame_step();
        stepper.frame_step();

        // add replicate
        stepper
            .server_app
            .world
            .entity_mut(server_entity)
            .insert(Replicate::default());
        // make sure that there is no panic
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was replicated correctly
        let replicated_component = stepper
            .client_app
            .world
            .query::<&Component1>()
            .get_single(&stepper.client_app.world)
            .unwrap();

        // add the resource
        stepper.server_app.world.insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        let replicated_resource = stepper
            .client_app
            .world
            .query::<&ReplicateResource<Resource1>>()
            .get_single(&stepper.client_app.world)
            .unwrap();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 1.0);

        // update the resource
        stepper.server_app.world.resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the update was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 2.0);

        // remove the resource
        stepper.server_app.world.remove_resource::<Resource1>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was removed on the client
        assert!(stepper
            .client_app
            .world
            .get_resource::<Resource1>()
            .is_none());
    }

    #[test]
    fn test_resource_replication_via_commands() {
        let mut stepper = BevyStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource1>(Replicate::default());
                });
        let stop_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource1>();
                });
        let _ = stepper.server_app.world.run_system(start_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was replicated correctly
        let replicated_component = stepper
            .client_app
            .world
            .query::<&ReplicateResource<Resource1>>()
            .get_single(&stepper.client_app.world)
            .unwrap();

        // add the resource
        stepper.server_app.world.insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        let replicated_resource = stepper
            .client_app
            .world
            .query::<&ReplicateResource<Resource1>>()
            .get_single(&stepper.client_app.world)
            .unwrap();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 1.0);

        // update the resource
        stepper.server_app.world.resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the update was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 2.0);

        // remove the resource
        stepper.server_app.world.remove_resource::<Resource1>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was removed on the client
        assert!(stepper
            .client_app
            .world
            .get_resource::<Resource1>()
            .is_none());

        // re-add the resource
        stepper.server_app.world.insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 1.0);

        // stop replicating the resource
        let _ = stepper.server_app.world.run_system(stop_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // update the resource
        stepper.server_app.world.resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was not deleted on the client, but also not updated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 1.0);
    }

    #[test]
    fn test_client_disconnect() {
        let mut stepper = BevyStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource1>(Replicate::default());
                });

        stepper.server_app.world.insert_resource(Resource1(1.0));
        let _ = stepper.server_app.world.run_system(start_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 1.0);

        // disconnect the client, which should despawn any replicated entities
        stepper
            .client_app
            .world
            .run_schedule(OnEnter(NetworkingState::Disconnected));

        stepper.frame_step();

        // check that the resource was removed on the client
        assert!(stepper
            .client_app
            .world
            .get_resource::<Resource1>()
            .is_none());
    }

    /// Check that:
    /// - we can stop replicating a resource without despawning the entity/resource
    /// - when the replication is stopped, we can despawn the resource on the sender, and the receiver still has the resource
    /// - we can call `start_replicate_resource` even if the replicating entity already exists
    #[test]
    fn test_stop_replication_without_despawn() {
        let mut stepper = BevyStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource1>(Replicate::default());
                });
        let stop_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource1>();
                });
        let _ = stepper.server_app.world.run_system(start_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // check that the component was replicated correctly
        let replicated_component = stepper
            .client_app
            .world
            .query::<&ReplicateResource<Resource1>>()
            .get_single(&stepper.client_app.world)
            .unwrap();

        // add the resource
        stepper.server_app.world.insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        let replicated_resource = stepper
            .client_app
            .world
            .query::<&ReplicateResource<Resource1>>()
            .get_single(&stepper.client_app.world)
            .unwrap();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 1.0);

        // stop replicating the resource
        let _ = stepper.server_app.world.run_system(stop_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // update the resource
        stepper.server_app.world.resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was not deleted on the client, but also not updated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 1.0);

        // re-start replicating the resource
        let _ = stepper.server_app.world.run_system(start_replicate_system);
        stepper.frame_step();
        // update the resource
        stepper.server_app.world.resource_mut::<Resource1>().0 = 3.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 3.0);

        // stop replicating the resource
        let _ = stepper.server_app.world.run_system(stop_replicate_system);
        stepper.frame_step();
        // remove the resource
        stepper.server_app.world.remove_resource::<Resource1>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the deletion hasn't been replicated
        assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 3.0);
    }
}
