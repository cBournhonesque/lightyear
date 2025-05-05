use super::{client, server};
use crate::client::config::ClientConfig;
use crate::prelude::{ChannelDirection, Message};
use crate::protocol::message::{MessageError, MessageKind};
use crate::protocol::registry::{NetId, TypeMapper};
use crate::protocol::serialize::ErasedSerializeFns;
use crate::protocol::SerializeFns;
use crate::serialize::reader::Reader;
use crate::serialize::writer::Writer;
use crate::serialize::ToBytes;
use crate::server::config::ServerConfig;
use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
use bevy::ecs::entity::MapEntities;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::debug;

pub struct MessageRegistration<'a, M> {
    pub(crate) app: &'a mut App,
    pub(crate) _marker: core::marker::PhantomData<M>,
}

impl<M> MessageRegistration<'_, M> {
    /// Specify that the message contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    pub fn add_map_entities(self) -> Self
    where
        M: Clone + MapEntities + 'static,
    {
        let mut registry = self.app.world_mut().resource_mut::<MessageRegistry>();
        registry.add_map_entities::<M>();
        self
    }
}

pub(crate) trait AppMessageInternalExt {
    /// Function used internally to register a Message with a specific [`MessageType`]
    fn register_message_internal<M: Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, M>;

    /// Function used internally to register a Message with a specific [`MessageType`]
    /// and a custom [`SerializeFns`] implementation
    fn register_message_internal_custom_serde<M: Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M>;
}

impl AppMessageInternalExt for App {
    fn register_message_internal<M: Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, M> {
        self.register_message_internal_custom_serde::<M>(direction, SerializeFns::<M>::default())
    }

    fn register_message_internal_custom_serde<M: Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M> {
        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        if !registry.is_registered::<M>() {
            let message_kind = registry.kind_map.add::<M>();
            registry.serialize_fns_map.insert(
                message_kind,
                ErasedSerializeFns::new_custom_serde::<M>(serialize_fns),
            );
        }
        debug!("register message {}", core::any::type_name::<M>());
        register_message::<M>(self, direction);
        MessageRegistration {
            app: self,
            _marker: core::marker::PhantomData,
        }
    }
}

/// Add a message to the list of messages that can be sent
pub trait AppMessageExt {
    /// Registers the message in the Registry
    /// This message can now be sent over the network.
    fn register_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, M>;

    /// Registers the message in the Registry
    ///
    /// This message can now be sent over the network.
    /// You need to provide your own [`SerializeFns`] for this message
    fn register_message_custom_serde<M: Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M>;
}

impl AppMessageExt for App {
    fn register_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
        direction: ChannelDirection,
    ) -> MessageRegistration<'_, M> {
        self.register_message_internal(direction)
    }

    fn register_message_custom_serde<M: Message>(
        &mut self,
        direction: ChannelDirection,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M> {
        self.register_message_internal_custom_serde(direction, serialize_fns)
    }
}

/// Register the message-receive metadata for a given message M
pub(crate) fn register_message<M: Message>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world().get_resource::<ClientConfig>().is_some();
    let is_server = app.world().get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_server {
                MessageRegistry::register_server_receive::<M>(app);
            };
            if is_client {
                MessageRegistry::register_client_send::<M>(app);
            };
        }
        ChannelDirection::ServerToClient => {
            if is_client {
                MessageRegistry::register_client_receive::<M>(app);
            };
            if is_server {
                MessageRegistry::register_server_send::<M>(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_message::<M>(app, ChannelDirection::ClientToServer);
            register_message::<M>(app, ChannelDirection::ServerToClient);
        }
    }
}

/// A [`Resource`] that will keep track of all the [`Message`]s that can be sent over the network.
/// A [`Message`] is any type that is serializable and deserializable.
///
///
/// ### Adding Messages
///
/// You register messages by calling the [`add_message`](AppMessageExt::register_message) method directly on the App.
/// You can provide a [`ChannelDirection`] to specify if the message should be sent from the client to the server, from the server to the client, or both.
///
/// ```rust
/// use bevy::prelude::*;
/// use serde::{Deserialize, Serialize};
/// use lightyear::prelude::*;
///
/// #[derive(Serialize, Deserialize)]
/// struct MyMessage;
///
/// fn add_messages(app: &mut App) {
///   app.register_message::<MyMessage>(ChannelDirection::Bidirectional);
/// }
/// ```
///
/// ### Customizing Message behaviour
///
/// There are some cases where you might want to define additional behaviour for a message.
/// For example, if the message contains [`Entities`](Entity), you need to specify how those en
/// entities will be mapped from the remote world to the local world.
///
/// Provided that your type implements [`MapEntities`], you can extend the protocol to support this behaviour, by
/// calling the [`add_map_entities`](MessageRegistration::add_map_entities) method.
///
/// ```rust
/// use bevy::ecs::entity::{EntityMapper, MapEntities};
/// use bevy::prelude::*;
/// use serde::{Deserialize, Serialize};
/// use lightyear::prelude::*;
///
/// #[derive(Serialize, Deserialize, Clone)]
/// struct MyMessage(Entity);
///
/// impl MapEntities for MyMessage {
///    fn map_entities<M: EntityMapper>(&mut self, entity_map: &mut M) {
///        self.0 = entity_map.get_mapped(self.0);
///    }
/// }
///
/// fn add_messages(app: &mut App) {
///   app.register_message::<MyMessage>(ChannelDirection::Bidirectional)
///       .add_map_entities();
/// }
/// ```
#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct MessageRegistry {
    // TODO: do we need to distinguish between message types?
    pub(crate) client_messages: client::MessageMetadata,
    pub(crate) server_messages: server::MessageMetadata,
    pub(crate) serialize_fns_map: HashMap<MessageKind, ErasedSerializeFns>,
    pub(crate) kind_map: TypeMapper<MessageKind>,
}

impl MessageRegistry {
    pub fn is_registered<M: 'static>(&self) -> bool {
        self.kind_map.net_id(&MessageKind::of::<M>()).is_some()
    }

    pub(crate) fn add_message_custom_serde<M: Message>(&mut self, serialize_fns: SerializeFns<M>) {
        let message_kind = self.kind_map.add::<M>();
        self.serialize_fns_map.insert(
            message_kind,
            ErasedSerializeFns::new_custom_serde::<M>(serialize_fns),
        );
    }

    pub(crate) fn try_add_map_entities<M: Clone + MapEntities + 'static>(&mut self) {
        let kind = MessageKind::of::<M>();
        if let Some(erased_fns) = self.serialize_fns_map.get_mut(&kind) {
            erased_fns.add_map_entities::<M>();
        }
    }

    pub(crate) fn add_map_entities<M: Clone + MapEntities + 'static>(&mut self) {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get_mut(&kind)
            .expect("the message is not part of the protocol");
        erased_fns.add_map_entities::<M>();
    }

    /// Returns true if we have a registered `map_entities` function for this message type
    pub(crate) fn is_map_entities<M: 'static>(&self) -> bool {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .expect("the message is not part of the protocol");
        erased_fns.map_entities.is_some()
    }

    pub(crate) fn serialize<M: Message>(
        &self,
        message: &M,
        writer: &mut Writer,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        let net_id = self.kind_map.net_id(&kind).unwrap();
        net_id.to_bytes(writer)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe {
            erased_fns.serialize(message, writer, entity_map)?;
        }
        Ok(())
    }

    pub(crate) fn deserialize<M: Message>(
        &self,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<M, MessageError> {
        let net_id = NetId::from_bytes(reader)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(MessageError::NotRegistered)?;
        let erased_fns = self
            .serialize_fns_map
            .get(kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe { erased_fns.deserialize(reader, entity_map) }.map_err(Into::into)
    }

    /// Deserialize the message bytes
    /// (We have already deserialized the NetId)
    pub(crate) fn raw_deserialize<M: Message>(
        &self,
        reader: &mut Reader,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<M, MessageError> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe { erased_fns.deserialize(reader, entity_map) }.map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::MessageRegistry;
    use crate::protocol::message::MessageKind;
    use crate::protocol::serialize::ErasedSerializeFns;
    use crate::protocol::SerializeFns;
    use crate::serialize::reader::Reader;
    use crate::serialize::writer::Writer;
    use crate::shared::replication::entity_map::{ReceiveEntityMap, SendEntityMap};
    use crate::tests::protocol::{
        deserialize_resource2, serialize_resource2, ComponentMapEntities, Resource1, Resource2,
    };
    use bevy::prelude::Entity;

    #[test]
    fn test_serde() {
        let mut registry = MessageRegistry::default();
        registry.kind_map.add::<Resource1>();
        registry.serialize_fns_map.insert(
            MessageKind::of::<Resource1>(),
            ErasedSerializeFns::new::<Resource1>(),
        );

        let message = Resource1(1.0);
        let mut writer = Writer::default();
        registry
            .serialize(&message, &mut writer, &mut SendEntityMap::default())
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(message, read);
    }

    #[test]
    fn test_serde_map() {
        let mut registry = MessageRegistry::default();
        registry.kind_map.add::<ComponentMapEntities>();
        registry.serialize_fns_map.insert(
            MessageKind::of::<ComponentMapEntities>(),
            ErasedSerializeFns::new::<ComponentMapEntities>(),
        );
        registry.add_map_entities::<ComponentMapEntities>();

        let message = ComponentMapEntities(Entity::from_raw(0));
        let mut writer = Writer::default();
        let mut map = SendEntityMap::default();
        map.insert(Entity::from_raw(0), Entity::from_raw(1));
        registry.serialize(&message, &mut writer, &mut map).unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize::<ComponentMapEntities>(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(read, ComponentMapEntities(Entity::from_raw(1)));
    }

    #[test]
    fn test_custom_serde() {
        let mut registry = MessageRegistry::default();
        registry.add_message_custom_serde::<Resource2>(SerializeFns {
            serialize: serialize_resource2,
            deserialize: deserialize_resource2,
        });

        let message = Resource2(1.0);
        let mut writer = Writer::default();
        registry
            .serialize(&message, &mut writer, &mut SendEntityMap::default())
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(message, read);
    }
}
