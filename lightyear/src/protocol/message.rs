use anyhow::Context;
use bevy::app::PreUpdate;
use bevy::ecs::entity::MapEntities;
use std::any::TypeId;
use std::fmt::Debug;

use crate::_internal::{ReadBuffer, ReadWordBuffer, WriteBuffer, WriteWordBuffer};
use crate::client::config::ClientConfig;
use crate::client::message::add_server_to_client_message;
use crate::prelude::{client, server, AppComponentExt, Channel, RemoteEntityMap};
use bevy::prelude::{
    App, Component, EntityMapper, EventWriter, IntoSystemConfigs, ResMut, Resource, TypePath, World,
};
use bevy::reflect::Map;
use bevy::utils::HashMap;
use bitcode::encoding::Fixed;
use bitcode::{Decode, Encode};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tracing::{debug, error};

use crate::inputs::native::input_buffer::InputMessage;
use crate::packet::message::Message;
use crate::prelude::server::ServerConfig;
use crate::prelude::{ChannelDirection, ChannelKind, MainSet};
use crate::protocol::component::ComponentKind;
use crate::protocol::registry::{NetId, TypeKind, TypeMapper};
use crate::protocol::serialize::{ErasedSerializeFns, MapEntitiesFn};
use crate::protocol::{BitSerializable, EventContext};
use crate::serialize::RawData;
use crate::server::message::add_client_to_server_message;
use crate::shared::replication::entity_map::EntityMap;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum MessageType {
    /// This is a message for a [`LeafwingUserAction`](crate::inputs::leafwing::LeafwingUserAction)
    #[cfg(feature = "leafwing")]
    LeafwingInput,
    /// This is a message for a [`UserAction`](crate::inputs::native::UserAction)
    NativeInput,
    /// This is not an input message, but a regular [`Message`]
    Normal,
}

#[derive(Debug, Default, Clone, Resource, PartialEq, TypePath)]
pub struct MessageRegistry {
    typed_map: HashMap<MessageKind, MessageType>,
    serialize_fns_map: HashMap<MessageKind, ErasedSerializeFns>,
    pub(crate) kind_map: TypeMapper<MessageKind>,
}

fn register_message_send<M: Message>(app: &mut App, direction: ChannelDirection) {
    let is_client = app.world.get_resource::<ClientConfig>().is_some();
    let is_server = app.world.get_resource::<ServerConfig>().is_some();
    match direction {
        ChannelDirection::ClientToServer => {
            if is_server {
                add_client_to_server_message::<M>(app);
            }
        }
        ChannelDirection::ServerToClient => {
            if is_client {
                add_server_to_client_message::<M>(app);
            }
        }
        ChannelDirection::Bidirectional => {
            register_message_send::<M>(app, ChannelDirection::ClientToServer);
            register_message_send::<M>(app, ChannelDirection::ServerToClient);
        }
    }
}

pub struct MessageRegistration<'a> {
    app: &'a mut App,
}

impl MessageRegistration<'_> {
    /// Specify that the message contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    pub fn add_map_entities<M: MapEntities + 'static>(self) -> Self {
        self.app.add_message_map_entities::<M>();
        self
    }
}

/// Add a message to the list of messages that can be sent
pub trait AppMessageExt {
    /// Registers the message in the Registry
    /// This message can now be sent over the network.
    fn add_message<M: Message>(&mut self, direction: ChannelDirection);

    /// Specify that the message contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    fn add_message_map_entities<M: MapEntities + 'static>(&mut self);
}

impl AppMessageExt for App {
    fn add_message<M: Message>(&mut self, direction: ChannelDirection) {
        let mut registry = self.world.resource_mut::<MessageRegistry>();
        if !registry.is_registered::<M>() {
            registry.add_message::<M>(MessageType::Normal);
        }
        debug!("register message {}", std::any::type_name::<M>());
        register_message_send::<M>(self, direction);
    }

    /// Specify that the message contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    fn add_message_map_entities<M: MapEntities + 'static>(&mut self) {
        let mut registry = self.world.resource_mut::<MessageRegistry>();
        registry.add_map_entities::<M>();
    }
}

impl MessageRegistry {
    pub(crate) fn message_type(&self, net_id: NetId) -> MessageType {
        let kind = self.kind_map.kind(net_id).unwrap();
        self.typed_map
            .get(kind)
            .map_or(MessageType::Normal, |message_type| *message_type)
    }

    pub fn is_registered<M: 'static>(&self) -> bool {
        self.kind_map.net_id(&MessageKind::of::<M>()).is_some()
    }

    pub(crate) fn add_message<M: Message>(&mut self, message_type: MessageType) {
        let message_kind = self.kind_map.add::<M>();
        self.serialize_fns_map
            .insert(message_kind, ErasedSerializeFns::new::<M>());
        self.typed_map.insert(message_kind, message_type);
    }

    pub(crate) fn try_add_map_entities<M: MapEntities + 'static>(&mut self) {
        let kind = MessageKind::of::<M>();
        if let Some(erased_fns) = self.serialize_fns_map.get_mut(&kind) {
            erased_fns.add_map_entities::<M>();
        }
    }

    pub(crate) fn add_map_entities<M: MapEntities + 'static>(&mut self) {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get_mut(&kind)
            .expect("the message is not part of the protocol");
        erased_fns.add_map_entities::<M>();
    }

    pub(crate) fn serialize<M: Message>(
        &self,
        message: &M,
        writer: &mut WriteWordBuffer,
    ) -> anyhow::Result<RawData> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .context("the message is not part of the protocol")?;
        let net_id = self.kind_map.net_id(&kind).unwrap();
        writer.start_write();
        writer.encode(net_id, Fixed)?;
        erased_fns.serialize(message, writer)?;
        Ok(writer.finish_write().to_vec())
    }

    pub(crate) fn deserialize<M: Message>(
        &self,
        reader: &mut ReadWordBuffer,
        entity_map: &mut EntityMap,
    ) -> anyhow::Result<M> {
        let net_id = reader.decode::<NetId>(Fixed)?;
        let kind = self.kind_map.kind(net_id).context("unknown message kind")?;
        let erased_fns = self
            .serialize_fns_map
            .get(kind)
            .context("the message is not part of the protocol")?;
        erased_fns.deserialize(reader, entity_map)
    }
}

/// [`MessageKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct MessageKind(TypeId);

impl MessageKind {
    pub fn of<M: 'static>() -> Self {
        Self(TypeId::of::<M>())
    }
}

impl TypeKind for MessageKind {}

impl From<TypeId> for MessageKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}
