use crate::plugin::TimelineMessageConfig;
use crate::receive::BufferedMessageTimeline;
use crate::registry::{MessageError, MessageKind, MessageReceiverKind, TimelineKind};
use crate::{Message, MessageManager};
use bevy_ecs::change_detection::MutUntyped;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::event::EntityEvent;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::world::{DeferredWorld, World};
use bevy_ecs::{event::Event, system::ParallelCommands};
use bevy_utils::prelude::DebugName;
use core::any::Any;
use core::marker::PhantomData;
use lightyear_core::id::PeerId;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::packet::message::MessageId;
use lightyear_utils::ready_buffer::ReadyBuffer;
use tracing::trace;

/// Bevy Event emitted when a `RemoteEvent<M>` is received and processed.
/// Contains the original trigger `M` and the `PeerId` of the sender.
#[derive(Event, Debug)]
pub struct RemoteEvent<M: Event> {
    pub trigger: M,
    pub from: PeerId,
}

impl<M: EntityEvent> EntityEvent for RemoteEvent<M> {
    fn event_target(&self) -> Entity {
        self.trigger.event_target()
    }
}

struct PendingRemoteEvent<M> {
    trigger: M,
    from: PeerId,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
}

struct EventTimelineBuffer<M> {
    pending: ReadyBuffer<(Tick, u64), PendingRemoteEvent<M>>,
    next_sequence: u64,
}

impl<M> Default for EventTimelineBuffer<M> {
    fn default() -> Self {
        Self {
            pending: ReadyBuffer::default(),
            next_sequence: 0,
        }
    }
}

impl<M: Event> EventTimelineBuffer<M> {
    fn push_pending(
        &mut self,
        trigger: M,
        from: PeerId,
        target_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        if self.pending.len() >= config.max_pending_per_receiver {
            return Err(MessageError::PendingTimelineOverflow {
                limit: config.max_pending_per_receiver,
            });
        }
        let key = (target_tick, self.next_sequence);
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.pending.push(
            key,
            PendingRemoteEvent {
                trigger,
                from,
                channel_kind,
                message_id,
            },
        );
        Ok(())
    }

    fn release_until<T: BufferedMessageTimeline>(
        &mut self,
        commands: &ParallelCommands,
        tick: Tick,
    ) {
        for ((target_tick, _), event) in self.pending.drain_until(&(tick, u64::MAX)) {
            trace!(
                target: "lightyear_debug::message",
                kind = "event_timeline_release",
                schedule = "PreUpdate",
                sample_point = "PreUpdate",
                event_name = core::any::type_name::<M>(),
                remote_tick = target_tick.0,
                target_tick = target_tick.0,
                release_tick = tick.0,
                target_timeline = ?TimelineKind::of::<T>(),
                channel = ?event.channel_kind,
                message_id = ?event.message_id,
                "released timeline-delayed remote event"
            );
            commands.command_scope(|mut c| {
                c.trigger(RemoteEvent {
                    trigger: event.trigger,
                    from: event.from,
                });
            });
        }
    }

    fn clear(&mut self) {
        self.pending.clear();
        self.next_sequence = 0;
    }
}

/// Internal typed queue for events delivered by timeline `T`.
/// Immediate events are triggered directly and do not add a receiver component.
#[derive(Component)]
#[require(MessageManager)]
#[component(on_add = TimelineEventReceiver::<M, T>::on_add_hook)]
pub(crate) struct TimelineEventReceiver<M: Event, T: BufferedMessageTimeline> {
    buffer: EventTimelineBuffer<M>,
    marker: PhantomData<fn() -> T>,
}

impl<M: Event, T: BufferedMessageTimeline> Default for TimelineEventReceiver<M, T> {
    fn default() -> Self {
        Self {
            buffer: EventTimelineBuffer::default(),
            marker: PhantomData,
        }
    }
}

impl<M: Event, T: BufferedMessageTimeline> TimelineEventReceiver<M, T> {
    pub(crate) fn push_pending(
        &mut self,
        trigger: M,
        from: PeerId,
        target_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        self.buffer
            .push_pending(trigger, from, target_tick, channel_kind, message_id, config)
    }

    #[cfg(test)]
    pub(crate) fn num_pending_timeline_events(&self) -> usize {
        self.buffer.pending.len()
    }

    fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let mut entity_mut = world.entity_mut(context.entity);
            let mut message_manager = entity_mut.get_mut::<MessageManager>().unwrap();
            let receiver_kind =
                MessageReceiverKind::new(MessageKind::of::<M>(), Some(TimelineKind::of::<T>()));
            if !message_manager
                .receive_triggers
                .iter()
                .any(|(kind, _)| *kind == receiver_kind)
            {
                message_manager
                    .receive_triggers
                    .push((receiver_kind, context.component_id));
            }
        });
    }
}

pub(crate) type ReceiveTriggerFn = unsafe fn(
    commands: &ParallelCommands,
    receiver: Option<MutUntyped>,
    reader: &mut Reader,
    channel_kind: ChannelKind,
    channel_name: &'static str,
    remote_tick: Tick,
    message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>;

pub(crate) type ReceiveLocalTriggerFn = unsafe fn(
    receiver: MutUntyped,
    trigger: &mut dyn Any,
    from: PeerId,
    remote_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>;

pub(crate) type ReleaseTimelineEventFn =
    unsafe fn(receiver: MutUntyped, commands: &ParallelCommands, tick: Tick);
pub(crate) type ClearPendingTimelineEventFn = unsafe fn(receiver: MutUntyped);

/// Deserialize and immediately emit a `RemoteEvent<M>`.
pub(crate) unsafe fn receive_event_typed<M: Message + Event>(
    commands: &ParallelCommands,
    _receiver: Option<MutUntyped>,
    reader: &mut Reader,
    _channel_kind: ChannelKind,
    _channel_name: &'static str,
    _remote_tick: Tick,
    _message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId,
    _config: &TimelineMessageConfig,
) -> Result<(), MessageError> {
    let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
    trace!(
        "Received trigger message: {:?} from: {from:?}",
        DebugName::type_name::<M>()
    );
    commands.command_scope(|mut c| {
        c.trigger(RemoteEvent {
            trigger: message,
            from,
        });
    });
    Ok(())
}

pub(crate) unsafe fn receive_timeline_event_typed<M, T>(
    _commands: &ParallelCommands,
    receiver: Option<MutUntyped>,
    reader: &mut Reader,
    channel_kind: ChannelKind,
    _channel_name: &'static str,
    remote_tick: Tick,
    message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>
where
    M: Message + Event,
    T: BufferedMessageTimeline,
{
    let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
    let receiver = receiver.ok_or(MessageError::MissingTimelineEventReceiver(
        TimelineKind::of::<T>(),
    ))?;
    let mut receiver = unsafe { receiver.with_type::<TimelineEventReceiver<M, T>>() };
    receiver
        .buffer
        .push_pending(message, from, remote_tick, channel_kind, message_id, config)
}

pub(crate) unsafe fn receive_local_timeline_event_typed<M, T>(
    receiver: MutUntyped,
    trigger: &mut dyn Any,
    from: PeerId,
    remote_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>
where
    M: Message + Event,
    T: BufferedMessageTimeline,
{
    let mut receiver = unsafe { receiver.with_type::<TimelineEventReceiver<M, T>>() };
    if receiver.buffer.pending.len() >= config.max_pending_per_receiver {
        return Err(MessageError::PendingTimelineOverflow {
            limit: config.max_pending_per_receiver,
        });
    }
    let trigger = trigger
        .downcast_mut::<Option<M>>()
        .ok_or(MessageError::IncorrectType)?
        .take()
        .ok_or(MessageError::IncorrectType)?;
    receiver
        .buffer
        .push_pending(trigger, from, remote_tick, channel_kind, message_id, config)
}

pub(crate) unsafe fn release_event_typed<M, T>(
    receiver: MutUntyped,
    commands: &ParallelCommands,
    tick: Tick,
) where
    M: Event,
    T: BufferedMessageTimeline,
{
    let mut receiver = unsafe { receiver.with_type::<TimelineEventReceiver<M, T>>() };
    receiver.buffer.release_until::<T>(commands, tick);
}

pub(crate) unsafe fn clear_pending_event_typed<M, T>(receiver: MutUntyped)
where
    M: Event,
    T: BufferedMessageTimeline,
{
    let mut receiver = unsafe { receiver.with_type::<TimelineEventReceiver<M, T>>() };
    receiver.buffer.clear();
}

pub(crate) unsafe fn has_pending_event_typed<M, T>(receiver: MutUntyped) -> bool
where
    M: Event,
    T: BufferedMessageTimeline,
{
    let receiver = unsafe { receiver.with_type::<TimelineEventReceiver<M, T>>() };
    !receiver.buffer.pending.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Event)]
    struct TestEvent;

    struct TestChannel;

    #[test]
    fn pending_timeline_events_are_bounded_per_receiver() {
        let mut buffer = EventTimelineBuffer::<TestEvent>::default();
        let config = TimelineMessageConfig {
            max_pending_per_receiver: 1,
            ..Default::default()
        };
        buffer
            .push_pending(
                TestEvent,
                PeerId::Local(1),
                Tick(1),
                ChannelKind::of::<TestChannel>(),
                None,
                &config,
            )
            .unwrap();
        let error = buffer
            .push_pending(
                TestEvent,
                PeerId::Local(1),
                Tick(2),
                ChannelKind::of::<TestChannel>(),
                None,
                &config,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            MessageError::PendingTimelineOverflow { limit: 1 }
        ));
        assert_eq!(buffer.pending.len(), 1);
    }
}
