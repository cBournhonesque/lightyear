use crate::prelude::RemoteEvent;
use crate::registry::{MessageError, MessageKind};
use crate::send::Priority;
use crate::{MessageManager, MessageNetId};
use alloc::{vec, vec::Vec};
use bevy_ecs::{
    change_detection::MutUntyped,
    component::{Component},
    entity::Entity,
    event::Event,
    system::ParallelCommands,
    world::{DeferredWorld, World},
};
use bevy_ecs::lifecycle::HookContext;
use lightyear_core::id::PeerId;
use lightyear_serde::ToBytes;
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_serde::writer::Writer;
use lightyear_transport::channel::{Channel, ChannelKind};
use lightyear_transport::prelude::Transport;
use tracing::trace;

/// Component used to send triggers of type `M` remotely.
#[derive(Component)]
#[require(MessageManager)]
#[component(on_add = EventSender::<M>::on_add_hook)]
pub struct EventSender<M: Event> {
    send: Vec<(M, ChannelKind, Priority)>,
    writer: Writer,
}

impl<M: Event> Default for EventSender<M> {
    fn default() -> Self {
        Self {
            send: Vec::new(),
            writer: Writer::default(),
        }
    }
}

impl<M: Event> EventSender<M> {
    /// Take all messages from the [`EventSender<M>`], serialize them, and buffer them
    /// on the appropriate channel of the [`Transport`].
    ///
    /// SAFETY: the `trigger_sender` must be of type [`EventSender<M>`]
    pub(crate) unsafe fn send_event_typed(
        trigger_sender: MutUntyped,
        net_id: MessageNetId,
        transport: &Transport,
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut SendEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY:  the `trigger_sender` must be of type `TriggerSender<M>`
        let mut sender = unsafe { trigger_sender.with_type::<Self>() };
        // enable split borrows
        let sender = &mut *sender;
        sender.send.drain(..).try_for_each(|(message, channel_kind, priority)| {
            // we write the message NetId, and then serialize the message
            net_id.to_bytes(&mut sender.writer)?;
            // SAFETY: the message has been checked to be of type `M`
            unsafe { serialize_metadata.serialize::<SendEntityMap, M, M>(&message, &mut sender.writer, entity_map)? };
            let bytes = sender.writer.split();
            trace!("Sending message of type {:?} with net_id {net_id:?} on channel {channel_kind:?}", core::any::type_name::<M>());
            transport.send_erased(channel_kind, bytes, priority)?;
            Ok(())
        })
    }

    // TODO: maybe we don't need this, it's identical to sending a message
    /// Take all messages from the [`EventSender<M>`], and trigger them as [`RemoteEvent<M>`] events
    ///
    /// # Safety
    ///
    /// - the `trigger_sender` must be of type [`EventSender<M>`]
    pub(crate) unsafe fn send_local_trigger_typed(
        trigger_sender: MutUntyped,
        commands: &ParallelCommands,
    ) {
        // SAFETY:  the `trigger_sender` must be of type `EventSender<M>`
        let mut sender = unsafe { trigger_sender.with_type::<Self>() };
        // enable split borrows
        sender
            .send
            .drain(..)
            .for_each(|(message, channel_kind, priority)| {
                let remote_trigger = RemoteEvent {
                    trigger: message,
                    // TODO: how to get the correct PeerId here?
                    from: PeerId::Local(0),
                };
                commands.command_scope(|mut c| {
                    c.trigger(remote_trigger);
                });
            });
    }

    pub fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let mut entity_mut = world.entity_mut(context.entity);
            let mut message_manager = entity_mut.get_mut::<MessageManager>().unwrap();
            let message_kind_present = message_manager
                .send_triggers
                .iter()
                .any(|(message_kind, _)| *message_kind == MessageKind::of::<M>());
            if !message_kind_present {
                message_manager
                    .send_triggers
                    .push((MessageKind::of::<M>(), context.component_id));
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

// SAFETY: the sender must correspond to the correct `TriggerSender<M>` type
pub(crate) type SendLocalTriggerFn = unsafe fn(sender: MutUntyped, commands: &ParallelCommands);

impl<M: Event> EventSender<M> {
    /// Buffers a trigger `M` to be sent over the specified channel to the target entities.
    pub fn trigger<C: Channel>(&mut self, trigger: M) {
        self.send.push((trigger, ChannelKind::of::<C>(), 1.0));
    }
}
