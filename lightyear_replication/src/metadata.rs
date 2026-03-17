use crate::prelude::ReplicationSender;
use bevy_app::prelude::*;
use bevy_ecs::prelude::*;
use bevy_time::{Timer, TimerMode};
use core::time::Duration;
use lightyear_connection::client::Connected;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::id::RemoteId;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{PositiveTickDelta, TickDelta};
use lightyear_messages::MessageManager;
use lightyear_messages::prelude::{AppTriggerExt, EventSender, RemoteEvent};
use lightyear_serde::ToBytes;
use lightyear_serde::prelude::*;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_transport::prelude::*;
use tracing::debug;

/// Resource that needs to be added to control the replication behaviour for the current App.
///
/// This is a resource since the replication interval has to be shared
/// across all senders.
// TODO: add a ReplicationMetadata resource with a replication-timer
//  also the TickDuration is not useful?
#[derive(Resource)]
pub struct ReplicationMetadata {
    pub(crate) timer: Timer,
}

impl ReplicationMetadata {
    pub fn new(replication_interval: Duration) -> Self {
        Self {
            timer: Timer::new(replication_interval, TimerMode::Repeating),
        }
    }
}

impl Default for ReplicationMetadata {
    fn default() -> Self {
        Self::new(Duration::default())
    }
}

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
    mut query: Query<(Entity, &mut EventSender<SenderMetadata>), With<Connected>>,
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

/// Receive metadata about the sender from a remote peer and populate the entity mapper
fn receive_sender_metadata(
    trigger: On<RemoteEvent<SenderMetadata>>,
    mut query: Query<(Entity, &RemoteId, &mut MessageManager)>,
    entity_map: Option<ResMut<bevy_replicon::shared::server_entity_map::ServerEntityMap>>,
) {
    let remote_entity = trigger.trigger.sender_entity;
    let from = &trigger.from;
    for (local_entity, remote_id, mut manager) in query.iter_mut() {
        if remote_id.0 == *from {
            debug!(
                "Received SenderMetadata: mapping remote {:?} -> local {:?}",
                remote_entity, local_entity
            );
            manager.entity_mapper.insert(remote_entity, local_entity);
            // TODO: how can we update this manually?
            // if let Some(mut entity_map) = entity_map {
            //     entity_map.insert(remote_entity, local_entity);
            // }
            return;
        }
    }
}

pub struct MetadataPlugin;

impl Plugin for MetadataPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReplicationMetadata>();
        app.add_channel::<MetadataChannel>(ChannelSettings {
            mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
            send_frequency: Duration::default(),
            priority: 10.0,
        });
        app.register_event_to_bytes::<SenderMetadata>()
            .add_direction(NetworkDirection::Bidirectional);
        app.add_observer(send_sender_metadata);
        app.add_observer(receive_sender_metadata);
    }
}
