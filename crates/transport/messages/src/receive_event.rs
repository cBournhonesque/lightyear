use crate::plugin::TimelineMessageConfig;
use crate::receive::BufferedMessageTimeline;
use crate::registry::{MessageError, TimelineKind};
use crate::{Message, MessageManager};
use bevy_ecs::change_detection::MutUntyped;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::event::{EntityEvent, Event};
use bevy_ecs::system::ParallelCommands;
use bevy_ecs::world::World;
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

/// Private typed storage for remote events delayed by delivery timeline `T`.
///
/// One component is registered for each `(M, T)` pair used by the protocol and
/// attached lazily when a connection first receives that event. Keeping `M`
/// typed avoids per-event allocation and runtime downcasting. This component
/// remains private because applications consume the resulting [`RemoteEvent<M>`], not
/// the pending queue. Immediate events do not use this component.
#[derive(Component)]
#[require(MessageManager)]
pub(crate) struct PendingTimelineEvents<M: Event, T: BufferedMessageTimeline> {
    pending: ReadyBuffer<(Tick, u64), PendingRemoteEvent<M>>,
    next_sequence: u64,
    /// Binds this otherwise timeline-agnostic queue type to timeline `T`.
    ///
    /// The queue stores no `T` value, so a marker is required to use `T` as a
    /// type parameter. `fn() -> T` makes the queue covariant in `T` without
    /// modeling ownership of a `T` or propagating its auto-trait and drop-check
    /// constraints. The queue never consumes or mutably exposes a `T`, so it
    /// needs neither contravariance nor invariance.
    marker: PhantomData<fn() -> T>,
}

impl<M: Event, T: BufferedMessageTimeline> Default for PendingTimelineEvents<M, T> {
    fn default() -> Self {
        Self {
            pending: ReadyBuffer::default(),
            next_sequence: 0,
            marker: PhantomData,
        }
    }
}

impl<M: Event, T: BufferedMessageTimeline> PendingTimelineEvents<M, T> {
    fn ensure_new_capacity(config: &TimelineMessageConfig) -> Result<(), MessageError> {
        if config.max_pending_per_receiver == 0 {
            return Err(MessageError::PendingTimelineOverflow { limit: 0 });
        }
        Ok(())
    }

    fn ensure_capacity(&self, config: &TimelineMessageConfig) -> Result<(), MessageError> {
        if self.pending.len() >= config.max_pending_per_receiver {
            return Err(MessageError::PendingTimelineOverflow {
                limit: config.max_pending_per_receiver,
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
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        self.ensure_capacity(config)?;
        self.push_unchecked(trigger, from, target_tick, channel_kind, message_id);
        Ok(())
    }

    fn push_unchecked(
        &mut self,
        trigger: M,
        from: PeerId,
        target_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
    ) {
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
    }

    fn queue(
        commands: &ParallelCommands,
        entity: Entity,
        trigger: M,
        from: PeerId,
        target_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        config: TimelineMessageConfig,
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
                    &config,
                ) {
                    error!(
                        "Error buffering event {:?} on lazily inserted timeline queue: {error:?}",
                        DebugName::type_name::<M>()
                    );
                }
            });
        });
    }

    pub(crate) fn release_until(&mut self, commands: &ParallelCommands, tick: Tick) {
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
            commands.command_scope(|mut commands| {
                commands.trigger(RemoteEvent {
                    trigger: event.trigger,
                    from: event.from,
                });
            });
        }
    }

    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.next_sequence = 0;
    }

    #[cfg(test)]
    pub(crate) fn num_pending(&self) -> usize {
        self.pending.len()
    }
}

pub(crate) type ReceiveTriggerFn = unsafe fn(
    commands: &ParallelCommands,
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
    commands: &ParallelCommands,
    trigger: &mut dyn Any,
    from: PeerId,
    remote_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>;

pub(crate) type ReceiveTimelineTriggerFn = unsafe fn(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
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

pub(crate) type ReceiveLocalTimelineTriggerFn = unsafe fn(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
    trigger: &mut dyn Any,
    from: PeerId,
    remote_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>;

pub(crate) type ReleaseTimelineTriggerFn =
    unsafe fn(receiver: MutUntyped, commands: &ParallelCommands, tick: Tick);

pub(crate) type ClearTimelineTriggerFn = unsafe fn(receiver: MutUntyped);

/// Deserialize and immediately emit a `RemoteEvent<M>`.
pub(crate) unsafe fn receive_event_typed<M: Message + Event>(
    commands: &ParallelCommands,
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
    commands.command_scope(|mut commands| {
        commands.trigger(RemoteEvent {
            trigger: message,
            from,
        });
    });
    Ok(())
}

pub(crate) unsafe fn receive_local_event_typed<M: Event>(
    commands: &ParallelCommands,
    trigger: &mut dyn Any,
    from: PeerId,
    _remote_tick: Tick,
    _channel_kind: ChannelKind,
    _message_id: Option<MessageId>,
    _config: &TimelineMessageConfig,
) -> Result<(), MessageError> {
    let trigger = trigger
        .downcast_mut::<Option<M>>()
        .ok_or(MessageError::IncorrectType)?
        .take()
        .ok_or(MessageError::IncorrectType)?;
    commands.command_scope(|mut commands| {
        commands.trigger(RemoteEvent { trigger, from });
    });
    Ok(())
}

pub(crate) unsafe fn receive_timeline_event_typed<M, T>(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
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
    if receiver.is_none() {
        PendingTimelineEvents::<M, T>::ensure_new_capacity(config)?;
    }
    let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
    if let Some(receiver) = receiver {
        // SAFETY: the callback and component id are registered for this queue type.
        let mut receiver = unsafe { receiver.with_type::<PendingTimelineEvents<M, T>>() };
        receiver.push(message, from, remote_tick, channel_kind, message_id, config)
    } else {
        PendingTimelineEvents::<M, T>::queue(
            commands,
            entity,
            message,
            from,
            remote_tick,
            channel_kind,
            message_id,
            *config,
        );
        Ok(())
    }
}

pub(crate) unsafe fn receive_local_timeline_event_typed<M, T>(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
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
    if let Some(receiver) = receiver.as_ref() {
        // SAFETY: the callback and component id are registered for this queue type.
        let receiver = unsafe { receiver.as_ref().deref::<PendingTimelineEvents<M, T>>() };
        receiver.ensure_capacity(config)?;
    } else {
        PendingTimelineEvents::<M, T>::ensure_new_capacity(config)?;
    }
    let trigger = trigger
        .downcast_mut::<Option<M>>()
        .ok_or(MessageError::IncorrectType)?
        .take()
        .ok_or(MessageError::IncorrectType)?;
    if let Some(receiver) = receiver {
        // SAFETY: the callback and component id are registered for this queue type.
        let mut receiver = unsafe { receiver.with_type::<PendingTimelineEvents<M, T>>() };
        receiver.push_unchecked(trigger, from, remote_tick, channel_kind, message_id);
        Ok(())
    } else {
        PendingTimelineEvents::<M, T>::queue(
            commands,
            entity,
            trigger,
            from,
            remote_tick,
            channel_kind,
            message_id,
            *config,
        );
        Ok(())
    }
}

pub(crate) unsafe fn release_timeline_events_typed<M, T>(
    receiver: MutUntyped,
    commands: &ParallelCommands,
    tick: Tick,
) where
    M: Event,
    T: BufferedMessageTimeline,
{
    // SAFETY: the callback is registered with PendingTimelineEvents<M, T>.
    let mut receiver = unsafe { receiver.with_type::<PendingTimelineEvents<M, T>>() };
    receiver.release_until(commands, tick);
}

pub(crate) unsafe fn clear_timeline_events_typed<M, T>(receiver: MutUntyped)
where
    M: Event,
    T: BufferedMessageTimeline,
{
    // SAFETY: the callback is registered with PendingTimelineEvents<M, T>.
    let mut receiver = unsafe { receiver.with_type::<PendingTimelineEvents<M, T>>() };
    receiver.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_ecs::component::Component;
    use lightyear_core::prelude::NetworkTimeline;
    use lightyear_core::time::{Overstep, TickDelta, TickInstant};
    use lightyear_core::timeline::TimelineConfig;

    #[derive(Event)]
    struct TestEvent;

    struct TestChannel;

    #[derive(Component)]
    struct TestTimelineConfig;

    #[derive(Component, Default)]
    struct TestTimeline(TickInstant);

    impl TimelineConfig for TestTimelineConfig {
        type Context = ();
        type Timeline = TestTimeline;
    }

    impl NetworkTimeline for TestTimeline {
        type Config = TestTimelineConfig;

        fn now(&self) -> TickInstant {
            self.0
        }

        fn tick(&self) -> Tick {
            self.0.tick()
        }

        fn overstep(&self) -> Overstep {
            self.0.overstep()
        }

        fn set_now(&mut self, now: TickInstant) {
            self.0 = now;
        }

        fn apply_delta(&mut self, delta: TickDelta) {
            self.0 = self.0 + delta;
        }
    }

    impl BufferedMessageTimeline for TestTimeline {}

    #[test]
    fn pending_timeline_events_are_bounded_per_timeline() {
        let mut buffer = PendingTimelineEvents::<TestEvent, TestTimeline>::default();
        let config = TimelineMessageConfig {
            max_pending_per_receiver: 1,
            ..Default::default()
        };
        buffer
            .push(
                TestEvent,
                PeerId::Local(1),
                Tick(1),
                ChannelKind::of::<TestChannel>(),
                None,
                &config,
            )
            .unwrap();
        let error = buffer
            .push(
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
        assert_eq!(buffer.num_pending(), 1);
    }
}
