use crate::plugin::TimelineMessageConfig;
use crate::registry::{
    MessageError, MessageKind, MessageReceiverKind, MessageRegistry, ReceiveTriggerMetadata,
    TimelineKind,
};
use crate::send::Priority;
use crate::{MessageManager, MessageNetId};
use alloc::vec::Vec;
use bevy_ecs::change_detection::MutUntyped;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::{DeferredWorld, FilteredEntityMut};
use bevy_utils::prelude::DebugName;
use lightyear_core::id::PeerId;
use lightyear_core::tick::Tick;
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
        let pending = core::mem::take(&mut sender.send);
        let mut pending = pending.into_iter();
        while let Some(item) = pending.next() {
            // we write the message NetId, and then serialize the message
            let serialize_result = net_id.to_bytes(&mut sender.writer).and_then(|_| {
                // SAFETY: the message has been checked to be of type `M`.
                unsafe {
                    serialize_metadata.serialize::<SendEntityMap, M, M>(
                        &item.event,
                        &mut sender.writer,
                        entity_map,
                    )
                }
            });
            if let Err(error) = serialize_result {
                sender.writer.split();
                sender.send.push(item);
                sender.send.extend(pending);
                return Err(error.into());
            }
            let bytes = sender.writer.split();
            trace!(
                "Sending message of type {:?} with net_id {net_id:?} on channel {:?}",
                DebugName::type_name::<M>(),
                item.channel_kind
            );
            if let Err(error) = transport.send_erased(item.channel_kind, bytes, item.priority) {
                sender.send.push(item);
                sender.send.extend(pending);
                return Err(error.into());
            }
        }
        Ok(())
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
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        // SAFETY:  the `trigger_sender` must be of type `EventSender<M>`
        let mut sender = unsafe { trigger_sender.with_type::<Self>() };
        // enable split borrows
        let queued = core::mem::take(&mut sender.send);
        let mut queued = queued.into_iter();
        while let Some(pending) = queued.next() {
            let target_timeline = channel_registry
                .settings(pending.channel_kind)
                .and_then(|settings| settings.delivery_timeline())
                .map(TimelineKind::from);
            if let Some(target_timeline) = target_timeline {
                let Some(timeline_metadata) = registry.timeline_metadata.get(&target_timeline)
                else {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::TimelineNotRegistered(target_timeline));
                };
                let Some(timeline_ptr) = receivers.get_by_id(timeline_metadata.component_id) else {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::MissingTimeline(target_timeline));
                };
                // SAFETY: the callback is registered with this timeline component id.
                let current_tick = unsafe { (timeline_metadata.tick_fn)(timeline_ptr) };
                let delta = tick - current_tick;
                if delta > 0 && delta as u32 > config.max_future_ticks {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::TimelineTooFarAhead {
                        target: tick,
                        current: current_tick,
                        max_future_ticks: config.max_future_ticks,
                    });
                }
            }

            let receiver_kind = MessageReceiverKind::new(MessageKind::of::<M>(), target_timeline);
            let Some(metadata) = registry.receive_trigger.get(&receiver_kind) else {
                sender.send.push(pending);
                sender.send.extend(queued);
                return Err(target_timeline.map_or(
                    MessageError::UnrecognizedMessage(MessageKind::of::<M>()),
                    MessageError::MissingTimelineEventRegistration,
                ));
            };
            let PendingEvent {
                event,
                channel_kind,
                priority,
            } = pending;
            let mut event = Some(event);
            let result = match metadata {
                ReceiveTriggerMetadata::Immediate(metadata) => unsafe {
                    (metadata.receive_local_trigger_fn)(
                        commands,
                        &mut event,
                        PeerId::Local(0),
                        tick,
                        channel_kind,
                        None,
                        config,
                    )
                },
                ReceiveTriggerMetadata::Timeline(metadata) => {
                    let receiver_entity = receivers.id();
                    let receiver = receivers.get_mut_by_id(metadata.component_id);
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
                            config,
                        )
                    }
                }
            };
            if let Err(error) = result {
                sender.send.push(PendingEvent {
                    event: event
                        .take()
                        .expect("failed local event receive must not consume the event"),
                    channel_kind,
                    priority,
                });
                sender.send.extend(queued);
                return Err(error);
            }
        }
        Ok(())
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
    config: &TimelineMessageConfig,
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
