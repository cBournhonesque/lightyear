use crate::receive::{
    ClearMessageFn, MessageReceiver, ReceiveLocalMessageFn, ReceiveMessageFn,
    ReleaseTimelineMessageFn,
};
use crate::receive_event::{ReceiveLocalTriggerFn, ReceiveTriggerFn, ReleaseTimelineTriggerFn};
use crate::send::{MessageSender, SendLocalMessageFn, SendMessageFn};
use crate::send_trigger::{SendLocalTriggerFn, SendTriggerFn};
use crate::{Message, MessageNetId};
use bevy_app::App;
use bevy_ecs::{component::ComponentId, entity::MapEntities, error::Result, resource::Resource};
use bevy_reflect::{Reflect, TypePath};
use bevy_utils::prelude::DebugName;
use core::any::TypeId;
use core::cell::UnsafeCell;
use core::hash::Hash;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::network::NetId;
use lightyear_core::prelude::{Tick, TimelineKind};
use lightyear_serde::entity_map::{ReceiveMapView, RemoteEntityMap, SendMapView};
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::{
    ContextDeserializeFn, ContextDeserializeFns, ContextSerializeFn, ContextSerializeFns,
    DeserializeFn, ErasedSerializeFns, SerializeFn, SerializeFns,
};
use lightyear_serde::writer::Writer;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_transport::channel::ChannelKind;
use lightyear_utils::collections::HashMap;
use lightyear_utils::registry::{RegistryHash, RegistryHasher, TypeKind, TypeMapper};
use serde::Serialize;
use serde::de::DeserializeOwned;
#[cfg(feature = "metrics")]
use std::sync::OnceLock;
#[allow(unused_imports)]
use tracing::{debug, trace};

#[derive(thiserror::Error, Debug)]
pub enum MessageError {
    #[error("the message if of the wrong type")]
    IncorrectType,
    #[error("message is not registered in the protocol")]
    NotRegistered,
    #[error("missing serialization functions for message")]
    MissingSerializationFns,
    #[error(transparent)]
    Serialization(#[from] SerializationError),
    #[error(transparent)]
    Packet(#[from] lightyear_transport::packet::error::PacketError),
    #[error("the component id {0:?} is missing from the entity")]
    MissingComponent(ComponentId),
    #[error("the channel kind {0:?} is missing from the entity")]
    MissingChannelKind(ChannelKind),
    #[error("the message kind {0:?} is not registered")]
    UnrecognizedMessage(MessageKind),
    #[error("the message id {0:?} is not registered")]
    UnrecognizedMessageId(MessageNetId),
    #[error("the delivery timeline {0:?} is not registered")]
    TimelineNotRegistered(TimelineKind),
    #[error("the receiving connection does not contain delivery timeline {0:?}")]
    MissingTimeline(TimelineKind),
    #[error(
        "delivery timeline at tick {current:?} is more than {max_lag_ticks} ticks behind payload target {target:?}"
    )]
    TimelineTooFarBehind {
        target: Tick,
        current: Tick,
        max_lag_ticks: u32,
    },
    #[error("timeline receiver reached its pending payload limit of {limit}")]
    PendingTimelineOverflow { limit: usize },
    #[error(transparent)]
    TransportError(#[from] lightyear_transport::error::TransportError),
}

/// [`MessageKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
pub struct MessageKind(TypeId);

impl MessageKind {
    #[inline(always)]
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

#[cfg(feature = "metrics")]
#[derive(Debug, Default, Clone)]
pub(crate) struct MessageMetricHandles {
    sent: OnceLock<metrics::Counter>,
    sent_bytes: OnceLock<metrics::Gauge>,
}

#[cfg(feature = "metrics")]
impl MessageMetricHandles {
    pub(crate) fn record_send<M: Message>(&self, bytes: usize) {
        self.sent
            .get_or_init(
                || metrics::counter!("message/send", "message" => core::any::type_name::<M>()),
            )
            .increment(1);
        self.sent_bytes
            .get_or_init(
                || metrics::gauge!("message/send_bytes", "message" => core::any::type_name::<M>()),
            )
            .increment(bytes as f64);
    }
}

#[derive(Debug, Clone)]
pub struct ReceiveMessageMetadata {
    /// ComponentId of the [`MessageReceiver<M>`] component (used if not a trigger)
    pub(crate) component_id: ComponentId,
    pub(crate) receive_message_fn: ReceiveMessageFn,
    pub(crate) receive_local_message_fn: ReceiveLocalMessageFn,
    pub(crate) message_clear_fn: ClearMessageFn,
    pub(crate) release_timeline_fn: ReleaseTimelineMessageFn,
}

#[derive(Debug, Clone, TypePath)]
pub(crate) struct SendMessageMetadata {
    /// ComponentId of the [`MessageSender<M>`] component
    pub(crate) component_id: ComponentId,
    pub(crate) send_message_fn: SendMessageFn,
    pub(crate) send_local_message_fn: SendLocalMessageFn,
}

#[derive(Debug, Clone, TypePath)]
pub(crate) struct SendTriggerMetadata {
    /// ComponentId of the [`TriggerSender<M>`](crate::send_trigger::EventSender) component
    pub(crate) component_id: ComponentId,
    pub(crate) send_trigger_fn: SendTriggerFn,
    pub(crate) send_local_trigger_fn: SendLocalTriggerFn,
}

#[derive(Debug, Clone, Copy, TypePath)]
pub(crate) struct ReceiveTriggerMetadata {
    pub(crate) component_id: ComponentId,
    pub(crate) receive_trigger_fn: ReceiveTriggerFn,
    pub(crate) receive_local_trigger_fn: ReceiveLocalTriggerFn,
    pub(crate) release_fn: ReleaseTimelineTriggerFn,
}

#[derive(Debug, Clone)]
pub(crate) enum MessageModeMetadata {
    Message {
        send: SendMessageMetadata,
        receive: ReceiveMessageMetadata,
    },
    Trigger {
        send: SendTriggerMetadata,
        receive: ReceiveTriggerMetadata,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct MessageMetadata {
    pub(crate) mode: MessageModeMetadata,
    pub(crate) serialize_fns: ErasedSerializeFns,
    #[cfg(feature = "metrics")]
    pub(crate) metrics: MessageMetricHandles,
}

impl MessageMetadata {
    pub(crate) fn send_component_id(&self) -> ComponentId {
        match &self.mode {
            MessageModeMetadata::Message { send, .. } => send.component_id,
            MessageModeMetadata::Trigger { send, .. } => send.component_id,
        }
    }

    pub(crate) fn receive_component_id(&self) -> ComponentId {
        match &self.mode {
            MessageModeMetadata::Message { receive, .. } => receive.component_id,
            MessageModeMetadata::Trigger { receive, .. } => receive.component_id,
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
///
/// You can provide a [`NetworkDirection`] to specify if the message should be sent from the client to the server, from the server to the client, or both.
/// Messages are sent through [`MessageSender<M>`] and read through
/// [`MessageReceiver<M>`]. Adding a [`NetworkDirection`] installs the sender as
/// a required component on the sending side. The receiving side gets its exact
/// typed receiver lazily when the first payload arrives.
///
///
/// ```rust
/// # use bevy_app::App;
/// # use serde::{Deserialize, Serialize};
/// # use lightyear_messages::prelude::*;
/// # use lightyear_connection::prelude::NetworkDirection;
///
/// #[derive(Serialize, Deserialize)]
/// struct MyMessage;
///
/// fn add_messages(app: &mut App) {
///   app.register_message::<MyMessage>()
///     .add_direction(NetworkDirection::ServerToClient);
/// }
/// ```
///
/// ### Customizing Message behaviour
///
/// There are some cases where you might want to define additional behaviour for a message.
/// For example, if the message contains Entities, you need to specify how those en
/// entities will be mapped from the remote world to the local world.
///
/// Provided that your type implements [`MapEntities`], you can extend the protocol to support this behaviour, by
/// calling the [`add_map_entities`](MessageRegistration::add_map_entities) method.
///
/// ```rust
/// # use bevy_app::App;
/// # use serde::{Deserialize, Serialize};
/// # use lightyear_messages::prelude::*;
/// # use lightyear_connection::prelude::NetworkDirection;
/// # use bevy_ecs::entity::{EntityMapper, Entity, MapEntities};
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
///   app.register_message::<MyMessage>()
///       .add_map_entities();
/// }
/// ```
#[derive(Debug, Default, Clone, Resource, TypePath)]
pub struct MessageRegistry {
    pub(crate) metadata: HashMap<MessageKind, MessageMetadata>,
    pub kind_map: TypeMapper<MessageKind>,
    hasher: RegistryHasher,
}

pub struct Context {
    registry: MessageRegistry,
    entity_mapper: UnsafeCell<RemoteEntityMap>,
}

fn mapped_context_serialize<M: MapEntities + Clone>(
    mapper: &SendMapView,
    message: &M,
    writer: &mut Writer,
    serialize_fn: SerializeFn<M>,
) -> core::result::Result<(), SerializationError> {
    let mut message = message.clone();
    // The view only exposes shared reads, so user mapping code runs without
    // requiring exclusive access to the entity map.
    let mut view = *mapper;
    message.map_entities(&mut view);
    serialize_fn(&message, writer)
}

fn mapped_context_deserialize<M: MapEntities>(
    mapper: &ReceiveMapView,
    reader: &mut Reader,
    deserialize_fn: DeserializeFn<M>,
) -> core::result::Result<M, SerializationError> {
    let mut message = deserialize_fn(reader)?;
    // The view only exposes shared reads, so user mapping code runs without
    // requiring exclusive access to the entity map.
    let mut view = *mapper;
    message.map_entities(&mut view);
    Ok(message)
}

impl MessageRegistry {
    pub(crate) fn register<M: Message, I: 'static>(
        &mut self,
        mode: MessageModeMetadata,
        serialize: ContextSerializeFns<M, I>,
        deserialize: ContextDeserializeFns<M, I>,
    ) {
        trace!("Registering message: {}", DebugName::type_name::<M>());
        let kind = self.kind_map.add::<I>();
        assert!(
            !self.metadata.contains_key(&kind),
            "message type {} is already registered",
            DebugName::type_name::<M>()
        );
        self.hasher.hash::<M>();

        let serialize_fns = ErasedSerializeFns::new::<M, I>(serialize, deserialize);

        let metadata = MessageMetadata {
            mode,
            serialize_fns,
            #[cfg(feature = "metrics")]
            metrics: MessageMetricHandles::default(),
        };

        self.metadata.insert(kind, metadata);
    }

    pub(crate) fn metadata(
        &self,
        kind: &MessageKind,
    ) -> core::result::Result<&MessageMetadata, MessageError> {
        self.metadata
            .get(kind)
            .ok_or(MessageError::UnrecognizedMessage(*kind))
    }

    pub(crate) fn is_map_entities<M: 'static>(&self) -> Result<bool> {
        let kind = MessageKind::of::<M>();
        let erased_fns = &self.metadata(&kind)?.serialize_fns;
        Ok(erased_fns.map_entities.is_some())
    }

    #[cfg(feature = "metrics")]
    pub(crate) fn metric_handles(
        &self,
        kind: &MessageKind,
    ) -> core::result::Result<&MessageMetricHandles, MessageError> {
        Ok(&self.metadata(kind)?.metrics)
    }

    pub(crate) fn add_map_entities<
        M: Clone + MapEntities + 'static,
        I: Clone + MapEntities + 'static,
    >(
        &mut self,
        context_serialize: ContextSerializeFn<M, I>,
        context_deserialize: ContextDeserializeFn<M, I>,
    ) {
        let kind = MessageKind::of::<I>();
        let erased_fns = self
            .metadata
            .get_mut(&kind)
            .expect("the message is not part of the protocol");
        let erased_fns = &mut erased_fns.serialize_fns;
        erased_fns.add_map_entities::<I>();
        erased_fns.context_serialize = unsafe { core::mem::transmute(context_serialize) };
        erased_fns.context_deserialize = unsafe { core::mem::transmute(context_deserialize) };
    }

    pub(crate) fn serialize<M: Message>(
        &self,
        message: &M,
        writer: &mut Writer,
        entity_map: &SendMapView,
    ) -> Result<(), MessageError> {
        let kind = MessageKind::of::<M>();
        let erased_fns = &self.metadata(&kind)?.serialize_fns;
        let net_id = self.kind_map.net_id(&kind).unwrap();
        net_id.to_bytes(writer)?;
        unsafe {
            erased_fns.serialize::<M, M>(message, writer, entity_map)?;
        }
        Ok(())
    }

    pub(crate) fn deserialize<M: Message>(
        &self,
        reader: &mut Reader,
        entity_map: &ReceiveMapView,
    ) -> Result<M, MessageError> {
        let net_id = NetId::from_bytes(reader)?;
        let kind = self
            .kind_map
            .kind(net_id)
            .ok_or(MessageError::NotRegistered)?;
        let erased_fns = &self.metadata(kind)?.serialize_fns;
        // SAFETY: the ErasedSerializeFns was created for the type M
        unsafe {
            erased_fns
                .deserialize::<M, M>(reader, entity_map)
                .map_err(Into::into)
        }
    }

    pub fn finish(&mut self) -> RegistryHash {
        self.hasher.finish()
    }
}

pub struct MessageRegistration<'a, M> {
    pub app: &'a mut App,
    pub(crate) _marker: core::marker::PhantomData<M>,
}

impl<'a, M: Message> MessageRegistration<'a, M> {
    #[cfg(feature = "test_utils")]
    pub fn new(app: &'a mut App) -> Self {
        Self {
            app,
            _marker: core::marker::PhantomData,
        }
    }

    /// Specify that the message contains entities which should be mapped from the remote world to the local world
    /// upon deserialization
    pub fn add_map_entities(&mut self) -> &mut Self
    where
        M: Clone + MapEntities + 'static,
    {
        let mut registry = self.app.world_mut().resource_mut::<MessageRegistry>();
        registry.add_map_entities::<M, M>(mapped_context_serialize, mapped_context_deserialize);
        self
    }

    /// Adds the sender component on each side that sends this message.
    ///
    /// Receiver components are inserted lazily when a payload arrives.
    pub fn add_direction(&mut self, direction: NetworkDirection) -> &mut Self {
        #[cfg(feature = "client")]
        self.add_client_direction(direction);
        #[cfg(feature = "server")]
        self.add_server_direction(direction);
        self
    }
}

/// Add messages or triggers to the list of types that can be sent.
pub trait AppMessageExt {
    /// Register a regular message type `M`.
    /// This registers the sender and default receiver component types. Calling
    /// [`MessageRegistration::add_direction`] installs senders as required
    /// components; receivers are inserted lazily on first receive.
    fn register_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
    ) -> MessageRegistration<'_, M>;

    fn is_message_registered<M: Message>(&self) -> bool;

    /// Register a regular message type `M` with custom serialization functions.
    fn register_message_custom_serde<M: Message>(
        &mut self,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M>;

    #[doc(hidden)]
    /// Register a regular message type `M` that uses `ToBytes` for serialization.
    fn register_message_to_bytes<M: Message + ToBytes>(&mut self) -> MessageRegistration<'_, M>;
}

impl AppMessageExt for App {
    fn register_message<M: Message + Serialize + DeserializeOwned>(
        &mut self,
    ) -> MessageRegistration<'_, M> {
        self.register_message_custom_serde::<M>(SerializeFns::<M>::default())
    }

    fn is_message_registered<M: Message>(&self) -> bool {
        self.world()
            .get_resource::<MessageRegistry>()
            .is_some_and(|r| r.kind_map.net_id(&MessageKind::of::<M>()).is_some())
    }

    fn register_message_custom_serde<M: Message>(
        &mut self,
        serialize_fns: SerializeFns<M>,
    ) -> MessageRegistration<'_, M> {
        if self
            .world_mut()
            .get_resource_mut::<MessageRegistry>()
            .is_none()
        {
            self.world_mut().init_resource::<MessageRegistry>();
        }
        // Register components for sending/receiving M
        let sender_id = self.world_mut().register_component::<MessageSender<M>>();
        let receiver_id = self.world_mut().register_component::<MessageReceiver<M>>();

        let mut registry = self.world_mut().resource_mut::<MessageRegistry>();
        registry.register::<M, M>(
            MessageModeMetadata::Message {
                send: SendMessageMetadata {
                    component_id: sender_id,
                    send_message_fn: MessageSender::<M>::send_message_typed,
                    send_local_message_fn: MessageSender::<M>::send_local_message_typed,
                },
                receive: ReceiveMessageMetadata {
                    component_id: receiver_id,
                    receive_message_fn: MessageReceiver::<M>::receive_message_typed,
                    receive_local_message_fn: MessageReceiver::<M>::receive_local_message_typed,
                    message_clear_fn: MessageReceiver::<M>::clear_typed,
                    release_timeline_fn: MessageReceiver::<M>::release_timeline_typed,
                },
            },
            ContextSerializeFns::new(serialize_fns.serialize),
            ContextDeserializeFns::new(serialize_fns.deserialize),
        );

        MessageRegistration {
            app: self,
            _marker: Default::default(),
        }
    }

    fn register_message_to_bytes<M: Message + ToBytes>(&mut self) -> MessageRegistration<'_, M> {
        self.register_message_custom_serde::<M>(SerializeFns::<M>::with_to_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::trigger::AppTriggerExt;
    use bevy_ecs::entity::{Entity, EntityMapper};
    use bevy_ecs::event::Event;
    use lightyear_serde::SerializationError;
    use lightyear_serde::entity_map::{ReceiveEntityMap, SendEntityMap};
    use lightyear_serde::reader::ReadInteger;
    use lightyear_serde::writer::WriteInteger;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message1(pub f32);

    /// Message where we provide our own serialization/deserialization functions
    #[derive(Debug, PartialEq, Clone, Reflect)]
    pub struct Message2(pub f32);

    pub(crate) fn serialize_message2(
        data: &Message2,
        writer: &mut Writer,
    ) -> core::result::Result<(), SerializationError> {
        writer.write_u32(data.0.to_bits())?;
        Ok(())
    }

    pub(crate) fn deserialize_message2(
        reader: &mut Reader,
    ) -> core::result::Result<Message2, SerializationError> {
        let data = f32::from_bits(reader.read_u32()?);
        Ok(Message2(data))
    }

    /// Message where we provide our own serialization/deserialization functions
    #[derive(Serialize, Deserialize, Debug, PartialEq, Clone, Reflect)]
    pub struct Message3(pub Entity);

    #[derive(Event, Serialize, Deserialize)]
    struct EventMessage;

    impl MapEntities for Message3 {
        fn map_entities<M: EntityMapper>(&mut self, entity_map: &mut M) {
            self.0 = entity_map.get_mapped(self.0);
        }
    }

    #[test]
    #[should_panic(expected = "is already registered")]
    fn message_and_trigger_registration_are_mutually_exclusive() {
        let mut app = App::new();
        app.register_message::<EventMessage>();
        app.register_event::<EventMessage>();
    }

    #[test]
    fn test_serde() {
        let mut app = App::new();
        app.register_message::<Message1>();
        let registry = app.world().resource::<MessageRegistry>();

        let message = Message1(1.0);
        let mut writer = Writer::default();
        let send_map = SendEntityMap::default();
        let send_view = SendMapView::local_only(&send_map);
        registry
            .serialize(&message, &mut writer, &send_view)
            .unwrap();
        let data = writer.into_bytes();

        let mut reader = Reader::from(data);
        let receive_map = ReceiveEntityMap::default();
        let receive_view = ReceiveMapView::local_only(&receive_map);
        let read = registry.deserialize(&mut reader, &receive_view).unwrap();
        assert_eq!(message, read);
    }

    #[test]
    fn test_custom_serde() {
        let mut app = App::new();
        app.register_message_custom_serde::<Message2>(SerializeFns {
            serialize: serialize_message2,
            deserialize: deserialize_message2,
        });
        let registry = app.world().resource::<MessageRegistry>();

        let message = Message2(1.0);
        let mut writer = Writer::default();
        let send_map = SendEntityMap::default();
        let send_view = SendMapView::local_only(&send_map);
        registry
            .serialize(&message, &mut writer, &send_view)
            .unwrap();
        let data = writer.into_bytes();

        let mut reader = Reader::from(data);
        let receive_map = ReceiveEntityMap::default();
        let receive_view = ReceiveMapView::local_only(&receive_map);
        let read = registry.deserialize(&mut reader, &receive_view).unwrap();
        assert_eq!(message, read);
    }

    #[test]
    fn test_entity_map() {
        let mut app = App::new();
        app.register_message::<Message3>().add_map_entities();
        let registry = app.world().resource::<MessageRegistry>();

        let message = Message3(Entity::from_bits(1));
        let mut writer = Writer::default();
        let mut entity_map = SendEntityMap::default();
        entity_map.set_mapped(Entity::from_bits(1), Entity::from_bits(2));
        let send_view = SendMapView::local_only(&entity_map);
        registry
            .serialize(&message, &mut writer, &send_view)
            .unwrap();
        let data = writer.into_bytes();

        let mut reader = Reader::from(data);
        let receive_map = ReceiveEntityMap::default();
        let receive_view = ReceiveMapView::local_only(&receive_map);
        let read = registry
            .deserialize::<Message3>(&mut reader, &receive_view)
            .unwrap();
        assert_eq!(read.0, Entity::from_bits(2));
    }
}
