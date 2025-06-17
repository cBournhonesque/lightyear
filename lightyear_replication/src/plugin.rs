//! This module contains the `ReplicationReceivePlugin` and `ReplicationSendPlugin` plugins, which control
//! the replication of entities and resources.
//!

use crate::buffer::{Replicate, ReplicationMode};
use crate::components::*;
use crate::control::{Controlled, ControlledBy, ControlledByRemote};
use crate::hierarchy::{DisableReplicateHierarchy, ReplicateLike, ReplicateLikeChildren};
use crate::message::{ActionsChannel, MetadataChannel, SenderMetadata, UpdatesChannel};
use crate::prelude::{ActionsMessage, AppComponentExt, UpdatesMessage};
use bevy::prelude::*;
use core::time::Duration;
use lightyear_connection::prelude::NetworkDirection;
use lightyear_messages::prelude::{AppMessageExt, AppTriggerExt};
use lightyear_transport::channel::builder::ReliableSettings;
use lightyear_transport::prelude::{AppChannelExt, ChannelMode, ChannelSettings};

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ReplicationSet {
    // PRE UPDATE
    /// Receive replication messages and apply them to the World
    Receive,
    ReceiveRelationships,

    // PostUpdate
    /// Flush the messages buffered in the Link to the io
    Send,
}

pub(crate) struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<Replicated>()
            .register_type::<InitialReplicated>()
            .register_type::<Replicating>()
            .register_type::<Confirmed>()
            .register_type::<Controlled>()
            .register_type::<ControlledBy>()
            .register_type::<ControlledByRemote>()
            .register_type::<Replicating>()
            .register_type::<ReplicationMode>()
            .register_type::<Replicate>()
            .register_type::<DisableReplicateHierarchy>()
            .register_type::<ReplicateLike>()
            .register_type::<ReplicateLikeChildren>()
            .register_type::<ComponentReplicationConfig>()
            .register_type::<ComponentReplicationOverride>()
            .register_type::<ReplicationGroupIdBuilder>()
            .register_type::<ReplicationGroup>()
            .register_type::<ReplicationGroupId>();

        app.register_component::<Controlled>();

        #[cfg(feature = "interpolation")]
        {
            app.register_type::<(ShouldBeInterpolated, InterpolationTarget)>();
            app.register_component::<ShouldBeInterpolated>();
        }
        #[cfg(feature = "prediction")]
        {
            app.register_type::<(ShouldBePredicted, PrePredicted, PredictionTarget)>();
            app.register_component::<ShouldBePredicted>();
        }

        app.add_channel::<MetadataChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            send_frequency: Duration::default(),
            priority: 10.0,
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.add_channel::<UpdatesChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliableWithAcks,
            // we do not send the send_frequency to `replication_interval` here
            // because we want to make sure that the entity updates for tick T
            // are sent on tick T, so we will set the `replication_interval`
            // directly on the replication_sender
            send_frequency: Duration::default(),
            priority: 1.0,
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.add_channel::<ActionsChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            // we do not send the send_frequency to `replication_interval` here
            // because we want to make sure that the entity updates for tick T
            // are sent on tick T, so we will set the `replication_interval`
            // directly on the replication_sender
            send_frequency: Duration::default(),
            // we want to send the entity actions as soon as possible
            priority: 10.0,
        })
        .add_direction(NetworkDirection::Bidirectional);
        app.add_message_to_bytes::<ActionsMessage>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_message_to_bytes::<UpdatesMessage>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_trigger_to_bytes::<SenderMetadata>()
            .add_direction(NetworkDirection::Bidirectional);
    }

    fn finish(&self, app: &mut App) {
        // PROTOCOL
        // we register components here because
        // - we need to run this in `finish` so that all plugins have been built (so ClientPlugin and ServerPlugin
        // both exists)
        // - the replication::SharedPlugin should only be added once, even when running in host-server mode
        // app.register_component::<PreSpawned>(ChannelDirection::Bidirectional);
        // app.register_component::<PrePredicted>(ChannelDirection::Bidirectional);

        // TODO: add direction?

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
