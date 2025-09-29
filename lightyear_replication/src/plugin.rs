//! This module contains the `ReplicationReceivePlugin` and `ReplicationSendPlugin` plugins, which control
//! the replication of entities and resources.
//!

use crate::control::{Controlled, ControlledBy, ControlledByRemote};
use crate::hierarchy::{DisableReplicateHierarchy, ReplicateLike, ReplicateLikeChildren};
use crate::message::{ActionsChannel, MetadataChannel, SenderMetadata, UpdatesChannel};
use crate::prelude::*;
use bevy_app::{App, Plugin};
use bevy_ecs::schedule::SystemSet;
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

    // PostUpdate
    /// Flush the messages buffered in the Link to the io
    Send,
}

pub(crate) struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<Replicated>()
            .register_type::<ConfirmedTick>()
            .register_type::<InitialReplicated>()
            .register_type::<Replicating>()
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
            app.register_type::<InterpolationTarget>();
        }
        #[cfg(feature = "prediction")]
        {
            app.register_type::<PredictionTarget>();
        }

        app.add_observer(ConfirmedTick::add_confirmed_tick_hook);

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
        app.register_message_to_bytes::<ActionsMessage>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_message_to_bytes::<UpdatesMessage>()
            .add_direction(NetworkDirection::Bidirectional);
        app.register_event_to_bytes::<SenderMetadata>()
            .add_direction(NetworkDirection::Bidirectional);
    }
}
