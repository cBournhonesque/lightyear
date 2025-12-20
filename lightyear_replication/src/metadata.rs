use core::time::Duration;
use bevy_app::{App, Plugin};
use lightyear_serde::prelude::*;
use bevy_ecs::prelude::*;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{PositiveTickDelta, TickDelta};
use lightyear_messages::prelude::{AppTriggerExt, EventSender};
use lightyear_serde::reader::Reader;
use lightyear_serde::ToBytes;
use lightyear_serde::writer::WriteInteger;
use lightyear_transport::prelude::*;
use crate::prelude::ReplicationSender;
use crate::server::ReplicationMetadata;

#[derive(Event, Debug)]
pub struct SenderMetadata {
    pub send_interval: PositiveTickDelta,
    pub sender_entity: Entity,
}

impl ToBytes for SenderMetadata {
    fn bytes_len(&self) -> usize {
        self.send_interval.bytes_len() + self.sender_entity.bytes_len()
    }

    fn to_bytes(&self, buffer: &mut impl WriteInteger) -> Result<(), SerializationError> {
        self.send_interval.to_bytes(buffer)?;
        self.sender_entity.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let send_interval = PositiveTickDelta::from_bytes(buffer)?;
        let sender_entity = Entity::from_bytes(buffer)?;
        Ok(Self {
            send_interval,
            sender_entity,
        })
    }
}


/// Default reliable channel to replicate metadata about the Sender or the connection
pub struct MetadataChannel;

/// Send a message containing metadata about the sender
fn send_sender_metadata(
    // NOTE: it's important to trigger on both Add<Connected> and Add<ReplicationSender> because the ClientOf could be
    //  added BEFORE the ReplicationSender is added. (ClientOf is spawned by netcode, ReplicationSender is added by the user)
    trigger: On<Add, (Connected, ReplicationSender)>,
    metadata: Res<ReplicationMetadata>,
    tick_duration: Res<TickDuration>,
    mut query: Query<
        (Entity, &mut EventSender<SenderMetadata>),
        With<Connected>,
    >,
) {
    let send_interval = metadata.timer.duration();
    let send_interval_delta = TickDelta::from_duration(send_interval, tick_duration.0);
    if let Ok((sender_entity, mut trigger_sender)) = query.get_mut(trigger.entity) {
        let metadata = SenderMetadata {
            send_interval: send_interval_delta.into(),
            sender_entity,
        };
        trigger_sender.trigger::<MetadataChannel>(metadata);
    }
}

pub struct MetadataPlugin;

impl Plugin for MetadataPlugin {
    fn build(&self, app: &mut App) {
        app.add_channel::<MetadataChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            send_frequency: Duration::default(),
            priority: 10.0,
        });
        app.register_event_to_bytes::<SenderMetadata>()
            .add_direction(NetworkDirection::Bidirectional);
    }
}