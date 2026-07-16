use crate::plugin::MAX_TIMELINE_LAG_TICKS;
use crate::registry::{MessageError, MessageKind, MessageRegistry};
use crate::send::Priority;
use crate::{MessageManager, MessageNetId};
use alloc::vec::Vec;
use bevy_ecs::change_detection::MutUntyped;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::{DeferredWorld, FilteredEntityMut};
use bevy_utils::prelude::DebugName;
use lightyear_core::id::PeerId;
use lightyear_core::prelude::{Tick, TimelineRegistry};
use lightyear_serde::ToBytes;
use lightyear_serde::entity_map::SendEntityMap;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_serde::writer::Writer;
use lightyear_transport::channel::{Channel, ChannelKind};
use lightyear_transport::prelude::{ChannelRegistry, Transport};
use tracing::trace;

/// Component used to send triggers of type `M` remotely.
#[derive(Component)]
#[require(MessageManager)]
#[component(on_add = EventSender::<M>::on_add_hook)]
pub struct EventSender<M: Event> {
    send: Vec<PendingEvent<M>>,
    writer: Writer,
}

struct PendingEvent<M> {
    event: M,
    channel_kind: ChannelKind,
    priority: Priority,
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
        sender.send.drain(..).try_for_each(|pending| {
            let PendingEvent {
                event,
                channel_kind,
                priority,
            } = pending;
            // we write the message NetId, and then serialize the message
            net_id.to_bytes(&mut sender.writer)?;
            // SAFETY: the message has been checked to be of type `M`.
            unsafe {
                serialize_metadata.serialize::<SendEntityMap, M, M>(
                    &event,
                    &mut sender.writer,
                    entity_map,
                )?
            };
            let bytes = sender.writer.split();
            trace!(
                "Sending message of type {:?} with net_id {net_id:?} on channel {:?}",
                DebugName::type_name::<M>(),
                channel_kind
            );
            transport.send_erased(channel_kind, bytes, priority)?;
            Ok(())
        })
    }

    // TODO: maybe we don't need this, it's identical to sending a message
    /// Take all messages from the [`EventSender<M>`], and trigger them as
    /// [`RemoteEvent<M>`](crate::receive_event::RemoteEvent) events.
    ///
    /// # Safety
    ///
    /// - the `trigger_sender` must be of type [`EventSender<M>`]
    pub(crate) unsafe fn send_local_trigger_typed(
        trigger_sender: MutUntyped,
        receivers: &mut FilteredEntityMut<'_, '_>,
        commands: &ParallelCommands,
        tick: Tick,
        registry: &MessageRegistry,
        channel_registry: &ChannelRegistry,
        timeline_registry: &TimelineRegistry,
    ) -> Result<(), MessageError> {
        // SAFETY:  the `trigger_sender` must be of type `EventSender<M>`
        let mut sender = unsafe { trigger_sender.with_type::<Self>() };
        sender.send.drain(..).try_for_each(|pending| {
            let target_timeline = channel_registry
                .settings(pending.channel_kind)
                .and_then(|settings| settings.timeline);
            if let Some(target_timeline) = target_timeline {
                let timeline_metadata = timeline_registry
                    .get(&target_timeline)
                    .ok_or(MessageError::TimelineNotRegistered(target_timeline))?;
                let timeline_ptr = receivers
                    .get_by_id(timeline_metadata.component_id())
                    .ok_or(MessageError::MissingTimeline(target_timeline))?;
                // SAFETY: the metadata is registered with this timeline component id.
                let current_tick = unsafe { timeline_metadata.tick(timeline_ptr) };
                let delta = tick - current_tick;
                if delta > 0 && delta as u32 > MAX_TIMELINE_LAG_TICKS {
                    return Err(MessageError::TimelineTooFarBehind {
                        target: tick,
                        current: current_tick,
                        max_lag_ticks: MAX_TIMELINE_LAG_TICKS,
                    });
                }
            }

            let metadata = registry
                .receive_trigger
                .get(&MessageKind::of::<M>())
                .ok_or(MessageError::UnrecognizedMessage(MessageKind::of::<M>()))?;
            let PendingEvent {
                event,
                channel_kind,
                priority: _,
            } = pending;
            let mut event = Some(event);
            let receiver_entity = receivers.id();
            let receiver = receivers.get_mut_by_id(metadata.component_id);
            // SAFETY: the callback and pending component id are registered for `M`.
            unsafe {
                (metadata.receive_local_trigger_fn)(
                    receiver,
                    commands,
                    receiver_entity,
                    &mut event,
                    PeerId::Local(0),
                    tick,
                    channel_kind,
                    None,
                    target_timeline,
                )
            }
        })
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
pub(crate) type SendLocalTriggerFn = unsafe fn(
    sender: MutUntyped,
    receivers: &mut FilteredEntityMut<'_, '_>,
    commands: &ParallelCommands,
    tick: Tick,
    registry: &MessageRegistry,
    channel_registry: &ChannelRegistry,
    timeline_registry: &TimelineRegistry,
) -> Result<(), MessageError>;

impl<M: Event> EventSender<M> {
    /// Buffers a trigger `M` to be sent over the specified channel to the target entities.
    pub fn trigger<C: Channel>(&mut self, trigger: M) {
        self.trigger_with_priority::<C>(trigger, 1.0);
    }

    /// Buffers an event with an explicit bandwidth priority.
    pub fn trigger_with_priority<C: Channel>(&mut self, trigger: M, priority: Priority) {
        self.send.push(PendingEvent {
            event: trigger,
            channel_kind: ChannelKind::of::<C>(),
            priority,
        });
    }
}
