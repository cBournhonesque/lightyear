use crate::plugin::MessagePlugin;
use crate::prelude::MessageSender;
use crate::registry::{MessageError, MessageKind, MessageRegistry};
use crate::send::Priority;
pub(crate) use crate::trigger::TriggerMessage;
use crate::{MessageManager, MessageNetId};
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
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
use tracing::{error, info, trace};

/// Component used to send triggers of type `M` remotely.
#[derive(Component)]
#[require(MessageManager)]
#[component(on_add = TriggerSender::<M>::on_add_hook)]
pub struct TriggerSender<M: Event> {
    send: Vec<(TriggerMessage<M>, ChannelKind, Priority)>,
    writer: Writer,
}

impl<M: Event> Default for TriggerSender<M> {
    fn default() -> Self {
        Self {
            send: Vec::new(),
            writer: Writer::default(),
        }
    }
}

impl <M: Event> TriggerSender<M> {

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
            serialize_metadata.serialize::<SendEntityMap, TriggerMessage<M>, M>(&message, &mut sender.writer, entity_map)?;
            let bytes = sender.writer.split();
            trace!("Sending message of type {:?} with net_id {net_id:?} on channel {channel_kind:?}", core::any::type_name::<TriggerMessage<M>>());
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
                .send_triggers
                .iter()
                .any(|(message_kind, _)| {
                    *message_kind == MessageKind::of::<M>()
                });
            if !message_kind_present {
                message_manager.send_triggers.push((MessageKind::of::<M>(), context.component_id));
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
        self.send.push((message, ChannelKind::of::<C>(), 1.0));
    }
}