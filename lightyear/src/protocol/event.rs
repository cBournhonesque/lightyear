use crate::client::config::ClientConfig;
use crate::prelude::server::ServerConfig;
use crate::prelude::{ChannelDirection, ClientId, Message, MessageRegistry};
use crate::protocol::message::{
    MessageError, MessageKind, MessageMetadata, MessageRegistration, MessageType, ReceiveMessageFn,
};
use crate::protocol::registry::NetId;
use crate::protocol::SerializeFns;
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::{SerializationError, ToBytes};
use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
use bevy::app::App;
use bevy::prelude::{Commands, Event, Events, FilteredResourcesMut};
use byteorder::{ReadBytesExt, WriteBytesExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::debug;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum EventReplicationMode {
    // TODO: Maybe also allow events to be replicated as normal messages? we would need to:
    //  - instead of 'register_event', just add `is_event` to MessageRegistration
    //  - in the serialize_function, check if the message type is MessageType::Event, in which case we would
    //    use an EventReplicationMode::None
    // /// Simply replicate the event as a normal message
    // None,
    ///
    /// Replicate the event and buffer it via an EventWriter
    Buffer,
    /// Replicate the event and trigger it
    Trigger,
}

impl ToBytes for EventReplicationMode {
    fn len(&self) -> usize {
        1
    }

    fn to_bytes<T: WriteBytesExt>(&self, buffer: &mut T) -> Result<(), SerializationError> {
        match self {
            // EventReplicationMode::None => buffer.write_u8(0)?,
            EventReplicationMode::Buffer => buffer.write_u8(1)?,
            EventReplicationMode::Trigger => buffer.write_u8(2)?,
        }
        Ok(())
    }

    fn from_bytes(buffer: &mut Reader) -> Result<Self, SerializationError>
    where
        Self: Sized,
    {
        let mode = buffer.read_u8()?;
        match mode {
            // 0 => Ok(EventReplicationMode::None),
            1 => Ok(EventReplicationMode::Buffer),
            2 => Ok(EventReplicationMode::Trigger),
            _ => Err(SerializationError::InvalidValue),
        }
    }
}

pub(crate) fn register_event_receive<E: Event + Message>(
    app: &mut App,
    direction: ChannelDirection,
) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    let register_fn = |app: &mut App| {
        app.add_event::<E>();
        let message_kind = MessageKind::of::<E>();
        let component_id = app.world_mut().register_resource::<Events<E>>();
        let receive_message_fn: ReceiveMessageFn = MessageRegistry::receive_event_internal::<E>;
        app.world_mut()
            .resource_mut::<MessageRegistry>()
            .message_receive_map
            .insert(
                message_kind,
                MessageMetadata {
                    message_type: MessageType::Event,
                    component_id,
                    receive_message_fn,
                },
            );
    };
    match direction {
        ChannelDirection::ClientToServer => {
            if is_server {
                register_fn(app);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_client {
                register_fn(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_event_receive::<E>(app, ChannelDirection::ClientToServer);
            register_event_receive::<E>(app, ChannelDirection::ServerToClient);
        }
    }
}

impl MessageRegistry {
    pub(crate) fn serialize_event<E: Event + Message>(
        &self,
        event: &E,
        event_replication_mode: EventReplicationMode,
        writer: &mut Writer,
        entity_map: Option<&mut SendEntityMap>,
    ) -> Result<(), MessageError> {
        let kind = MessageKind::of::<E>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        let net_id = self.kind_map.net_id(&kind).unwrap();
        net_id.to_bytes(writer)?;
        event_replication_mode.to_bytes(writer)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe {
            erased_fns.serialize(event, writer, entity_map)?;
        }
        Ok(())
    }

    pub(crate) fn deserialize_event<E: Event + Message>(
        &self,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(E, EventReplicationMode), MessageError> {
        let net_id = NetId::from_bytes(reader)?;
        let event_replication_mode = EventReplicationMode::from_bytes(reader)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(MessageError::NotRegistered)?;
        let erased_fns = self
            .serialize_fns_map
            .get(kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        let event = unsafe { erased_fns.deserialize(reader, entity_map) }?;
        Ok((event, event_replication_mode))
    }

    fn receive_event_internal<E: Message + Event>(
        &self,
        commands: &mut Commands,
        events: &mut FilteredResourcesMut,
        from: ClientId,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        let kind = MessageKind::of::<E>();
        let receive_metadata = self
            .message_receive_map
            .get(&kind)
            .ok_or(MessageError::NotRegistered)?;
        match receive_metadata.message_type {
            MessageType::Event => {
                // we deserialize the message and send an event or trigger the event
                let (event, mode) = self.deserialize_event::<E>(reader, entity_map)?;
                match mode {
                    EventReplicationMode::Buffer => {
                        let events = events
                            .get_mut_by_id(receive_metadata.component_id)
                            .ok_or(MessageError::NotRegistered)?;
                        // SAFETY
                        let mut events = unsafe { events.with_type::<Events<E>>() };
                        events.send(event);
                    }
                    EventReplicationMode::Trigger => {
                        commands.trigger(event);
                    }
                }
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}

pub(crate) trait AppEventInternalExt {
    /// Function used internally to register an Event
    fn register_event_internal<E: Event + Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, E>;

    /// Function used internally to register an event
    /// with a custom [`SerializeFns`] implementation
    fn register_event_internal_custom_serde<E: Event + Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<E>,
    ) -> MessageRegistration<'_, E>;
}

impl AppEventInternalExt for App {
    fn register_event_internal<E: Event + Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, E> {
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        if !registry.is_registered::<E>() {
            registry.add_message::<E>();
        }
        debug!("register event {}", std::any::type_name::<E>());
        register_event_receive::<E>(self, direction);
        MessageRegistration {
            app: self,
            _marker: std::marker::PhantomData,
        }
    }

    fn register_event_internal_custom_serde<E: Event + Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<E>,
    ) -> MessageRegistration<'_, E> {
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        if !registry.is_registered::<E>() {
            registry.add_message_custom_serde::<E>(serialize_fns);
        }
        debug!("register message {}", std::any::type_name::<E>());
        register_event_receive::<E>(self, direction);
        MessageRegistration {
            app: self,
            _marker: std::marker::PhantomData,
        }
    }
}

pub trait AppEventExt {
    /// Registers the event in the Registry
    /// This event can now be sent over the network.
    fn register_event<E: Event + Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, E>;

    /// Registers the event in the Registry
    ///
    /// This event can now be sent over the network.
    /// You need to provide your own [`SerializeFns`] for this message
    fn register_event_custom_serde<E: Event + Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<E>,
    ) -> MessageRegistration<'_, E>;
}

impl AppEventExt for App {
    fn register_event<E: Event + Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, E> {
        self.register_event_internal(direction)
    }

    fn register_event_custom_serde<E: Event + Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<E>,
    ) -> MessageRegistration<'_, E> {
        self.register_event_internal_custom_serde(direction, serialize_fns)
    }
}
