use crate::plugin::MessagePlugin;
use crate::prelude::MessageSender;
use crate::registry::{MessageError, MessageKind, MessageRegistry};
use crate::send::Priority;
pub(crate) use crate::trigger::TriggerMessage;
use crate::{MessageManager, MessageNetId};
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::{DeferredWorld, FilteredEntityMut};
use bevy::prelude::{Commands, Component, Entity, Event, Query, Res, World};
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_serde::writer::Writer;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::{Channel, ChannelKind};
use lightyear_transport::prelude::Transport;
use std::sync::Arc;
use tracing::{error, trace};

/// Component used to send triggers of type `M` remotely.
/// This wraps a `MessageSender<TriggerMessage<M>>`.
#[derive(Component, Default)]
#[require(MessageManager)] // Requires MessageManager like MessageSender
#[component(on_add = MessageSender::<TriggerMessage<M>>::on_add_hook)]
pub struct TriggerSender<M: Event> {
    send: Vec<(TriggerMessage<M>, ChannelKind, Priority)>,
    writer: Writer,
}

impl <M: Event> TriggerSender<M> {
    /// Creates a new `TriggerSender` component.
    pub fn new() -> Self {
        Self {
            send: Vec::new(),
            writer: Writer::default(),
        }
    }

    /// Take all messages from the TriggerSender<M>, serialize them, and buffer them
    /// on the appropriate ChannelSender<C>
    ///
    /// SAFETY: the `trigger_sender` must be of type `TriggerSender<M>`
    pub(crate) unsafe fn send_trigger_typed(
        trigger_sender: MutUntyped,
        net_id: MessageNetId,
        transport: &Transport,
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY:  the `trigger_sender` must be of type `TriggerSender<M>`
        let mut sender = unsafe { trigger_sender.with_type::<Self>()};
        // enable split borrows
        let sender = &mut *sender;
        sender.send.drain(..).try_for_each(|(message, channel_kind, priority)| {
            // we write the message NetId, and then serialize the message
            net_id.to_bytes(&mut sender.writer)?;
            serialize_metadata.serialize::<SendEntityMap, TriggerMessage<M>, M>(&message, &mut sender.writer,entity_map)?;
            let bytes = sender.writer.split();
            trace!("Sending message of type {:?} with net_id {net_id:?} on channel {channel_kind:?}", core::any::type_name::<M>());
            transport.send_erased(channel_kind, bytes, priority)?;
            Ok(())
        })
    }

    pub fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let mut entity_mut = world
                .entity_mut(context.entity);
            let mut message_manager = entity_mut
                .get_mut::<MessageManager>()
                .unwrap();
            let message_kind_present = message_manager
                .send_messages
                .iter()
                .any(|(message_kind, _)| {
                    *message_kind == MessageKind::of::<M>()
                });
            if !message_kind_present {
                message_manager.send_messages.push((MessageKind::of::<M>(), context.component_id));
            }
        })
    }
}


// SAFETY: the sender must correspond to the correct `TriggerSender<M>` type
pub(crate) type SendTriggerFn = unsafe fn(
    sender: MutUntyped,
    message_net_id: MessageNetId,
    transport: &Transport,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut SendEntityMap,
) -> Result<(), MessageError>;


impl<M: Event> TriggerSender<M> {
    /// Buffers a trigger `M` to be sent over the specified channel to the target entities.
    pub fn trigger<C: Channel>(
        &mut self,
        trigger: M,
    ) {
        self.trigger_targets::<C>(trigger, vec![]);
    }

    /// Buffers a trigger `M` to be sent over the specified channel to the target entities.
    pub fn trigger_targets<C: Channel>(
        &mut self,
        trigger: M,
        targets: impl IntoIterator<Item = Entity>,
    ) {
        let message = TriggerMessage {
            trigger,
            target_entities: targets.into_iter().collect(),
        };
        // Use the inner MessageSender to send the TriggerMessage
        // Use a default priority for now, could be configurable
        self.send.push((message, ChannelKind::of::<C>(), 1.0));
    }
}


impl MessagePlugin {
    /// Take messages to send from the MessageSender<M> components
    /// Serialize them into bytes that are buffered in a ChannelSender<C>
    pub fn send_trigger(
        // TODO: maybe prevent users from sending messages if Connecting/Disconnected is present?
        //   but then this crate would import lightyear_connection; and we might want to remain independent
        //   or should we have a feature called 'connection'?
        mut transport_query: Query<(Entity, &Transport, &mut MessageManager)>,
        // TriggerSender<M> present on that entity
        message_sender_query: Query<FilteredEntityMut>,
        registry: Res<MessageRegistry>,
    ) {
        // We use Arc to make the query Clone, since we know that we will only access MessageSender<M> components
        // on different entities
        let mut message_sender_query = Arc::new(message_sender_query);

        transport_query.par_iter_mut().for_each(|(entity, transport, mut message_manager)| {
            // SAFETY: we know that this won't lead to violating the aliasing rule
            let mut message_sender_query = unsafe { message_sender_query.reborrow_unsafe() };

            // TODO: allow sending from senders in parallel! The only issue is the mutable borrow of the entity mapper
            // enable split borrows
            let message_manager = &mut *message_manager;
            message_manager.send_triggers.iter().try_for_each(|(message_kind, sender_id)| {
                let mut entity_mut = message_sender_query.get_mut(entity).unwrap();
                let message_sender = entity_mut.get_mut_by_id(*sender_id).ok_or(MessageError::MissingComponent(*sender_id))?;
                let send_metadata = registry.send_metadata.get(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                let serialize_fns = registry.serialize_fns_map.get(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                let message_id = registry.kind_map.net_id(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                // SAFETY: we know the message_sender corresponds to the correct `MessageSender<M>` type
                unsafe { (send_metadata.send_message_fn)(
                    message_sender,
                    *message_id,
                    transport,
                    serialize_fns,
                    &mut message_manager.entity_mapper.local_to_remote,
                )?; }
                Ok::<_, MessageError>(())
            }).inspect_err(|e| error!("error sending message: {e:?}")).ok();
        })
    }
}

