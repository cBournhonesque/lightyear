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

pub use command::{ReplicateResourceExt, StopReplicateResourceExt};

use crate::prelude::{ChannelKind, Message, NetworkTarget};
use crate::protocol::BitSerializable;
use crate::shared::replication::components::Replicate;
use crate::shared::replication::ReplicationSend;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};

mod command {
    use super::*;
    use crate::prelude::Channel;

    /// Extension trait to be able to replicate a resource to remote clients via [`Commands`].
    pub trait ReplicateResourceExt {
        /// Start replicating a resource to remote clients.
        ///
        /// Any change to the resource will be replicated to the clients.
        fn replicate_resource<R: Resource, C: Channel>(&mut self, target: NetworkTarget);
    }

    impl ReplicateResourceExt for Commands<'_, '_> {
        fn replicate_resource<R: Resource, C: Channel>(&mut self, target: NetworkTarget) {
            self.insert_resource(ReplicateResourceMetadata::<R> {
                target,
                channel: ChannelKind::of::<C>(),
                _marker: PhantomData,
            });
        }
    }

    /// Extension trait to be able to stop replicating a resource to remote clients via [`Commands`].
    pub trait StopReplicateResourceExt {
        /// Stop replicating a resource to remote clients.
        ///
        /// This doesn't despawn the resource, it just stops replicating the updates.
        fn stop_replicate_resource<R: Resource>(&mut self);
    }

    impl StopReplicateResourceExt for Commands<'_, '_> {
        fn stop_replicate_resource<R: Resource>(&mut self) {
            self.remove_resource::<ReplicateResourceMetadata<R>>();
        }
    }
}

/// Metadata indicating how a resource should be replicated.
/// The resource is only replicated if this resource exists
///
/// Currently, resources are cloned to be replicated, so only use this for resources that are
/// cheap-to-clone. (the clone only happens when the resource is modified)
///
/// Only one entity per World should have this component.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct ReplicateResourceMetadata<R> {
    pub target: NetworkTarget,
    pub channel: ChannelKind,
    _marker: PhantomData<R>,
}

/// Message that indicates that a resource should be despawned
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DespawnResource<R> {
    _marker: PhantomData<R>,
}

impl<R> Default for DespawnResource<R> {
    fn default() -> Self {
        Self {
            _marker: PhantomData,
        }
    }
}

pub(crate) mod send {
    use super::*;
    use crate::prelude::NetworkIdentity;
    use crate::shared::message::MessageSend;
    use bevy::prelude::resource_removed;
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
        fn build(&self, app: &mut App) {}
    }

    pub(crate) fn add_resource_send_systems<R: Resource + Message, S: MessageSend>(app: &mut App) {
        app.add_systems(
            PostUpdate,
            (
                send_resource_removal::<R, S>.run_if(resource_removed::<R>()),
                send_resource_update::<R, S>,
            )
                .in_set(InternalReplicationSet::<S::SetMarker>::BufferResourceUpdates),
        );
    }

    /// Send a message indicating that the resource was removed
    fn send_resource_removal<R: Resource + Message, S: MessageSend>(
        mut connection_manager: ResMut<S>,
        replication_resource: Option<Res<ReplicateResourceMetadata<R>>>,
    ) {
        if let Some(replication_resource) = replication_resource {
            let _ = connection_manager.erased_send_message_to_target::<DespawnResource<R>>(
                &DespawnResource::default(),
                replication_resource.channel,
                replication_resource.target.clone(),
            );
        }
    }

    /// Send a message when the resource is updated
    fn send_resource_update<R: Resource + Message, S: MessageSend>(
        mut connection_manager: ResMut<S>,
        replication_resource: Option<Res<ReplicateResourceMetadata<R>>>,
        resource: Option<Res<R>>,
    ) {
        if let Some(resource) = resource {
            if resource.is_changed() {
                if let Some(replication_resource) = replication_resource {
                    let _ = connection_manager.erased_send_message_to_target(
                        resource.as_ref(),
                        replication_resource.channel,
                        replication_resource.target.clone(),
                    );
                }
            }
        }
    }
}

pub(crate) mod receive {
    use crate::prelude::{ChannelDirection, ComponentRegistry, NetworkIdentity};
    use crate::protocol::component::ComponentKind;
    use crate::protocol::EventContext;
    use crate::shared::events::components::{
        ComponentInsertEvent, ComponentRemoveEvent, ComponentUpdateEvent, MessageEvent,
    };
    use crate::shared::message::MessageSend;
    use bevy::prelude::{DetectChangesMut, EventReader, Events, RemovedComponents};
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
            // TODO: have a way to delete a resource if it was spawned via connection?
            //  i.e. when we receive a resource, create an entity/resource with DespawnTracker<R>
            //  on disconnection, delete that entity as well as the associated resource.
        }
    }

    pub(crate) fn add_resource_receive_systems<R: Resource + Message, S: MessageSend>(
        app: &mut App,
    ) {
        app.add_systems(
            PreUpdate,
            handle_resource_message::<R, S::EventContext>
                .in_set(InternalReplicationSet::<S::SetMarker>::ReceiveResourceUpdates),
        );
    }

    fn handle_resource_message<R: Resource + Message, Ctx: EventContext>(
        mut commands: Commands,
        mut update_message: ResMut<Events<MessageEvent<R, Ctx>>>,
        mut remove_message: EventReader<MessageEvent<DespawnResource<R>, Ctx>>,
        mut resource: Option<ResMut<R>>,
    ) {
        for message in update_message.drain() {
            // TODO: disable change detection only for bidirectional!
            if let Some(ref mut resource) = resource {
                // write the received value to the resource
                // without change detection to avoid an infinite loop
                *(resource.bypass_change_detection()) = message.message;
            } else {
                commands.insert_resource(message.message);
            }
        }
        for message in remove_message.read() {
            if resource.is_some() {
                commands.remove_resource::<R>();
            }
        }
    }

    // TODO: upon disconnection, despawn the replicated resource?
    // /// If the entity that was driving the replication of the resource is despawned (usually when the
    // /// client disconnects from the server), despawn the resource
    // fn handle_despawned_entity<R: Resource + Clone>(
    //     mut commands: Commands,
    //     mut despawned: RemovedComponents<ReplicateResource<R>>,
    // ) {
    //     for despawned_entity in despawned.read() {
    //         error!(
    //             "Despawning resource {:?} because the entity was despawned",
    //             std::any::type_name::<R>()
    //         );
    //         commands.remove_resource::<R>();
    //     }
    // }
}

#[cfg(test)]
mod tests {
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::{apply_deferred, Commands, OnEnter, Resource};
    use serde::{Deserialize, Serialize};
    use std::marker::PhantomData;
    use tracing::error;

    use crate::prelude::client::NetworkingState;
    use crate::prelude::{AppComponentExt, NetworkTarget, Replicate};
    use crate::shared::replication::resources::ReplicateResourceExt;
    use crate::tests::protocol::{Channel1, Component1, Resource1, Resource2};
    use crate::tests::stepper::{BevyStepper, Step};

    use super::{ReplicateResourceMetadata, StopReplicateResourceExt};

    #[test]
    fn test_resource_replication_via_commands() {
        let mut stepper = BevyStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource1, Channel1>(NetworkTarget::All);
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

        // add the resource
        stepper.server_app.world.insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

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

    // /// Check that when a client disconnects, every resource that was spawned from replication
    // /// gets despawned.
    // #[test]
    // fn test_client_disconnect() {
    //     let mut stepper = BevyStepper::default();
    //
    //     // start replicating a resource via commands (even if the resource doesn't exist yet)
    //     let start_replicate_system =
    //         stepper
    //             .server_app
    //             .world
    //             .register_system(|mut commands: Commands| {
    //                 commands.replicate_resource::<Resource1, Channel1>(NetworkTarget::All);
    //             });
    //
    //     stepper.server_app.world.insert_resource(Resource1(1.0));
    //     let _ = stepper.server_app.world.run_system(start_replicate_system);
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // check that the resource was replicated
    //     assert_eq!(stepper.client_app.world.resource::<Resource1>().0, 1.0);
    //
    //     // disconnect the client, which should despawn any replicated entities
    //     stepper
    //         .client_app
    //         .world
    //         .run_schedule(OnEnter(NetworkingState::Disconnected));
    //
    //     stepper.frame_step();
    //
    //     // check that the resource was removed on the client
    //     assert!(stepper
    //         .client_app
    //         .world
    //         .get_resource::<Resource1>()
    //         .is_none());
    // }

    /// Check that:
    /// - we can stop replicating a resource without despawning the resource
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
                    commands.replicate_resource::<Resource1, Channel1>(NetworkTarget::All);
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

        // add the resource
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

    /// Check that:
    /// - resource replication works in both directions
    #[test]
    fn test_bidirectional_replication() {
        let mut stepper = BevyStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_server_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource2, Channel1>(NetworkTarget::All);
                });
        let stop_server_replicate_system =
            stepper
                .server_app
                .world
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource2>();
                });
        stepper.server_app.world.insert_resource(Resource2(1.0));
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
                    commands.replicate_resource::<Resource2, Channel1>(NetworkTarget::All);
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
        // TODO: why do we need an extra frame here?
        stepper.frame_step();

        // check that the update was replicated to the server
        assert_eq!(stepper.server_app.world.resource::<Resource2>().0, 2.0);

        // stop the replication on the server, and remove the resource
        let _ = stepper
            .server_app
            .world
            .run_system(stop_server_replicate_system);
        stepper.server_app.world.remove_resource::<Resource2>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource is still present on the client
        assert_eq!(stepper.client_app.world.resource::<Resource2>().0, 2.0);

        // update the resource on the client, it should be replicated again to the server
        stepper.client_app.world.resource_mut::<Resource2>().0 = 3.0;
        stepper.frame_step();
        stepper.frame_step();
        // TODO: why do we need an extra frame here?
        // stepper.frame_step();

        // check that the update was replicated to the server
        assert_eq!(stepper.server_app.world.resource::<Resource2>().0, 3.0);
    }
}
