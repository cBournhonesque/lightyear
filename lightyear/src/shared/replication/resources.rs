//! Module to handle the replication of bevy [`Resource`]s

use crate::_reexport::{ComponentProtocol, ReplicationSend};
use crate::prelude::{Message, Protocol};
use crate::shared::replication::components::Replicate;
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};
use async_compat::CompatExt;
use bevy::app::App;
use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, IntoSystemConfigs, IntoSystemSetConfigs, Plugin,
    PostUpdate, PreUpdate, Query, Ref, Res, ResMut, Resource, SystemSet, With,
};
use serde::{Deserialize, Serialize};
use tracing::error;

/// This component can be added to an entity to start replicating a [`Resource`] to remote clients.
///
/// Currently resources are cloned to be replicated, so only use this for resources that are
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

pub(crate) struct ResourceSendPlugin<P, R> {
    _marker: std::marker::PhantomData<(P, R)>,
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
    let Some(resource) = resource else {
        return;
    };
    if replicating_entity.iter().len() > 1 {
        error!(
            "Only one entity per World should have a ReplicateResource<{:?}> component",
            std::any::type_name::<R>()
        );
        return;
    }
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

pub(crate) struct ResourceReceivePlugin<P, R> {
    _marker: std::marker::PhantomData<(P, R)>,
}

impl<P, R> Default for ResourceReceivePlugin<P, R> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
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
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReplicateResource;
    use crate::tests::protocol::{Component1, Replicate, Resource1};
    use crate::tests::stepper::{BevyStepper, Step};

    #[test]
    fn test_resource_replication() {
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
    }
}
