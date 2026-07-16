use crate::receive::{
    BufferedMessageTimeline, ClearMessageFn, ClearPendingTimelineMessageFn,
    IntoMessageReceiverTimeline, MessageReceiver, ReceiveLocalMessageFn, ReceiveMessageFn,
    ReleaseTimelineMessageFn,
};
use crate::send::{MessageSender, SendLocalMessageFn, SendMessageFn};
use crate::{Message, MessageNetId};
use bevy_app::App;
use bevy_ecs::{
    component::ComponentId, entity::MapEntities, error::Result, ptr::Ptr, resource::Resource,
};
use bevy_reflect::{Reflect, TypePath};
use bevy_utils::prelude::DebugName;
use core::any::TypeId;
use core::cell::UnsafeCell;
use core::hash::Hash;
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::network::NetId;
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline, Tick};
use lightyear_serde::entity_map::{ReceiveEntityMap, RemoteEntityMap, SendEntityMap};
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
    #[error("the event is not registered for delivery timeline {0:?}")]
    MissingTimelineEventRegistration(TimelineKind),
    #[error("the receiving connection has no message receiver for delivery timeline {0:?}")]
    MissingTimelineMessageReceiver(TimelineKind),
    #[error(
        "timeline payload targets tick {target:?}, which is more than {max_future_ticks} ticks ahead of {current:?}"
    )]
    TimelineTooFarAhead {
        target: Tick,
        current: Tick,
        max_future_ticks: u32,
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

/// Runtime identifier for a [`NetworkTimeline`] registered for message delivery.
///
/// Timeline identifiers are part of the protocol and must be registered in the
/// same order on both peers.
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
pub struct TimelineKind(TypeId);

impl TimelineKind {
    /// Returns the runtime identifier for timeline `T`.
    #[inline]
    pub fn of<T: NetworkTimeline>() -> Self {
        Self(TypeId::of::<T>())
    }
}

impl From<TypeId> for TimelineKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}

impl From<TypeId> for MessageKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}

/// Identifies the receiver component for a message and delivery timeline.
/// `timeline == None` is the default immediate [`LocalTimeline`]
/// receiver.
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
pub(crate) struct MessageReceiverKind {
    pub(crate) message: MessageKind,
    pub(crate) timeline: Option<TimelineKind>,
}

impl MessageReceiverKind {
    pub(crate) fn new(message: MessageKind, timeline: Option<TimelineKind>) -> Self {
        Self { message, timeline }
    }

    pub(crate) fn of<M: Message, T: IntoMessageReceiverTimeline>() -> Self {
        Self::new(MessageKind::of::<M>(), T::timeline_kind())
    }
}

use crate::receive_event::{
    ClearTimelineTriggerFn, ReceiveLocalTimelineTriggerFn, ReceiveLocalTriggerFn,
    ReceiveTimelineTriggerFn, ReceiveTriggerFn, ReleaseTimelineTriggerFn,
};
use crate::send_trigger::{SendLocalTriggerFn, SendTriggerFn};

#[derive(Debug, Clone)]
pub struct ReceiveMessageMetadata {
    /// ComponentId of the [`MessageReceiver<M>`] component (used if not a trigger)
    pub(crate) component_id: ComponentId,
    pub(crate) receive_message_fn: ReceiveMessageFn,
    pub(crate) receive_local_message_fn: ReceiveLocalMessageFn,
    pub(crate) message_clear_fn: ClearMessageFn,
    pub(crate) timeline: Option<TimelineReceiverMetadata>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct TimelineReceiverMetadata {
    pub(crate) release_fn: ReleaseTimelineMessageFn,
    pub(crate) clear_fn: ClearPendingTimelineMessageFn,
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
pub(crate) struct ImmediateTriggerMetadata {
    pub(crate) receive_trigger_fn: ReceiveTriggerFn,
    pub(crate) receive_local_trigger_fn: ReceiveLocalTriggerFn,
}

#[derive(Debug, Clone, Copy, TypePath)]
pub(crate) struct TimelineTriggerMetadata {
    pub(crate) component_id: ComponentId,
    pub(crate) receive_trigger_fn: ReceiveTimelineTriggerFn,
    pub(crate) receive_local_trigger_fn: ReceiveLocalTimelineTriggerFn,
    pub(crate) release_fn: ReleaseTimelineTriggerFn,
    pub(crate) clear_fn: ClearTimelineTriggerFn,
}

#[derive(Debug, Clone, Copy, TypePath)]
pub(crate) enum ReceiveTriggerMetadata {
    Immediate(ImmediateTriggerMetadata),
    Timeline(TimelineTriggerMetadata),
}

#[derive(Debug, Clone)]
pub(crate) struct TimelineMetadata {
    pub(crate) component_id: ComponentId,
    pub(crate) tick_fn: unsafe fn(Ptr<'_>) -> Tick,
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
    pub(crate) send_metadata: HashMap<MessageKind, SendMessageMetadata>,
    pub(crate) send_trigger_metadata: HashMap<MessageKind, SendTriggerMetadata>,
    pub(crate) receive_metadata: HashMap<MessageReceiverKind, ReceiveMessageMetadata>,
    pub(crate) receive_trigger: HashMap<MessageReceiverKind, ReceiveTriggerMetadata>,
    pub serialize_fns_map: HashMap<MessageKind, ErasedSerializeFns>,
    pub kind_map: TypeMapper<MessageKind>,
    pub(crate) timeline_metadata: HashMap<TimelineKind, TimelineMetadata>,
    hasher: RegistryHasher,
}

pub struct Context {
    registry: MessageRegistry,
    entity_mapper: UnsafeCell<RemoteEntityMap>,
}

fn mapped_context_serialize<M: MapEntities + Clone>(
    mapper: &mut SendEntityMap,
    message: &M,
    writer: &mut Writer,
    serialize_fn: SerializeFn<M>,
) -> core::result::Result<(), SerializationError> {
    let mut message = message.clone();
    message.map_entities(mapper);
    serialize_fn(&message, writer)
}

fn mapped_context_deserialize<M: MapEntities>(
    mapper: &mut ReceiveEntityMap,
    reader: &mut Reader,
    deserialize_fn: DeserializeFn<M>,
) -> core::result::Result<M, SerializationError> {
    let mut message = deserialize_fn(reader)?;
    message.map_entities(mapper);
    Ok(message)
}

impl MessageRegistry {
    pub(crate) fn register_timeline<T: NetworkTimeline>(&mut self, component_id: ComponentId) {
        let kind = TimelineKind::of::<T>();
        if self.timeline_metadata.contains_key(&kind) {
            return;
        }
        self.hasher.hash::<T>();
        self.timeline_metadata.insert(
            kind,
            TimelineMetadata {
                component_id,
                tick_fn: |ptr| {
                    // SAFETY: this function is stored with the component id for T.
                    let timeline = unsafe { ptr.deref::<T>() };
                    timeline.tick()
                },
            },
        );
    }

    pub(crate) fn register_message<M: Message, I: 'static>(
        &mut self,
        serialize: ContextSerializeFns<SendEntityMap, M, I>,
        deserialize: ContextDeserializeFns<ReceiveEntityMap, M, I>,
    ) {
        trace!("Registering message: {}", DebugName::type_name::<M>());
        self.hasher.hash::<M>();
        let message_kind = self.kind_map.add::<I>();
        self.serialize_fns_map.insert(
            message_kind,
            ErasedSerializeFns::new::<SendEntityMap, ReceiveEntityMap, M, I>(
                serialize,
                deserialize,
            ),
        );
    }

    pub(crate) fn register_sender<M: Message>(&mut self, component_id: ComponentId) {
        self.send_metadata.insert(
            MessageKind::of::<M>(),
            SendMessageMetadata {
                component_id,
                send_message_fn: MessageSender::<M>::send_message_typed,
                send_local_message_fn: MessageSender::<M>::send_local_message_typed,
            },
        );
    }

    pub(crate) fn register_receiver<M, T>(&mut self, component_id: ComponentId)
    where
        M: Message,
        T: IntoMessageReceiverTimeline,
    {
        self.receive_metadata.insert(
            MessageReceiverKind::of::<M, T>(),
            ReceiveMessageMetadata {
                component_id,
                receive_message_fn: MessageReceiver::<M, T>::receive_message_typed,
                receive_local_message_fn: MessageReceiver::<M, T>::receive_local_message_typed,
                message_clear_fn: MessageReceiver::<M, T>::clear_typed,
                timeline: T::timeline_kind().map(|_| TimelineReceiverMetadata {
                    release_fn: MessageReceiver::<M, T>::release_timeline_typed,
                    clear_fn: MessageReceiver::<M, T>::clear_pending_timelines_typed,
                }),
            },
        );
    }

    pub(crate) fn is_map_entities<M: 'static>(&self) -> Result<bool> {
        let kind = MessageKind::of::<M>();
        let erased_fns = self
            .serialize_fns_map
            .get(&kind)
            .ok_or(MessageError::MissingSerializationFns)?;
        Ok(erased_fns.map_entities.is_some())
    }

    pub(crate) fn add_map_entities<
        M: Clone + MapEntities + 'static,
        I: Clone + MapEntities + 'static,
    >(
        &mut self,
        context_serialize: ContextSerializeFn<SendEntityMap, M, I>,
        context_deserialize: ContextDeserializeFn<ReceiveEntityMap, M, I>,
    ) {
        let kind = MessageKind::of::<I>();
        let erased_fns = self
            .serialize_fns_map
            .get_mut(&kind)
            .expect("the message is not part of the protocol");
        erased_fns.add_map_entities::<I>();
        erased_fns.context_serialize = unsafe { core::mem::transmute(context_serialize) };
        erased_fns.context_deserialize = unsafe { core::mem::transmute(context_deserialize) };
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
        unsafe {
            erased_fns.serialize::<SendEntityMap, M, M>(message, writer, entity_map)?;
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
        unsafe {
            erased_fns
                .deserialize::<ReceiveEntityMap, M, M>(reader, entity_map)
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

    /// Send and receive this message on channels delivered by timeline `T`.
    ///
    /// The receiving connection lazily gets a distinct [`MessageReceiver<M, T>`]
    /// when its first payload arrives.
    /// The timeline itself must also be registered with
    /// [`register_message_timeline`](crate::plugin::register_message_timeline).
    pub fn add_direction_on_timeline<T>(&mut self, direction: NetworkDirection) -> &mut Self
    where
        T: BufferedMessageTimeline,
    {
        let receiver_id = self
            .app
            .world_mut()
            .register_component::<MessageReceiver<M, T>>();
        self.app
            .world_mut()
            .resource_mut::<MessageRegistry>()
            .register_receiver::<M, T>(receiver_id);

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
        // Register M for serialization/deserialization
        registry.register_message::<M, M>(
            ContextSerializeFns::new(serialize_fns.serialize),
            ContextDeserializeFns::new(serialize_fns.deserialize),
        );
        // Register sender/receiver metadata for M, ensuring trigger_fn is None
        registry.register_sender::<M>(sender_id);
        registry.register_receiver::<M, LocalTimeline>(receiver_id);

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
    use bevy_ecs::entity::{Entity, EntityMapper};
    use lightyear_serde::SerializationError;
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

    impl MapEntities for Message3 {
        fn map_entities<M: EntityMapper>(&mut self, entity_map: &mut M) {
            self.0 = entity_map.get_mapped(self.0);
        }
    }

    #[test]
    fn test_serde() {
        let mut registry = MessageRegistry::default();
        registry.kind_map.add::<Message1>();
        registry.serialize_fns_map.insert(
            MessageKind::of::<Message1>(),
            ErasedSerializeFns::new(
                ContextSerializeFns::<(), _>::new(SerializeFns::<Message1>::default().serialize),
                ContextDeserializeFns::<(), _>::new(
                    SerializeFns::<Message1>::default().deserialize,
                ),
            ),
        );

        let message = Message1(1.0);
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
    fn test_custom_serde() {
        let mut registry = MessageRegistry::default();
        registry.register_message::<Message2, _>(
            ContextSerializeFns::new(serialize_message2),
            ContextDeserializeFns::new(deserialize_message2),
        );

        let message = Message2(1.0);
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
    fn test_entity_map() {
        let mut registry = MessageRegistry::default();
        registry.kind_map.add::<Message3>();
        registry.serialize_fns_map.insert(
            MessageKind::of::<Message3>(),
            ErasedSerializeFns::new(
                ContextSerializeFns::<SendEntityMap, _>::new(
                    SerializeFns::<Message3>::default().serialize,
                ),
                ContextDeserializeFns::<ReceiveEntityMap, _>::new(
                    SerializeFns::<Message3>::default().deserialize,
                ),
            ),
        );
        registry.add_map_entities(
            mapped_context_serialize::<Message3>,
            mapped_context_deserialize::<Message3>,
        );

        let message = Message3(Entity::from_bits(1));
        let mut writer = Writer::default();
        let mut entity_map = SendEntityMap::default();
        entity_map.set_mapped(Entity::from_bits(1), Entity::from_bits(2));
        registry
            .serialize(&message, &mut writer, &mut entity_map)
            .unwrap();
        let data = writer.to_bytes();

        let mut reader = Reader::from(data);
        let read = registry
            .deserialize::<Message3>(&mut reader, &mut ReceiveEntityMap::default())
            .unwrap();
        assert_eq!(read.0, Entity::from_bits(2));
    }
}
