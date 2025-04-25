//! When a ReplicationSender first connects to a ReplicationReceiver, it sends a
//! a trigger to inform the receiver of its SendInterval. This interval is used
//! by the receiver to determine how the InterpolationTime should be configured

use crate::manager::InterpolationManager;
use bevy::prelude::*;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{PositiveTickDelta, TickDelta};
use lightyear_messages::prelude::{AppTriggerExt, RemoteTrigger};
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_sync::prelude::client::InterpolationTimeline;

// TODO: the message should be a trigger
#[derive(Event, Debug)]
pub struct SenderMetadata {
    send_interval: PositiveTickDelta,
}

impl ToBytes for SenderMetadata {
    fn bytes_len(&self) -> usize {
        self.send_interval.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.send_interval.to_bytes(buffer)
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized
    {
        let send_interval = PositiveTickDelta::from_bytes(buffer)?;
        Ok(Self {
            send_interval,
        })
    }
}

pub struct MetadataPlugin;

impl MetadataPlugin {
    fn handle_sender_metadata(
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
        app.add_trigger_to_bytes::<SenderMetadata>()
            .add_direction(NetworkDirection::Bidirectional);

        app.add_observer(Self::handle_sender_metadata);
    }
}