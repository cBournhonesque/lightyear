//! Module to handle the replication of bevy [`Resource`]s

use crate::_reexport::ReplicationSend;
use crate::prelude::{Message, Protocol};
use crate::shared::sets::{InternalMainSet, InternalReplicationSet};
use async_compat::CompatExt;
use bevy::app::App;
use bevy::prelude::{
    Commands, Component, DetectChanges, Entity, IntoSystemSetConfigs, Plugin, PostUpdate,
    PreUpdate, Query, Ref, Res, ResMut, Resource, SystemSet, With,
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
pub struct ReplicateResource<R: Resource + Message> {
    resource: R,
}

struct ResourceSendPlugin<P> {
    _marker: std::marker::PhantomData<P>,
}

impl<P> Default for ResourceSendPlugin<P> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, R: ReplicationSend<P>> Plugin for ResourceSendPlugin<P> {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PostUpdate,
            // we need to make sure that the resource data is copied to the component before
            // we send the component update
            InternalReplicationSet::<R::SetMarker>::SendResourceUpdates
                .before(InternalReplicationSet::<R::SetMarker>::SendComponentUpdates),
        );
        // TODO: call copy_send_resource for every component that is `ReplicateResource` in the `ComponentProtocol`.
        // app.add_systems(PostUpdate, copy_send_resource())
    }
}

fn copy_send_resource<R: Resource + Clone>(
    resource: Res<R>,
    mut replicating_entity: Query<&mut ReplicateResource<R>>,
) {
    if resource.is_changed() {
        if replicating_entity.iter().len() > 1 {
            error!(
                "Only one entity per World should have a ReplicateResource<{:?}> component",
                std::any::type_name::<R>()
            );
            return;
        }
        // TODO: we should be able to avoid this clone? we only need the reference to the resource to serialize it
        //  - we could directly serialize the data here and store it in the component
        //  - the component could just be a marker that we need to serialize the resource, and then we have a custom
        //    serialization function that fetches the resource and serializes it?
        if let Ok(mut replicating_entity) = replicating_entity.get_single_mut() {
            replicating_entity.resource = resource.clone();
        }
    }
}

struct ResourceReceivePlugin<P> {
    _marker: std::marker::PhantomData<P>,
}

impl<P> Default for ResourceReceivePlugin<P> {
    fn default() -> Self {
        Self {
            _marker: std::marker::PhantomData,
        }
    }
}

impl<P: Protocol, R: ReplicationSend<P>> Plugin for ResourceReceivePlugin<P> {
    fn build(&self, app: &mut App) {
        app.configure_sets(
            PreUpdate,
            InternalReplicationSet::<R::SetMarker>::ReceiveResourceUpdates
                .after(InternalMainSet::<R::SetMarker>::Receive),
        );
        // TODO: call copy_receive_resource for every component that is `ReplicateResource` in the `ComponentProtocol`.
        // app.add_systems(PreUpdate, copy_send_resource())
    }
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
            if let Some(mut resource) = resource {
                *resource = replicating_entity.resource.clone();
            } else {
                commands.insert_resource(replicating_entity.resource.clone());
            }
        }
    }
}
