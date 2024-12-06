//! Module to handle the replication of bevy [`Resource`]s

use std::marker::PhantomData;

use bevy::app::App;
use bevy::prelude::{
    Commands, DetectChanges, IntoSystemConfigs, IntoSystemSetConfigs, Plugin, PostUpdate,
    PreUpdate, Res, ResMut, Resource,
};
pub use command::{ReplicateResourceExt, StopReplicateResourceExt};
use serde::{Deserialize, Serialize};

use crate::prelude::{ChannelKind, Message};
use crate::shared::replication::network_target::NetworkTarget;
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

    use crate::connection::client::{ClientConnection, NetClient};
    use crate::shared::message::MessageSend;
    use bevy::prelude::resource_removed;
    use tracing::trace;

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

    pub(crate) fn add_resource_send_systems<
        R: Resource + Message,
        S: MessageSend + ReplicationSend,
    >(
        app: &mut App,
    ) {
        app.add_systems(
            PostUpdate,
            (
                send_resource_removal::<R, S>.run_if(resource_removed::<R>),
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
                &mut DespawnResource::default(),
                replication_resource.channel,
                replication_resource.target.clone(),
            );
        }
    }

    /// Send a message when the resource is updated
    fn send_resource_update<R: Resource + Message, S: MessageSend + ReplicationSend>(
        mut connection_manager: ResMut<S>,
        replication_resource: Option<Res<ReplicateResourceMetadata<R>>>,
        // TODO: support Res<R> by separating MapEntities from non-map-entities?
        mut resource: Option<ResMut<R>>,
        local_client_connection: Option<Res<ClientConnection>>,
    ) {
        // send the resource to newly connected clients
        let new_clients = connection_manager.new_connected_clients();
        if !new_clients.is_empty() {
            if let Some(resource) = resource.as_mut() {
                if let Some(replication_resource) = replication_resource.as_ref() {
                    trace!(
                        "sending resource replication update to new clients: {:?}",
                        std::any::type_name::<R>()
                    );
                    let _ = connection_manager.erased_send_message_to_target(
                        resource.as_mut(),
                        replication_resource.channel,
                        NetworkTarget::Only(new_clients.clone()),
                    );
                }
            }
        }
        if let Some(resource) = resource.as_mut() {
            if resource.is_changed() {
                if let Some(replication_resource) = replication_resource {
                    trace!(
                        "sending resource replication update: {:?}",
                        std::any::type_name::<R>()
                    );
                    let mut target = replication_resource.target.clone();
                    // no need to send a duplicate message to new clients
                    target.exclude(&NetworkTarget::Only(new_clients));
                    // if running in host-server mode, we don't want to replicate the resource to the local client
                    if let Some(local_client) = local_client_connection.as_ref() {
                        target.exclude(&NetworkTarget::Single(local_client.client.id()));
                    }
                    let _ = connection_manager.erased_send_message_to_target(
                        resource.as_mut(),
                        replication_resource.channel,
                        target,
                    );
                }
            }
        }
    }
}

pub(crate) mod receive {

    use crate::protocol::EventContext;
    use crate::shared::events::components::MessageEvent;
    use crate::shared::message::MessageSend;

    use crate::shared::replication::ReplicationPeer;
    use bevy::prelude::{DetectChangesMut, EventReader, Events};
    use tracing::trace;

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

    impl<R: ReplicationPeer> Plugin for ResourceReceivePlugin<R> {
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

    pub(crate) fn add_resource_receive_systems<
        R: Resource + Message,
        S: MessageSend + ReplicationSend,
    >(
        app: &mut App,
        is_bidirectional: bool,
    ) {
        // If `is_bidirectional` is  true, that means that the resource can be replicated in both directions.
        // In that case, we need to disable change detection or we would get an infinite loop of updates.
        if is_bidirectional {
            app.add_systems(
                PreUpdate,
                handle_resource_message_bidirectional::<R, S::EventContext>
                    .in_set(InternalReplicationSet::<S::SetMarker>::ReceiveResourceUpdates),
            );
        } else {
            app.add_systems(
                PreUpdate,
                handle_resource_message::<R, S::EventContext>
                    .in_set(InternalReplicationSet::<S::SetMarker>::ReceiveResourceUpdates),
            );
        }
    }

    fn handle_resource_message<R: Resource + Message, Ctx: EventContext>(
        mut commands: Commands,
        mut update_message: ResMut<Events<MessageEvent<R, Ctx>>>,
        mut remove_message: EventReader<MessageEvent<DespawnResource<R>, Ctx>>,
        mut resource: Option<ResMut<R>>,
    ) {
        for message in update_message.drain() {
            trace!("received resource replication message");
            // TODO: disable change detection only for bidirectional!
            if let Some(ref mut resource) = resource {
                **resource = message.message;
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

    fn handle_resource_message_bidirectional<R: Resource + Message, Ctx: EventContext>(
        mut commands: Commands,
        mut update_message: ResMut<Events<MessageEvent<R, Ctx>>>,
        mut remove_message: EventReader<MessageEvent<DespawnResource<R>, Ctx>>,
        mut resource: Option<ResMut<R>>,
    ) {
        for message in update_message.drain() {
            trace!("received resource replication message");
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
    use super::StopReplicateResourceExt;
    use crate::shared::replication::network_target::NetworkTarget;
    use crate::shared::replication::resources::ReplicateResourceExt;
    use crate::tests::host_server_stepper::HostServerStepper;
    use crate::tests::protocol::{Channel1, Resource1, Resource2};
    use crate::tests::stepper::BevyStepper;
    use bevy::prelude::*;

    #[test]
    fn test_resource_replication_via_commands() {
        let mut stepper = BevyStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_replicate_system =
            stepper
                .server_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource1, Channel1>(NetworkTarget::All);
                });
        let stop_replicate_system =
            stepper
                .server_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource1>();
                });
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(start_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // add the resource
        stepper
            .server_app
            .world_mut()
            .insert_resource(Resource1(1.0));
        dbg!("SHOULD SEND RESOURCE MESSAGE");
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);

        // update the resource
        stepper.server_app.world_mut().resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the update was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 2.0);

        // remove the resource
        stepper
            .server_app
            .world_mut()
            .remove_resource::<Resource1>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was removed on the client
        assert!(stepper
            .client_app
            .world()
            .get_resource::<Resource1>()
            .is_none());

        // re-add the resource
        stepper
            .server_app
            .world_mut()
            .insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);

        // stop replicating the resource
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(stop_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // update the resource
        stepper.server_app.world_mut().resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was not deleted on the client, but also not updated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);
    }

    #[test]
    fn test_resource_replication_via_commands_host_server() {
        let mut stepper = HostServerStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_replicate_system =
            stepper
                .server_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource1, Channel1>(NetworkTarget::All);
                });
        let stop_replicate_system =
            stepper
                .server_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource1>();
                });
        let change_detection_closure = |resource: Res<Resource1>, mut changes: Local<u32>| -> u32 {
            if resource.is_changed() {
                *changes += 1;
            }
            *changes
        };
        let change_detection_system_client = stepper
            .client_app
            .world_mut()
            .register_system(change_detection_closure);
        let change_detection_system_server = stepper
            .server_app
            .world_mut()
            .register_system(change_detection_closure);
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(start_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // add the resource
        stepper
            .server_app
            .world_mut()
            .insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);
        assert_eq!(stepper.server_app.world().resource::<Resource1>().0, 1.0);

        // Check that we got a change detection
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .run_system(change_detection_system_client)
                .ok()
                .unwrap(),
            1
        );
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .run_system(change_detection_system_server)
                .ok()
                .unwrap(),
            1
        );

        // update the resource
        stepper.server_app.world_mut().resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();
        stepper.frame_step();

        dbg!(
            "CLIENT RESOURCE: {:?}",
            stepper.client_app.world().resource::<Resource1>()
        );

        // check that the update was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 2.0);
        assert_eq!(stepper.server_app.world().resource::<Resource1>().0, 2.0);

        // Check that we got a change detection
        assert_eq!(
            stepper
                .client_app
                .world_mut()
                .run_system(change_detection_system_client)
                .ok()
                .unwrap(),
            2
        );
        assert_eq!(
            stepper
                .server_app
                .world_mut()
                .run_system(change_detection_system_server)
                .ok()
                .unwrap(),
            2
        );

        // remove the resource
        stepper
            .server_app
            .world_mut()
            .remove_resource::<Resource1>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was removed on the client
        assert!(stepper
            .client_app
            .world()
            .get_resource::<Resource1>()
            .is_none());
        assert!(stepper
            .server_app
            .world()
            .get_resource::<Resource1>()
            .is_none());

        // re-add the resource
        stepper
            .server_app
            .world_mut()
            .insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);
        assert_eq!(stepper.server_app.world().resource::<Resource1>().0, 1.0);

        // stop replicating the resource
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(stop_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // update the resource
        stepper.server_app.world_mut().resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was not deleted on the client, but also not updated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);

        // Check that the server still has the resource
        assert_eq!(stepper.server_app.world().resource::<Resource1>().0, 2.0);
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
    //             .world_mut()
    //             .register_system(|mut commands: Commands| {
    //                 commands.replicate_resource::<Resource1, Channel1>(NetworkTarget::All);
    //             });
    //
    //     stepper.server_app.world_mut().insert_resource(Resource1(1.0));
    //     let _ = stepper.server_app.world_mut().run_system(start_replicate_system);
    //     stepper.frame_step();
    //     stepper.frame_step();
    //
    //     // check that the resource was replicated
    //     assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);
    //
    //     // disconnect the client, which should despawn any replicated entities
    //     stepper
    //         .client_app
    //         .world_mut()
    //         .run_schedule(OnEnter(NetworkingState::Disconnected));
    //
    //     stepper.frame_step();
    //
    //     // check that the resource was removed on the client
    //     assert!(stepper
    //         .client_app
    //         .world()    //         .get_resource::<Resource1>()
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
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource1, Channel1>(NetworkTarget::All);
                });
        let stop_replicate_system =
            stepper
                .server_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource1>();
                });
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(start_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // add the resource
        stepper
            .server_app
            .world_mut()
            .insert_resource(Resource1(1.0));
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);

        // stop replicating the resource
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(stop_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // update the resource
        stepper.server_app.world_mut().resource_mut::<Resource1>().0 = 2.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was not deleted on the client, but also not updated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 1.0);

        // re-start replicating the resource
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(start_replicate_system);
        stepper.frame_step();
        // update the resource
        stepper.server_app.world_mut().resource_mut::<Resource1>().0 = 3.0;
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 3.0);

        // stop replicating the resource
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(stop_replicate_system);
        stepper.frame_step();
        // remove the resource
        stepper
            .server_app
            .world_mut()
            .remove_resource::<Resource1>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the deletion hasn't been replicated
        assert_eq!(stepper.client_app.world().resource::<Resource1>().0, 3.0);
    }

    /// Check that:
    /// - resource replication works in both directions
    #[test]
    fn test_bidirectional_replication() {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::ERROR)
        //     .init();
        let mut stepper = BevyStepper::default();

        // start replicating a resource via commands (even if the resource doesn't exist yet)
        let start_server_replicate_system =
            stepper
                .server_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource2, Channel1>(NetworkTarget::All);
                });
        let stop_server_replicate_system =
            stepper
                .server_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource2>();
                });
        stepper
            .server_app
            .world_mut()
            .insert_resource(Resource2(1.0));
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(start_server_replicate_system);
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource was replicated
        assert_eq!(stepper.client_app.world().resource::<Resource2>().0, 1.0);

        // update the resource on the client
        // and start replicating the resource to the server
        let start_client_replicate_system =
            stepper
                .client_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.replicate_resource::<Resource2, Channel1>(NetworkTarget::None);
                });
        let stop_client_replicate_system =
            stepper
                .server_app
                .world_mut()
                .register_system(|mut commands: Commands| {
                    commands.stop_replicate_resource::<Resource2>();
                });

        let _ = stepper
            .client_app
            .world_mut()
            .run_system(start_client_replicate_system);
        stepper.client_app.world_mut().resource_mut::<Resource2>().0 = 2.0;
        stepper.frame_step();

        // check that the update was replicated to the server
        assert_eq!(stepper.server_app.world().resource::<Resource2>().0, 2.0);

        // stop the replication on the server, and remove the resource
        let _ = stepper
            .server_app
            .world_mut()
            .run_system(stop_server_replicate_system);
        stepper
            .server_app
            .world_mut()
            .remove_resource::<Resource2>();
        stepper.frame_step();
        stepper.frame_step();

        // check that the resource is still present on the client
        assert_eq!(stepper.client_app.world().resource::<Resource2>().0, 2.0);

        // update the resource on the client, it should be replicated again to the server
        stepper.client_app.world_mut().resource_mut::<Resource2>().0 = 3.0;
        stepper.frame_step();

        // check that the update was replicated to the server
        assert_eq!(stepper.server_app.world().resource::<Resource2>().0, 3.0);
    }
}
