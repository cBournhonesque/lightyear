//! This module contains the `ReplicationReceivePlugin` and `ReplicationSendPlugin` plugins, which control
//! the replication of entities and resources.
//!
use crate::authority::{AuthorityPeer, HasAuthority};
use crate::buffer::Replicate;
use crate::components::*;
use crate::delta::DeltaManager;
use crate::hierarchy::ReplicateLike;
use crate::receive::ReplicationReceiver;
use crate::send::ReplicationSender;
use bevy::prelude::*;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ReplicationSet {
    // PRE UPDATE
    /// Receive replication messages and apply them to the World
    Receive,

    // PostUpdate
    /// Flush the messages buffered in the Link to the io
    Send,
}

pub(crate) struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<TargetEntity>()
            .register_type::<Replicated>()
            .register_type::<Controlled>()
            .register_type::<Replicating>()
            .register_type::<Replicate>()
            .register_type::<DisableReplicateHierarchy>()
            .register_type::<ReplicateLike>()
            .register_type::<ReplicationGroupIdBuilder>()
            .register_type::<ReplicationGroup>()
            .register_type::<ReplicationGroupId>()
            .register_type::<NetworkRelevanceMode>()
            .register_type::<ShouldBeInterpolated>()
            .register_type::<PrePredicted>()
            .register_type::<ShouldBePredicted>()
            .register_type::<HasAuthority>()
            .register_type::<AuthorityPeer>();
    }

    fn finish(&self, app: &mut App) {
        // PROTOCOL
        // we register components here because
        // - we need to run this in `finish` so that all plugins have been built (so ClientPlugin and ServerPlugin
        // both exists)
        // - the replication::SharedPlugin should only be added once, even when running in host-server mode
        // app.register_component::<PreSpawned>(ChannelDirection::Bidirectional);
        // app.register_component::<PrePredicted>(ChannelDirection::Bidirectional);
        // app.register_component::<ShouldBePredicted>(ChannelDirection::ServerToClient);
        // app.register_component::<ShouldBeInterpolated>(ChannelDirection::ServerToClient);
        // app.register_component::<RelationshipSync<ChildOf>>(ChannelDirection::Bidirectional)
        //     // to replicate ReplicationSync on the predicted/interpolated entities so that they spawn their own hierarchies
        //     .add_prediction(ComponentSyncMode::Simple)
        //     .add_interpolation(ComponentSyncMode::Simple)
        //     .add_map_entities();
        // app.register_component::<Controlled>(ChannelDirection::ServerToClient)
        //     .add_prediction(ComponentSyncMode::Once)
        //     .add_interpolation(ComponentSyncMode::Once);
        //
        // app.register_message::<AuthorityChange>(ChannelDirection::ServerToClient)
        //     .add_map_entities();
        //
        // // check that the protocol was built correctly
        // app.world().resource::<ComponentRegistry>().check();
    }
}
