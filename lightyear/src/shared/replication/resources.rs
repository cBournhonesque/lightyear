//! Module to handle the replication of bevy [`Resource`]s

use crate::_reexport::{ComponentProtocol, ReplicationSend};
use crate::prelude::{Message, Protocol};
use crate::shared::replication::components::Replicate;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};
use async_compat::CompatExt;
use bevy::app::App;
use bevy::ecs::system::Command;
use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, IntoSystemConfigs, IntoSystemSetConfigs, Plugin,
    PostUpdate, PreUpdate, Query, Ref, Res, ResMut, Resource, SystemSet, With, World,
};
use serde::{Deserialize, Serialize};
use std::marker::PhantomData;
use tracing::error;

pub mod command {
    use super::*;

    /// Extension trait to add the `replicate_resource` method to [`Commands`].
    pub trait ReplicateResourceExt<P: Protocol> {
        /// Start replicating a resource to remote clients.
        ///
        /// Any change to the resource will be replicated to the clients.
        // TODO: we use `Replicate<P>` as argument instead of the simpler `NetworkTarget`
        //  because it helps with type-inference when calling this method.
        //  We can switch to `NetworkTarget` if we remove the `P` bound of `Replicate`.
        fn replicate_resource<R: Resource + Clone>(&mut self, replicate: Replicate<P>);
    }

    impl<P: Protocol> ReplicateResourceExt<P> for Commands<'_, '_> {
        fn replicate_resource<R: Resource + Clone>(&mut self, replicate: Replicate<P>) {
            self.spawn((ReplicateResource::<R>::default(), replicate));
        }
    }

    pub struct StopReplicateCommand<R> {
        _marker: PhantomData<R>,
    }

    impl<R: Resource + Clone> Command for StopReplicateCommand<R> {
        fn apply(self, world: &mut World) {
            if let Ok(entity) = world
                .query_filtered::<Entity, With<ReplicateResource<R>>>()
                .get_single(world)
            {
                world.despawn(entity);
            }
        }
    }

    pub trait StopReplicateResourceExt {
        /// Stop replicating a resource to remote clients.
        fn stop_replicate_resource<R: Resource + Clone>(&mut self);
    }

    impl StopReplicateResourceExt for Commands<'_, '_> {
        fn stop_replicate_resource<R: Resource + Clone>(&mut self) {
            self.add(StopReplicateCommand::<R> {
                _marker: PhantomData,
            });
        }
    }
}
pub use command::{ReplicateResourceExt, StopReplicateResourceExt};

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

impl<R> Default for ReplicateResource<R> {
    fn default() -> Self {
        Self { resource: None }
    }
}

pub(crate) mod send {
    use super::*;
    pub(crate) struct ResourceSendPlugin<P, R> {
        _marker: PhantomData<(P, R)>,
    }

    impl<P, R> Default for ResourceSendPlugin<P, R> {
        fn default() -> Self {
            Self {
                _marker: std::marker::PhantomData,
            }
        }
    }

    impl<P: Protocol, R: ReplicationSend<P>> Plugin for ResourceSendPlugin<P, R> {
        fn build(&self, app: &mut App) {
            app.configure_sets(
                PostUpdate,
                // we need to make sure that the resource data is copied to the component before
                // we send the component update
                InternalReplicationSet::<R::SetMarker>::SendResourceUpdates
                    .before(InternalReplicationSet::<R::SetMarker>::SendComponentUpdates),
            );
            P::Components::add_resource_send_systems::<R>(app);
        }
    }

    pub fn add_resource_send_systems<P: Protocol, S: ReplicationSend<P>, R: Resource + Clone>(
        app: &mut App,
    ) {
        app.add_systems(
            PostUpdate,
            copy_send_resource::<P, R>
                .in_set(InternalReplicationSet::<S::SetMarker>::SendResourceUpdates),
        );
    }

    fn copy_send_resource<P: Protocol, R: Resource + Clone>(
        resource: Option<Res<R>>,
        mut replicating_entity: Query<&mut ReplicateResource<R>, With<Replicate<P>>>,
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
    pub(crate) struct ResourceReceivePlugin<P, R> {
        _marker: PhantomData<(P, R)>,
    }

    impl<P, R> Default for ResourceReceivePlugin<P, R> {
        fn default() -> Self {
            Self {
                _marker: PhantomData,
            }
        }
    }

    impl<P: Protocol, R: ReplicationSend<P>> Plugin for ResourceReceivePlugin<P, R> {
        fn build(&self, app: &mut App) {
            app.configure_sets(
                PreUpdate,
                InternalReplicationSet::<R::SetMarker>::ReceiveResourceUpdates
                    .after(InternalMainSet::<R::SetMarker>::Receive),
            );
            P::Components::add_resource_receive_systems::<R>(app);
        }
    }

    pub fn add_resource_receive_systems<P: Protocol, S: ReplicationSend<P>, R: Resource + Clone>(
        app: &mut App,
    ) {
        app.add_systems(
            PreUpdate,
            copy_receive_resource::<R>
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
}

#[cfg(test)]
mod tests {
    use super::{ReplicateResource, StopReplicateResourceExt};
    use crate::prelude::NetworkTarget;
    use crate::shared::replication::resources::ReplicateResourceExt;
    use crate::tests::protocol::{Component1, Replicate, Resource1};
    use crate::tests::stepper::{BevyStepper, Step};
    use bevy::prelude::Commands;

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
}
