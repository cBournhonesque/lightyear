//! When a ReplicationSender first connects to a ReplicationReceiver, it sends a
//! a trigger to inform the receiver of its SendInterval. This interval is used
//! by the receiver to determine how the InterpolationTime should be configured

use crate::manager::InterpolationManager;
use bevy::prelude::*;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{PositiveTickDelta, TickDelta};
use lightyear_messages::prelude::{AppTriggerExt, RemoteTrigger};
use lightyear_replication::message::SenderMetadata;
use lightyear_replication::prelude::ReplicationSender;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_sync::prelude::client::InterpolationTimeline;


pub struct MetadataPlugin;

impl MetadataPlugin {
    fn receive_sender_metadata(
        trigger: Trigger<RemoteTrigger<SenderMetadata>>,
        tick_duration: Res<TickDuration>,
        mut query: Query<&mut InterpolationTimeline>,
    ) {
        let delta = TickDelta::from(trigger.trigger.send_interval);
        let duration = delta.to_duration(tick_duration.0);
        query.iter_mut().for_each(|mut interpolation_timeline| {
            debug!("Updating remote send interval to {:?}", duration);
            interpolation_timeline.context.remote_send_interval = duration;
        })
    }
}


impl Plugin for MetadataPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(Self::receive_sender_metadata);
    }
}