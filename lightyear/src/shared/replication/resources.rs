//! Module to handle the replication of bevy [`Resource`]s

use std::marker::PhantomData;

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
use tracing::error;

pub use command::{ReplicateResourceExt, StopReplicateCommand, StopReplicateResourceExt};

use crate::prelude::Message;
use crate::protocol::BitSerializable;
use crate::shared::replication::components::Replicate;
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

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

// TODO: serialize the resource directly into the component, instead of cloning it,
//  but then we need to make sure that the component is serialized with just a memcpy,
//  since it's already been serialized. Maybe we can implement its BitSerialize ourselves?
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
    use crate::prelude::NetworkIdentity;
    use tracing::info;

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
                InternalReplicationSet::<R::SetMarker>::BufferResourceUpdates
                    .before(InternalReplicationSet::<R::SetMarker>::BufferComponentUpdates),
            );
        }
    }

    pub(crate) fn add_resource_send_systems<S: ReplicationSend, R: Resource + Clone>(
        app: &mut App,
    ) {
        app.add_systems(
            PostUpdate,
            copy_send_resource::<R>
                .in_set(InternalReplicationSet::<S::SetMarker>::BufferResourceUpdates),
        );
    }

    fn copy_send_resource<R: Resource + Clone>(
        identity: NetworkIdentity,
        resource: Option<Res<R>>,
        mut replicating_entity: Query<&mut ReplicateResource<R>, With<Replicate>>,
    ) {
        // if replicating_entity.iter().len() > 1 {
        //     error!(
        //         "Only one entity per World should have a ReplicateResource<{:?}> component",
        //         std::any::type_name::<R>()
        //     );
        //     return;
        // }
        let Some(resource) = resource else {
            // if the resource was removed, remove it from the entity
            if let Ok(mut replicating_entity) = replicating_entity.get_single_mut() {
                if replicating_entity.resource.is_some() {
                    error!(identity = ?identity.identity(), "Sending removal for resource {:?}", std::any::type_name::<R>());
                    replicating_entity.resource = None;
                }
            }
            return;
        };
        if let Ok(mut replicating_entity) = replicating_entity.get_single_mut() {
            if resource.is_changed() || replicating_entity.is_added() {
                error!(identity = ?identity.identity(),
                    "Sending update for resource {:?}",
                    std::any::type_name::<R>()
                );
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
    use crate::prelude::{ChannelDirection, ComponentRegistry, NetworkIdentity};
    use crate::protocol::component::ComponentKind;
    use crate::protocol::EventContext;
    use crate::shared::events::components::{
        ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent,
    };
    use bevy::prelude::{DetectChangesMut, EventReader, RemovedComponents};
    use tracing::debug;

    use super::*;

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
                    .after(InternalMainSet::<R::SetMarker>::EmitEvents),
            );
        }
    }

    pub(crate) fn add_resource_receive_systems<S: ReplicationSend, R: Resource + Clone>(
        app: &mut App,
    ) {
        app.add_systems(
            PreUpdate,
            handle_despawned_entity::<R>
                .in_set(InternalReplicationSet::<S::SetMarker>::ReceiveResourceUpdates),
        );
        app.add_systems(
            PreUpdate,
            copy_receive_resource::<R, S::EventContext>
                .in_set(InternalReplicationSet::<S::SetMarker>::ReceiveResourceUpdates),
        );
    }

    // NOTE: we cannot use directly the value of the resource here inside the ReplicateResource<R> component,
    //  because in the bidirectional case, we would then be removing the resource immediately since the
    //  ReplicateResource component doesn't contain the resource in the Receive SystemSet.
    //  instead, use the replication events.
    fn copy_receive_resource<R: Resource + Clone, Ctx: EventContext>(
        mut commands: Commands,
        identity: NetworkIdentity,
        mut received_inserts: EventReader<ComponentInsertEvent<ReplicateResource<R>, Ctx>>,
        mut received_updates: EventReader<ComponentUpdateEvent<ReplicateResource<R>, Ctx>>,
        replicating_entity: Query<Ref<ReplicateResource<R>>>,
        mut resource: Option<ResMut<R>>,
    ) {
        for entity in received_inserts
            .read()
            .map(|e| e.entity())
            .chain(received_updates.read().map(|e| e.entity()))
        {
            error!(identity = ?identity.identity(), "Received resource replication event");
            if let Ok(replicating_entity) = replicating_entity.get(entity) {
                if replicating_entity.is_changed() {
                    if let Some(received_value) = &replicating_entity.resource {
                        error!(identity = ?identity.identity(), "Update resource");
                        if let Some(ref mut resource) = resource {
                            // write the received value to the resource
                            // without change detection to avoid an infinite loop
                            *(resource.bypass_change_detection()) = received_value.clone();
                        } else {
                            commands.insert_resource(received_value.clone());
                        }
                    } else {
                        error!(
                            is_client = ?identity.is_client(),
                            "Despawning resource {:?} because the replicating entity doesn't contain it",
                            std::any::type_name::<R>(),
                        );
                        commands.remove_resource::<R>();
                    }
                }
            }
        }
    }

    // fn copy_receive_resource<R: Resource + Clone>(
    //     component_registry: Res<ComponentRegistry>,
    //     mut commands: Commands,
    //     replicating_entity: Query<Ref<ReplicateResource<R>>>,
    //     resource: Option<ResMut<R>>,
    // ) {
    //     if replicating_entity.iter().len() > 1 {
    //         error!(
    //             "Only one entity per World should have a ReplicateResource<{:?}> component",
    //             std::any::type_name::<R>()
    //         );
    //         return;
    //     }
    //     if let Ok(replicating_entity) = replicating_entity.get_single() {
    //         if replicating_entity.is_changed() {
    //             if let Some(received_value) = &replicating_entity.resource {
    //                 if let Some(mut resource) = resource {
    //                     // TODO: use set_if_neq for PartialEq
    //                     // resource.set_if_neq(received_value.clone());
    //                     *resource = received_value.clone();
    //                 } else {
    //                     commands.insert_resource(received_value.clone());
    //                 }
    //             } else if let Some(resource) = resource {
    //                 error!(
    //                     "Despawning resource {:?} because the replicating entity doesn't contain it",
    //                     std::any::type_name::<R>()
    //                 );
    //                 commands.remove_resource::<R>();
    //             }
    //         }
    //     }
    // }

    /// If the entity that was driving the replication of the resource is despawned (usually when the
    /// client disconnects from the server), despawn the resource
    fn handle_despawned_entity<R: Resource + Clone>(
        mut commands: Commands,
        mut despawned: RemovedComponents<ReplicateResource<R>>,
    ) {
        for despawned_entity in despawned.read() {
            error!(
                "Despawning resource {:?} because the entity was despawned",
                std::any::type_name::<R>()
            );
            commands.remove_resource::<R>();
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::{apply_deferred, Commands, OnEnter, Resource};
    use serde::{Deserialize, Serialize};
    use std::marker::PhantomData;
    use tracing::error;

    use crate::prelude::client::NetworkingState;
    use crate::prelude::{AppComponentExt, Replicate};
    use crate::shared::replication::resources::command::StartReplicateCommand;
    use crate::shared::replication::resources::ReplicateResourceExt;
    use crate::tests::protocol::{Component1, Resource1, Resource2};
    use crate::tests::stepper::{BevyStepper, Step};

    use super::{ReplicateResource, StopReplicateResourceExt};

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

    /// Check that when a client disconnects, every resource that was spawned from replication
    /// gets despawned.
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

    #[test]
    fn test_bidirectional_replication() {
        tracing_subscriber::FmtSubscriber::builder()
            .with_max_level(tracing::Level::INFO)
            .init();
        let mut stepper = BevyStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_server_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource2>(Replicate::default());
                });
        let stop_server_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource2>();
                });
        stepper.server_app.world.insert_resource(Resource2(1.0));
        error!("resource inserted");
        let _ = stepper
            .server_app
            .world
            .run_system(start_server_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world.resource::<Resource2>().0, 1.0);

        // update the resource on the client
        // and start replicating the resource to the server
        let start_client_replicate_system =
            stepper
                .client_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource2>(Replicate::default());
                });
        let stop_client_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource2>();
                });

        let _ = stepper
            .client_app
            .world
            .run_system(start_client_replicate_system);
        stepper.client_app.world.resource_mut::<Resource2>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();

        // check that the update was replicated to the server
        assert_eq!(stepper.server_app.world.resource::<Resource2>().0, 2.0);
    }
}
