use crate::plugin::TimelineMessageConfig;
use crate::registry::MessageError;
use crate::registry::{MessageKind, TimelineKind};
use crate::{Message, MessageManager};
use bevy_ecs::change_detection::MutUntyped;
use bevy_ecs::component::Component;
use bevy_ecs::entity::Entity;
use bevy_ecs::event::EntityEvent;
use bevy_ecs::lifecycle::HookContext;
use bevy_ecs::world::{DeferredWorld, World};
use bevy_ecs::{event::Event, system::ParallelCommands};
use bevy_utils::prelude::DebugName;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_transport::channel::ChannelKind;
use lightyear_utils::collections::HashMap;
use lightyear_utils::ready_buffer::ReadyBuffer;

use lightyear_core::id::PeerId;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_transport::packet::message::MessageId;
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

#[derive(Component)]
#[require(MessageManager)]
#[component(on_add = EventReceiver::<M>::on_add_hook)]
pub(crate) struct EventReceiver<M: Event> {
    pending_by_timeline: HashMap<TimelineKind, ReadyBuffer<(Tick, u64), PendingRemoteEvent<M>>>,
    next_sequence: u64,
}

impl<M: Event> Default for EventReceiver<M> {
    fn default() -> Self {
        Self {
            pending_by_timeline: HashMap::default(),
            next_sequence: 0,
        }
    }
}

struct PendingRemoteEvent<M> {
    trigger: M,
    from: PeerId,
    remote_tick: Tick,
    target_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
}

impl<M: Event> EventReceiver<M> {
    pub(crate) fn ensure_pending_capacity(
        &self,
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        let pending_count: usize = self
            .pending_by_timeline
            .values()
            .map(ReadyBuffer::len)
            .sum();
        if pending_count >= config.max_pending_per_receiver {
            return Err(MessageError::PendingTimelineOverflow {
                limit: config.max_pending_per_receiver,
            });
        }
        Ok(())
    }

    pub(crate) fn push_pending(
        &mut self,
        trigger: M,
        from: PeerId,
        remote_tick: Tick,
        target_tick: Tick,
        target_timeline: TimelineKind,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        self.ensure_pending_capacity(config)?;
        let key = (target_tick, self.next_sequence);
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.pending_by_timeline
            .entry(target_timeline)
            .or_default()
            .push(
                key,
                PendingRemoteEvent {
                    trigger,
                    from,
                    remote_tick,
                    target_tick,
                    channel_kind,
                    message_id,
                },
            );
        Ok(())
    }

    pub(crate) fn release_timeline_until(
        &mut self,
        commands: &ParallelCommands,
        timeline: TimelineKind,
        tick: Tick,
    ) {
        let Some(pending) = self.pending_by_timeline.get_mut(&timeline) else {
            return;
        };
        for (_, event) in pending.drain_until(&(tick, u64::MAX)) {
            trace!(
                target: "lightyear_debug::message",
                kind = "event_interpolated_release",
                schedule = "PreUpdate",
                sample_point = "PreUpdate",
                event_name = core::any::type_name::<M>(),
                remote_tick = event.remote_tick.0,
                target_tick = event.target_tick.0,
                release_tick = tick.0,
                target_timeline = ?timeline,
                channel = ?event.channel_kind,
                message_id = ?event.message_id,
                "released interpolated remote event"
            );
            commands.command_scope(|mut c| {
                c.trigger(RemoteEvent {
                    trigger: event.trigger,
                    from: event.from,
                });
            });
        }
        if pending.is_empty() {
            self.pending_by_timeline.remove(&timeline);
        }
    }

    pub(crate) fn clear_pending_timelines(&mut self) {
        self.pending_by_timeline.clear();
        self.next_sequence = 0;
    }

    #[cfg(test)]
    pub(crate) fn num_pending_timeline_events(&self) -> usize {
        self.pending_by_timeline
            .values()
            .map(ReadyBuffer::len)
            .sum()
    }

    fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let mut entity_mut = world.entity_mut(context.entity);
            let mut message_manager = entity_mut.get_mut::<MessageManager>().unwrap();
            let message_kind_present = message_manager
                .receive_triggers
                .iter()
                .any(|(message_kind, _)| *message_kind == MessageKind::of::<M>());
            if !message_kind_present {
                message_manager
                    .receive_triggers
                    .push((MessageKind::of::<M>(), context.component_id));
            }
        })
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
    target_timeline: Option<TimelineKind>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId, // Add sender PeerId
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>;

pub(crate) type ReleaseTimelineEventFn = unsafe fn(
    receiver: MutUntyped,
    commands: &ParallelCommands,
    timeline: TimelineKind,
    tick: Tick,
);
pub(crate) type ClearPendingTimelineEventFn = unsafe fn(receiver: MutUntyped);

/// Receive a `TriggerEvent<M>`, deserialize it, and emit a `RemoteEvent<M>` event.
///
/// SAFETY: The `reader` must contain a valid serialized `TriggerEvent<M>`.
/// The `serialize_metadata` must correspond to the `TriggerEvent<M>` type.
pub(crate) unsafe fn receive_event_typed<M: Message + Event>(
    commands: &ParallelCommands,
    receiver: Option<MutUntyped>,
    reader: &mut Reader,
    channel_kind: ChannelKind,
    _channel_name: &'static str,
    remote_tick: Tick,
    message_id: Option<MessageId>,
    target_timeline: Option<TimelineKind>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError> {
    // we deserialize the message and send a MessageEvent
    let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
    trace!(
        "Received trigger message: {:?} from: {from:?}",
        DebugName::type_name::<M>()
    );
    if let Some(target_timeline) = target_timeline {
        if let Some(receiver) = receiver {
            let mut receiver = unsafe { receiver.with_type::<EventReceiver<M>>() };
            receiver.push_pending(
                message,
                from,
                remote_tick,
                remote_tick,
                target_timeline,
                channel_kind,
                message_id,
                config,
            )?;
        } else {
            commands.command_scope(|mut c| {
                c.trigger(RemoteEvent {
                    trigger: message,
                    from,
                });
            });
        }
    } else {
        commands.command_scope(|mut c| {
            c.trigger(RemoteEvent {
                trigger: message,
                from,
            });
        });
    }
    Ok(())
}

pub(crate) unsafe fn release_event_typed<M: Event>(
    receiver: MutUntyped,
    commands: &ParallelCommands,
    timeline: TimelineKind,
    tick: Tick,
) {
    // SAFETY: we know the receiver corresponds to EventReceiver<M>.
    let mut receiver = unsafe { receiver.with_type::<EventReceiver<M>>() };
    receiver.release_timeline_until(commands, timeline, tick);
}

pub(crate) unsafe fn clear_pending_event_typed<M: Event>(receiver: MutUntyped) {
    // SAFETY: we know the receiver corresponds to EventReceiver<M>.
    let mut receiver = unsafe { receiver.with_type::<EventReceiver<M>>() };
    receiver.clear_pending_timelines();
}

pub(crate) unsafe fn has_pending_event_typed<M: Event>(receiver: MutUntyped) -> bool {
    // SAFETY: we know the receiver corresponds to EventReceiver<M>.
    let receiver = unsafe { receiver.with_type::<EventReceiver<M>>() };
    receiver
        .pending_by_timeline
        .values()
        .any(|pending| !pending.is_empty())
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
    fn pending_timeline_events_are_bounded_per_receiver() {
        let mut receiver = EventReceiver::<TestEvent>::default();
        let config = TimelineMessageConfig {
            max_pending_per_receiver: 1,
            ..Default::default()
        };
        let timeline = TimelineKind::from(TypeId::of::<TestTimeline>());
        receiver
            .push_pending(
                TestEvent,
                PeerId::Local(1),
                Tick(1),
                Tick(1),
                timeline,
                ChannelKind::of::<TestChannel>(),
                None,
                &config,
            )
            .unwrap();
        let error = receiver
            .push_pending(
                TestEvent,
                PeerId::Local(1),
                Tick(2),
                Tick(2),
                timeline,
                ChannelKind::of::<TestChannel>(),
                None,
                &config,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            MessageError::PendingTimelineOverflow { limit: 1 }
        ));
        assert_eq!(receiver.num_pending_timeline_events(), 1);
    }
}
