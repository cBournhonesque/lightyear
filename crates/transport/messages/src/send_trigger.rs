use crate::plugin::TimelineMessageConfig;
use crate::receive_event::{EventReceiver, RemoteEvent};
use crate::registry::{MessageError, MessageKind, MessageRegistry, TimelineKind};
use crate::send::Priority;
use crate::{MessageManager, MessageNetId};
use alloc::vec::Vec;
use bevy_ecs::change_detection::MutUntyped;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::prelude::*;
use bevy_ecs::world::DeferredWorld;
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
    /// Take all messages from the [`EventSender<M>`], and trigger them as [`RemoteEvent<M>`] events
    ///
    /// # Safety
    ///
    /// - the `trigger_sender` must be of type [`EventSender<M>`]
    pub(crate) unsafe fn send_local_trigger_typed(
        trigger_sender: MutUntyped,
        trigger_receiver: Option<MutUntyped>,
        commands: &ParallelCommands,
        tick: Tick,
        registry: &MessageRegistry,
        channel_registry: &ChannelRegistry,
        available_timelines: &[(TimelineKind, Tick)],
        config: &TimelineMessageConfig,
    ) -> Result<usize, MessageError> {
        // SAFETY:  the `trigger_sender` must be of type `EventSender<M>`
        let mut sender = unsafe { trigger_sender.with_type::<Self>() };
        // SAFETY: if present, the receiver was looked up from the registry for this event type.
        let mut receiver =
            trigger_receiver.map(|receiver| unsafe { receiver.with_type::<EventReceiver<M>>() });
        // enable split borrows
        let queued = core::mem::take(&mut sender.send);
        let mut queued = queued.into_iter();
        let mut pending_count = 0;
        while let Some(pending) = queued.next() {
            let target_timeline = channel_registry
                .settings(pending.channel_kind)
                .and_then(|settings| settings.delivery_timeline())
                .map(TimelineKind::from);
            if let Some(target_timeline) = target_timeline {
                if !registry.timeline_metadata.contains_key(&target_timeline) {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::TimelineNotRegistered(target_timeline));
                }
                let Some((_, current_tick)) = available_timelines
                    .iter()
                    .find(|(kind, _)| *kind == target_timeline)
                else {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::MissingTimeline(target_timeline));
                };
                let delta = tick - *current_tick;
                if delta > 0 && delta as u32 > config.max_future_ticks {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::TimelineTooFarAhead {
                        target: tick,
                        current: *current_tick,
                        max_future_ticks: config.max_future_ticks,
                    });
                }
                if let Some(receiver) = receiver.as_mut() {
                    if let Err(error) = receiver.ensure_pending_capacity(config) {
                        sender.send.push(pending);
                        sender.send.extend(queued);
                        return Err(error);
                    }
                    if let Err(error) = receiver.push_pending(
                        pending.event,
                        PeerId::Local(0),
                        tick,
                        tick,
                        target_timeline,
                        pending.channel_kind,
                        None,
                        config,
                    ) {
                        sender.send.extend(queued);
                        return Err(error);
                    }
                    pending_count += 1;
                } else {
                    sender.send.push(pending);
                    sender.send.extend(queued);
                    return Err(MessageError::MissingTimelineEventReceiver(target_timeline));
                }
            } else {
                let remote_trigger = RemoteEvent {
                    trigger: pending.event,
                    // TODO: how to get the correct PeerId here?
                    from: PeerId::Local(0),
                };
                commands.command_scope(|mut c| {
                    c.trigger(remote_trigger);
                });
            }
        }
        Ok(pending_count)
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
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    tick: Tick,
    registry: &MessageRegistry,
    channel_registry: &ChannelRegistry,
    available_timelines: &[(TimelineKind, Tick)],
    config: &TimelineMessageConfig,
) -> Result<usize, MessageError>;

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
