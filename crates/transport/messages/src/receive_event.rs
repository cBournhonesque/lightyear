use crate::plugin::MAX_PENDING_TIMELINE_PAYLOADS;
use crate::registry::MessageError;
use crate::{Message, MessageManager};
use bevy_ecs::change_detection::MutUntyped;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::event::{EntityEvent, Event};
use bevy_ecs::system::ParallelCommands;
use bevy_ecs::world::World;
use bevy_utils::prelude::DebugName;
use core::any::Any;
use lightyear_core::id::PeerId;
use lightyear_core::tick::Tick;
use lightyear_core::timeline::TimelineKind;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::packet::message::MessageId;
use lightyear_utils::collections::HashMap;
use lightyear_utils::ready_buffer::ReadyBuffer;
use tracing::{error, trace};

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

struct PendingRemoteEvent<M: Event> {
    trigger: M,
    from: PeerId,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
}

struct TimelineEventBuffer<M: Event> {
    pending: ReadyBuffer<(Tick, u64), PendingRemoteEvent<M>>,
    next_sequence: u64,
}

impl<M: Event> Default for TimelineEventBuffer<M> {
    fn default() -> Self {
        Self {
            pending: ReadyBuffer::default(),
            next_sequence: 0,
        }
    }
}

/// Private typed storage for remote events delayed by a channel timeline.
///
/// One component is registered per event `M` and attached lazily when a
/// connection first receives that event on any timeline channel. Its internal
/// queues are keyed by [`TimelineKind`], so adding a channel timeline does not
/// require another event registration. Keeping `M` typed avoids per-event
/// allocation and runtime downcasting. Applications consume the resulting
/// [`RemoteEvent<M>`], not this component.
#[derive(Component)]
#[require(MessageManager)]
pub(crate) struct PendingTimelineEvents<M: Event> {
    timelines: HashMap<TimelineKind, TimelineEventBuffer<M>>,
}

impl<M: Event> Default for PendingTimelineEvents<M> {
    fn default() -> Self {
        Self {
            timelines: HashMap::default(),
        }
    }
}

impl<M: Event> PendingTimelineEvents<M> {
    fn ensure_capacity(&self, timeline: TimelineKind) -> Result<(), MessageError> {
        if self
            .timelines
            .get(&timeline)
            .is_some_and(|pending| pending.pending.len() >= MAX_PENDING_TIMELINE_PAYLOADS)
        {
            return Err(MessageError::PendingTimelineOverflow {
                limit: MAX_PENDING_TIMELINE_PAYLOADS,
            });
        }
        Ok(())
    }

    pub(crate) fn push(
        &mut self,
        trigger: M,
        from: PeerId,
        target_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        timeline: TimelineKind,
    ) -> Result<(), MessageError> {
        self.ensure_capacity(timeline)?;
        self.push_unchecked(
            trigger,
            from,
            target_tick,
            channel_kind,
            message_id,
            timeline,
        );
        Ok(())
    }

    fn push_unchecked(
        &mut self,
        trigger: M,
        from: PeerId,
        target_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        timeline: TimelineKind,
    ) {
        let pending = self.timelines.entry(timeline).or_default();
        let key = (target_tick, pending.next_sequence);
        pending.next_sequence = pending.next_sequence.wrapping_add(1);
        pending.pending.push(
            key,
            PendingRemoteEvent {
                trigger,
                from,
                channel_kind,
                message_id,
            },
        );
    }

    fn queue(
        commands: &ParallelCommands,
        entity: Entity,
        trigger: M,
        from: PeerId,
        target_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        timeline: TimelineKind,
    ) {
        commands.command_scope(|mut commands| {
            commands.queue(move |world: &mut World| {
                let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
                    return;
                };
                if !entity_mut.contains::<Self>() {
                    entity_mut.insert(Self::default());
                }
                let mut receiver = entity_mut
                    .get_mut::<Self>()
                    .expect("timeline event queue was just inserted");
                if let Err(error) = receiver.push(
                    trigger,
                    from,
                    target_tick,
                    channel_kind,
                    message_id,
                    timeline,
                ) {
                    error!(
                        "Error buffering event {:?} on lazily inserted timeline queue: {error:?}",
                        DebugName::type_name::<M>()
                    );
                }
            });
        });
    }

    pub(crate) fn release_until(
        &mut self,
        commands: &ParallelCommands,
        timeline: TimelineKind,
        tick: Tick,
    ) {
        let Some(pending) = self.timelines.get_mut(&timeline) else {
            return;
        };
        for ((target_tick, _), event) in pending.pending.drain_until(&(tick, u64::MAX)) {
            trace!(
                target: "lightyear_debug::message",
                kind = "event_timeline_release",
                schedule = "PreUpdate",
                sample_point = "PreUpdate",
                event_name = core::any::type_name::<M>(),
                remote_tick = target_tick.0,
                target_tick = target_tick.0,
                release_tick = tick.0,
                target_timeline = ?timeline,
                channel = ?event.channel_kind,
                message_id = ?event.message_id,
                "released timeline-delayed remote event"
            );
            commands.command_scope(|mut commands| {
                commands.trigger(RemoteEvent {
                    trigger: event.trigger,
                    from: event.from,
                });
            });
        }
    }

    #[cfg(test)]
    pub(crate) fn num_pending(&self) -> usize {
        self.timelines
            .values()
            .map(|pending| pending.pending.len())
            .sum()
    }
}

/// Type-erased callback for the serialized-byte receive path of an event.
pub(crate) type ReceiveTriggerFn = unsafe fn(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
    reader: &mut Reader,
    channel_kind: ChannelKind,
    channel_name: &'static str,
    remote_tick: Tick,
    message_id: Option<MessageId>,
    target_timeline: Option<TimelineKind>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId,
) -> Result<(), MessageError>;

/// Type-erased callback for direct, already-typed host-client event delivery.
pub(crate) type ReceiveLocalTriggerFn = unsafe fn(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
    trigger: &mut dyn Any,
    from: PeerId,
    remote_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
    target_timeline: Option<TimelineKind>,
) -> Result<(), MessageError>;

pub(crate) type ReleaseTimelineTriggerFn = unsafe fn(
    receiver: MutUntyped,
    commands: &ParallelCommands,
    timeline: TimelineKind,
    tick: Tick,
);

/// Receives an event from the serialized-byte path.
///
/// This path is used after reading a payload from transport (or an equivalent
/// serialized host-client buffer). It deserializes `M` and applies receive-side
/// entity mapping. An immediate-channel event triggers [`RemoteEvent<M>`]
/// directly; a timeline-channel event is buffered until that timeline reaches
/// `remote_tick`.
pub(crate) unsafe fn receive_event_typed<M: Message + Event>(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
    reader: &mut Reader,
    channel_kind: ChannelKind,
    _channel_name: &'static str,
    remote_tick: Tick,
    message_id: Option<MessageId>,
    target_timeline: Option<TimelineKind>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId,
) -> Result<(), MessageError> {
    let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
    if let Some(timeline) = target_timeline {
        if let Some(receiver) = receiver {
            // SAFETY: the callback and component id are registered for this event type.
            let mut receiver = unsafe { receiver.with_type::<PendingTimelineEvents<M>>() };
            return receiver.push(
                message,
                from,
                remote_tick,
                channel_kind,
                message_id,
                timeline,
            );
        }
        PendingTimelineEvents::<M>::queue(
            commands,
            entity,
            message,
            from,
            remote_tick,
            channel_kind,
            message_id,
            timeline,
        );
        return Ok(());
    }
    trace!(
        "Received trigger message: {:?} from: {from:?}",
        DebugName::type_name::<M>()
    );
    commands.command_scope(|mut commands| {
        commands.trigger(RemoteEvent {
            trigger: message,
            from,
        });
    });
    Ok(())
}

/// Receives an event through the direct host-client fast path.
///
/// Unlike [`receive_event_typed`], the event is already a typed `M` and has not
/// made a serialization round trip. It is stored as `Option<M>` behind erased
/// `Any`; this function takes ownership, then either triggers [`RemoteEvent<M>`]
/// immediately or buffers it for its channel timeline.
pub(crate) unsafe fn receive_local_event_typed<M: Event>(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
    trigger: &mut dyn Any,
    from: PeerId,
    remote_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
    target_timeline: Option<TimelineKind>,
) -> Result<(), MessageError> {
    if let Some(timeline) = target_timeline
        && let Some(receiver) = receiver.as_ref()
    {
        // SAFETY: the callback and component id are registered for this event type.
        let receiver = unsafe { receiver.as_ref().deref::<PendingTimelineEvents<M>>() };
        receiver.ensure_capacity(timeline)?;
    }
    let trigger = trigger
        .downcast_mut::<Option<M>>()
        .ok_or(MessageError::IncorrectType)?
        .take()
        .ok_or(MessageError::IncorrectType)?;
    if let Some(timeline) = target_timeline {
        if let Some(receiver) = receiver {
            // SAFETY: the callback and component id are registered for this event type.
            let mut receiver = unsafe { receiver.with_type::<PendingTimelineEvents<M>>() };
            receiver.push_unchecked(
                trigger,
                from,
                remote_tick,
                channel_kind,
                message_id,
                timeline,
            );
        } else {
            PendingTimelineEvents::<M>::queue(
                commands,
                entity,
                trigger,
                from,
                remote_tick,
                channel_kind,
                message_id,
                timeline,
            );
        }
        return Ok(());
    }
    commands.command_scope(|mut commands| {
        commands.trigger(RemoteEvent { trigger, from });
    });
    Ok(())
}

pub(crate) unsafe fn release_timeline_events_typed<M>(
    receiver: MutUntyped,
    commands: &ParallelCommands,
    timeline: TimelineKind,
    tick: Tick,
) where
    M: Event,
{
    // SAFETY: the callback is registered with PendingTimelineEvents<M>.
    let mut receiver = unsafe { receiver.with_type::<PendingTimelineEvents<M>>() };
    receiver.release_until(commands, timeline, tick);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::any::TypeId;

    #[derive(Event)]
    struct TestEvent;

    struct TestChannel;

    struct TestTimeline;

    #[test]
    fn pending_timeline_events_are_bounded_per_timeline() {
        let mut buffer = PendingTimelineEvents::<TestEvent>::default();
        let timeline = TimelineKind::from(TypeId::of::<TestTimeline>());
        for _ in 0..MAX_PENDING_TIMELINE_PAYLOADS {
            buffer
                .push(
                    TestEvent,
                    PeerId::Local(1),
                    Tick(1),
                    ChannelKind::of::<TestChannel>(),
                    None,
                    timeline,
                )
                .unwrap();
        }
        let error = buffer
            .push(
                TestEvent,
                PeerId::Local(1),
                Tick(2),
                ChannelKind::of::<TestChannel>(),
                None,
                timeline,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            MessageError::PendingTimelineOverflow {
                limit: MAX_PENDING_TIMELINE_PAYLOADS
            }
        ));
        assert_eq!(buffer.num_pending(), MAX_PENDING_TIMELINE_PAYLOADS);
    }
}
