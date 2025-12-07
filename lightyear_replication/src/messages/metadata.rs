use bevy_ecs::entity::Entity;
use bevy_ecs::event::Event;
use lightyear_core::time::PositiveTickDelta;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};

/// Default reliable channel to replicate metadata about the Sender or the connection
pub struct MetadataChannel;
#[derive(Event, Debug)]
pub struct SenderMetadata {
    pub send_interval: PositiveTickDelta,
    pub sender_entity: Entity,
}

impl ToBytes for SenderMetadata {
    fn bytes_len(&self) -> usize {
        self.send_interval.bytes_len() + self.sender_entity.bytes_len()
    }

    fn to_bytes(
        &self,
        buffer: &mut impl WriteInteger,
    ) -> bevy_ecs::error::Result<(), SerializationError> {
        self.send_interval.to_bytes(buffer)?;
        self.sender_entity.to_bytes(buffer)?;
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> bevy_ecs::error::Result<Self, SerializationError>
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
