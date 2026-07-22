use crate::MessageManager;
use crate::plugin::{MAX_PENDING_TIMELINE_PAYLOADS, MAX_TIMELINE_LAG_TICKS, MessagePlugin};
use crate::registry::{MessageError, MessageKind, MessageRegistry};
use crate::{Message, MessageNetId};
use alloc::vec::Vec;
use bevy_ecs::{
    change_detection::MutUntyped,
    component::Component,
    entity::Entity,
    event::Event,
    query::With,
    system::{ParallelCommands, Query, Res},
    world::{DeferredWorld, FilteredEntityMut, World},
};
use lightyear_core::prelude::{TimelineKind, TimelineRegistry};
use lightyear_core::tick::Tick;
use lightyear_serde::ToBytes;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::prelude::Transport;
use lightyear_utils::collections::HashMap;
use lightyear_utils::ready_buffer::ReadyBuffer;

use alloc::sync::Arc;
use bevy_ecs::lifecycle::HookContext;
use bevy_utils::prelude::DebugName;
use bytes::Bytes;
use lightyear_connection::client::Connected;
use lightyear_connection::host::HostClient;
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_transport::packet::message::MessageId;
use lightyear_transport::prelude::ChannelRegistry;
use tracing::{error, trace};

use core::any::Any;

/// Bevy Trigger emitted when a remote trigger is received and processed.
///
/// Contains the original trigger `M` and the [`PeerId`] of the sender.
#[derive(Event)]
pub struct RemoteOn<M: Message> {
    pub trigger: M,
    pub from: PeerId,
}

/// A component that receives messages of type `M` from the network.
///
/// Messages received from the network are stored in this receiver's ready
/// buffer. Call [`receive`](Self::receive) to drain and process them.
///
/// The messages will be cleared every frame in the `Last` schedule.
/// Messages received on an immediate channel are ready during the normal
/// receive phase. Messages received on a timeline channel remain pending in
/// this same component until that channel's timeline reaches the sender tick.
#[derive(Component)]
#[require(MessageManager)]
#[component(on_add = MessageReceiver::<M>::on_add_hook)]
pub struct MessageReceiver<M: Message> {
    ready: Vec<ReceivedMessage<M>>,
    pending: HashMap<TimelineKind, TimelineMessageBuffer<M>>,
}

#[derive(Debug)]
pub struct ReceivedMessage<M: Message> {
    pub data: M,
    /// Tick on the remote peer when the message was sent,
    pub remote_tick: Tick,
    /// Channel that was used to send the message
    pub channel_kind: ChannelKind,
    /// MessageId of the message
    pub message_id: Option<MessageId>,
}

/// Read-only per-message metadata handed to
/// [`MessageReceiver::retain_received_messages`] validators alongside `&mut`
/// access to the message data.
#[derive(Debug, Clone, Copy)]
pub struct MessageMetadata {
    /// Tick on the remote peer when the message was sent.
    pub remote_tick: Tick,
    /// Channel the message was sent on.
    pub channel_kind: ChannelKind,
    /// MessageId of the message, if the channel assigns one.
    pub message_id: Option<MessageId>,
}

struct TimelineMessageBuffer<M: Message> {
    pending: ReadyBuffer<(Tick, u64), ReceivedMessage<M>>,
    next_sequence: u64,
}

impl<M: Message> Default for TimelineMessageBuffer<M> {
    fn default() -> Self {
        Self {
            pending: ReadyBuffer::default(),
            next_sequence: 0,
        }
    }
}

impl<M: Message> TimelineMessageBuffer<M> {
    fn push(&mut self, message: ReceivedMessage<M>) -> Result<(), MessageError> {
        if self.pending.len() >= MAX_PENDING_TIMELINE_PAYLOADS {
            return Err(MessageError::PendingTimelineOverflow {
                limit: MAX_PENDING_TIMELINE_PAYLOADS,
            });
        }
        let key = (message.remote_tick, self.next_sequence);
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.pending.push(key, message);
        Ok(())
    }

    fn release_until(&mut self, tick: Tick) -> impl Iterator<Item = ReceivedMessage<M>> + '_ {
        self.pending
            .drain_until(&(tick, u64::MAX))
            .into_iter()
            .map(|(_, message)| message)
    }

    fn num_pending(&self) -> usize {
        self.pending.len()
    }
}

impl<M: Message> Default for MessageReceiver<M> {
    fn default() -> Self {
        Self {
            ready: Vec::new(),
            pending: HashMap::default(),
        }
    }
}

// TODO: do we care about the channel that the message was sent from? user-specified message usually don't
impl<M: Message> MessageReceiver<M> {
    fn ready(&self) -> &Vec<ReceivedMessage<M>> {
        &self.ready
    }

    fn ready_mut(&mut self) -> &mut Vec<ReceivedMessage<M>> {
        &mut self.ready
    }

    pub fn has_messages(&self) -> bool {
        !self.ready().is_empty()
    }

    /// Take all messages from the [`MessageReceiver<M>`], deserialize them, and return them
    pub fn receive(&mut self) -> impl Iterator<Item = M> {
        self.ready_mut().drain(..).map(|m| m.data)
    }

    /// Take all messages from the [`MessageReceiver<M>`], deserialize them, and return them
    pub fn receive_with_tick(&mut self) -> impl Iterator<Item = ReceivedMessage<M>> {
        self.ready_mut().drain(..)
    }

    /// Mutate and/or drop the buffered messages in place, *before* they are
    /// consumed by the receiving system.
    ///
    /// This is the hook for validation/sanitization systems that run between
    /// message receipt and whatever consumes the messages (e.g. server-side
    /// input validation between `MessageSystems::Receive` and the input-buffer
    /// apply). Returning `false` from `keep` drops that message; mutating the
    /// `&mut M` rewrites it. Per-message metadata (remote tick, channel,
    /// message id) is preserved automatically — unlike drain-then-re-push.
    pub fn retain_messages(&mut self, mut keep: impl FnMut(&mut M) -> bool) {
        self.ready_mut()
            .retain_mut(|received| keep(&mut received.data));
    }

    /// Like [`retain_messages`](Self::retain_messages), but the predicate also
    /// gets the per-message [`MessageMetadata`] (`remote_tick`, channel, and
    /// message id) that `retain_messages` hides.
    ///
    /// Use this when validation needs the metadata, e.g. rate limiting,
    /// tick-window / staleness checks, replay diagnostics, or per-channel
    /// policy. The metadata is passed **by value (read-only)** — only the
    /// message data is `&mut` (mutate to rewrite, return `false` to drop) — so a
    /// validator can't accidentally rewrite the wire metadata.
    pub fn retain_received_messages(
        &mut self,
        mut keep: impl FnMut(MessageMetadata, &mut M) -> bool,
    ) {
        self.ready_mut().retain_mut(|received| {
            let metadata = MessageMetadata {
                remote_tick: received.remote_tick,
                channel_kind: received.channel_kind,
                message_id: received.message_id,
            };
            keep(metadata, &mut received.data)
        });
    }

    pub fn num_messages(&self) -> usize {
        self.ready().len()
    }

    /// Returns the total number of messages waiting across delivery timelines.
    pub fn num_pending_timeline_messages(&self) -> usize {
        self.pending
            .values()
            .map(TimelineMessageBuffer::num_pending)
            .sum()
    }

    /// Releases messages for `timeline` whose sender tick has become visible.
    pub(crate) fn release_timeline_until(&mut self, timeline: TimelineKind, tick: Tick) {
        if let Some(pending) = self.pending.get_mut(&timeline) {
            self.ready.extend(pending.release_until(tick));
        }
    }

    pub(crate) fn push_received(
        &mut self,
        data: M,
        remote_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        target_timeline: Option<TimelineKind>,
    ) -> Result<(), MessageError> {
        let received_message = ReceivedMessage {
            data,
            remote_tick,
            channel_kind,
            message_id,
        };
        if let Some(timeline) = target_timeline {
            self.pending
                .entry(timeline)
                .or_default()
                .push(received_message)
        } else {
            self.ready.push(received_message);
            Ok(())
        }
    }

    fn ensure_capacity(&self, target_timeline: Option<TimelineKind>) -> Result<(), MessageError> {
        if let Some(timeline) = target_timeline
            && self
                .pending
                .get(&timeline)
                .is_some_and(|pending| pending.num_pending() >= MAX_PENDING_TIMELINE_PAYLOADS)
        {
            return Err(MessageError::PendingTimelineOverflow {
                limit: MAX_PENDING_TIMELINE_PAYLOADS,
            });
        }
        Ok(())
    }

    fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let mut entity_mut = world.entity_mut(context.entity);
            let mut message_manager = entity_mut.get_mut::<MessageManager>().unwrap();
            let receiver_present = message_manager
                .receive_messages
                .iter()
                .any(|(kind, _)| *kind == MessageKind::of::<M>());
            if !receiver_present {
                message_manager
                    .receive_messages
                    .push((MessageKind::of::<M>(), context.component_id));
            }
        })
    }
}

pub(crate) type ReceiveMessageFn = unsafe fn(
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
) -> Result<(), MessageError>;

pub(crate) type ReceiveLocalMessageFn = unsafe fn(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
    message: &mut dyn Any,
    remote_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
    target_timeline: Option<TimelineKind>,
) -> Result<(), MessageError>;

/// Clear all messages in the [`MessageReceiver<M>`] buffer
pub(crate) type ClearMessageFn = unsafe fn(receiver: MutUntyped);

/// Release interpolation-timed messages in the [`MessageReceiver<M>`] buffer.
pub(crate) type ReleaseTimelineMessageFn =
    unsafe fn(receiver: MutUntyped, timeline: TimelineKind, tick: Tick);

impl<M: Message> MessageReceiver<M> {
    fn queue_received(
        commands: &ParallelCommands,
        entity: Entity,
        message: M,
        remote_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        target_timeline: Option<TimelineKind>,
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
                    .expect("message receiver was just inserted");
                if let Err(error) = receiver.push_received(
                    message,
                    remote_tick,
                    channel_kind,
                    message_id,
                    target_timeline,
                ) {
                    error!(
                        "Error buffering message {:?} on lazily inserted receiver: {error:?}",
                        DebugName::type_name::<M>()
                    );
                }
            });
        });
    }

    /// Receive a single message of type `M` from the channel
    ///
    /// SAFETY: when present, `receiver` must be of type [`MessageReceiver<M>`],
    /// and the message bytes must be a valid serialized message of type `M`.
    pub(crate) unsafe fn receive_message_typed(
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
    ) -> Result<(), MessageError> {
        let insert_receiver = receiver.is_none();
        let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
        if let Some(receiver) = receiver {
            // SAFETY: the callback and component id are registered for Self.
            let mut receiver = unsafe { receiver.with_type::<Self>() };
            receiver.push_received(
                message,
                remote_tick,
                channel_kind,
                message_id,
                target_timeline,
            )?;
        } else {
            Self::queue_received(
                commands,
                entity,
                message,
                remote_tick,
                channel_kind,
                message_id,
                target_timeline,
            );
        }
        trace!(
            "Received message {:?} on channel {channel_kind:?}",
            DebugName::type_name::<M>()
        );
        trace!(
            target: "lightyear_debug::message",
            kind = "message_receive_typed",
            schedule = "PreUpdate",
            sample_point = "PreUpdate",
            message_name = core::any::type_name::<M>(),
            channel = channel_name,
            remote_tick = remote_tick.0,
            target_timeline = ?target_timeline,
            message_id = ?message_id,
            insert_receiver,
            "deserialized message into receiver"
        );
        Ok(())
    }

    pub(crate) unsafe fn receive_local_message_typed(
        receiver: Option<MutUntyped>,
        commands: &ParallelCommands,
        entity: Entity,
        message: &mut dyn Any,
        remote_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        target_timeline: Option<TimelineKind>,
    ) -> Result<(), MessageError> {
        if let Some(receiver) = receiver.as_ref() {
            // SAFETY: the callback and component id are registered for Self.
            let receiver = unsafe { receiver.as_ref().deref::<Self>() };
            receiver.ensure_capacity(target_timeline)?;
        }
        let message = message
            .downcast_mut::<Option<M>>()
            .ok_or(MessageError::IncorrectType)?
            .take()
            .ok_or(MessageError::IncorrectType)?;
        if let Some(receiver) = receiver {
            // SAFETY: the callback and component id are registered for Self.
            let mut receiver = unsafe { receiver.with_type::<Self>() };
            receiver.push_received(
                message,
                remote_tick,
                channel_kind,
                message_id,
                target_timeline,
            )
        } else {
            Self::queue_received(
                commands,
                entity,
                message,
                remote_tick,
                channel_kind,
                message_id,
                target_timeline,
            );
            Ok(())
        }
    }

    pub(crate) unsafe fn clear_typed(receiver: MutUntyped) {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>() };
        receiver.ready_mut().clear();
    }

    pub(crate) unsafe fn release_timeline_typed(
        receiver: MutUntyped,
        timeline: TimelineKind,
        tick: Tick,
    ) {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>() };
        receiver.release_timeline_until(timeline, tick);
    }
}

impl MessagePlugin {
    fn receive_message_bytes(
        bytes: Bytes,
        registry: &MessageRegistry,
        timeline_registry: &TimelineRegistry,
        receiver_query: &mut Query<FilteredEntityMut>,
        entity: Entity,
        channel_kind: ChannelKind,
        channel_name: &'static str,
        tick: Tick,
        message_id: Option<MessageId>,
        target_timeline: Option<TimelineKind>,
        message_manager: &mut MessageManager,
        commands: &ParallelCommands,
        remote_peer_id: PeerId,
    ) -> Result<(), MessageError> {
        trace!(
            "Received message (id:{message_id:?}) from peer {:?} on channel {channel_kind:?}. {entity:?}",
            remote_peer_id
        );
        let mut reader = Reader::from(bytes);
        // we receive the message NetId, and then deserialize the message
        let message_net_id = MessageNetId::from_bytes(&mut reader)?;
        if let Some(timeline) = target_timeline {
            let metadata = timeline_registry
                .get(&timeline)
                .ok_or(MessageError::TimelineNotRegistered(timeline))?;
            let entity_mut = receiver_query.get_mut(entity).unwrap();
            let Some(timeline_ptr) = entity_mut.get_by_id(metadata.component_id()) else {
                return Err(MessageError::MissingTimeline(timeline));
            };
            // SAFETY: the metadata is registered together with this timeline component id.
            let current_tick = unsafe { metadata.tick(timeline_ptr) };
            let delta = tick - current_tick;
            if delta > 0 && delta as u32 > MAX_TIMELINE_LAG_TICKS {
                return Err(MessageError::TimelineTooFarBehind {
                    target: tick,
                    current: current_tick,
                    max_lag_ticks: MAX_TIMELINE_LAG_TICKS,
                });
            }
        }
        let message_kind = registry
            .kind_map
            .kind(message_net_id)
            .ok_or(MessageError::UnrecognizedMessageId(message_net_id))?;
        let message_name = registry.kind_map.name(message_kind).unwrap_or("Unknown");
        trace!(
            target: "lightyear_debug::message",
            kind = "message_receive_bytes",
            schedule = "PreUpdate",
            sample_point = "PreUpdate",
            entity = ?entity,
            message_name = message_name,
            message_net_id = message_net_id,
            channel = channel_name,
            remote_tick = tick.0,
            target_timeline = ?target_timeline,
            message_id = ?message_id,
            remote_peer = ?remote_peer_id,
            "received message bytes"
        );
        let serialize_fns = registry
            .serialize_fns_map
            .get(message_kind)
            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
        if let Some(recv_metadata) = registry.receive_metadata.get(message_kind) {
            let component_id = recv_metadata.component_id;
            let mut entity_mut = receiver_query.get_mut(entity).unwrap();
            let receiver = entity_mut.get_mut_by_id(component_id);
            // SAFETY: when present, the receiver corresponds to the callback's concrete type.
            unsafe {
                (recv_metadata.receive_message_fn)(
                    receiver,
                    commands,
                    entity,
                    &mut reader,
                    channel_kind,
                    channel_name,
                    tick,
                    message_id,
                    target_timeline,
                    serialize_fns,
                    &mut message_manager.entity_mapper.remote_to_local,
                )
            }
        } else if let Some(metadata) = registry.receive_trigger.get(message_kind) {
            let mut entity_mut = receiver_query.get_mut(entity).unwrap();
            let receiver = entity_mut.get_mut_by_id(metadata.component_id);
            // SAFETY: when present, the receiver corresponds to this event's pending component.
            unsafe {
                (metadata.receive_trigger_fn)(
                    receiver,
                    commands,
                    entity,
                    &mut reader,
                    channel_kind,
                    channel_name,
                    tick,
                    message_id,
                    target_timeline,
                    serialize_fns,
                    &mut message_manager.entity_mapper.remote_to_local,
                    remote_peer_id,
                )
            }
        } else {
            Err(MessageError::UnrecognizedMessageId(message_net_id))
        }
    }

    /// Receive bytes from each channel of the Transport
    /// Deserialize the bytes into Messages.
    /// - If the message is a `RemoteOn<M>`, emit a `TriggerEvent<M>` via `Commands`.
    /// - Otherwise, buffer the message in the `MessageReceiver<M>` component.
    pub fn recv(
        // NOTE: we only need the mut bound on MessageManager because EntityMapper requires mut
        mut transport_query: Query<
            // note: we still listen for messages on the Transport for the host-client, because of the way
            //  MultiMessageSender works. (it simply serializes messages to the Transport instead of writing
            //  them directly to the host-server's MessageReceiver<M>)
            (
                Entity,
                &mut MessageManager,
                &mut Transport,
                &RemoteId,
                Option<&mut HostClient>,
            ),
            With<Connected>,
        >,
        // List of ChannelReceivers<M> present on that entity
        receiver_query: Query<FilteredEntityMut>,
        registry: Res<MessageRegistry>,
        channel_registry: Res<ChannelRegistry>,
        timeline_registry: Res<TimelineRegistry>,
        commands: ParallelCommands,
    ) {
        // We use Arc to make the query Clone, since we know that we will only access MessageReceiver<M> components
        // on potentially different entities in parallel (though the current loop isn't parallel)
        let receiver_query = Arc::new(receiver_query);
        transport_query.par_iter_mut().for_each(
            |(
                entity,
                mut message_manager,
                mut transport,
                remote_peer_id,
                mut host_client,
            )| {
                // SAFETY: we know that this won't lead to violating the aliasing rule
                let mut receiver_query = unsafe { receiver_query.reborrow_unsafe() };
                // enable split borrows
                let transport = &mut *transport;
                // TODO: we can run this in parallel using rayon!
                if let Some(host_client) = host_client.as_mut() {
                    // for host-clients, we might have to deserialize messages that are in the Transports' senders
                    let buffered = core::mem::take(&mut host_client.buffer);
                    let mut buffered = buffered.into_iter();
                    while let Some((bytes, channel_type_id, tick)) = buffered.next() {
                        let channel_kind = ChannelKind(channel_type_id);
                        trace!("Received local message bytes from server on host-client {entity:?} on channel {channel_kind:?}");
                        let target_timeline = channel_registry
                            .settings(channel_kind)
                            .and_then(|settings| settings.timeline);
                        // we fake the message_id for host-client messages
                        if let Err(error) = Self::receive_message_bytes(
                            bytes,
                            &registry,
                            &timeline_registry,
                            &mut receiver_query,
                            entity,
                            channel_kind,
                            channel_registry.get_name_from_kind(&channel_kind),
                            tick,
                            None,
                            target_timeline,
                            &mut message_manager,
                            &commands,
                            remote_peer_id.0,
                        ) {
                            host_client.buffer.extend(buffered);
                            error!("Error receiving messages: {error:?}");
                            break;
                        }
                    }
                } else {
                    transport
                        .receivers
                        .values_mut()
                        .try_for_each(|receiver_metadata| {
                            let channel_kind = receiver_metadata.channel_kind;
                            let channel_name = channel_registry.get_name_from_kind(&channel_kind);
                            while let Some((tick, bytes, message_id)) =
                                receiver_metadata.receiver.read_message()
                            {
                                let target_timeline = channel_registry
                                    .settings(channel_kind)
                                    .and_then(|settings| settings.timeline);
                                Self::receive_message_bytes(
                                    bytes,
                                    &registry,
                                    &timeline_registry,
                                    &mut receiver_query,
                                    entity,
                                    channel_kind,
                                    channel_name,
                                    tick,
                                    message_id,
                                    target_timeline,
                                    &mut message_manager,
                                    &commands,
                                    remote_peer_id.0,
                                )?;
                            }
                            Ok::<_, MessageError>(())
                        })
                        .inspect_err(|e| error!("Error receiving messages: {e:?}"))
                        .ok();
                }
            },
        )
    }

    /// Clear all the message receivers to prevent messages from accumulating
    pub fn clear(
        manager_query: Query<(Entity, &MessageManager), With<Connected>>,
        mut receiver_query: Query<FilteredEntityMut>,
        registry: Res<MessageRegistry>,
    ) {
        manager_query.iter().for_each(|(entity, manager)| {
            manager
                .receive_messages
                .iter()
                .for_each(|(kind, component_id)| {
                    let mut entity_mut = receiver_query.get_mut(entity).unwrap();
                    let receiver = entity_mut.get_mut_by_id(*component_id).unwrap();
                    let clear_fn = registry
                        .receive_metadata
                        .get(kind)
                        .unwrap()
                        .message_clear_fn;
                    // SAFETY: we know that we are calling the function for the correct component_id
                    unsafe { clear_fn(receiver) };
                })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use core::any::TypeId;

    #[derive(Debug, PartialEq)]
    struct TestMessage(&'static str);

    struct TestChannel;

    struct TestTimeline;
    struct OtherTimeline;

    #[test]
    fn local_timeline_messages_are_immediately_ready() {
        let mut receiver = MessageReceiver::<TestMessage>::default();
        receiver
            .push_received(
                TestMessage("immediate"),
                Tick(10),
                ChannelKind::of::<TestChannel>(),
                None,
                None,
            )
            .unwrap();

        assert_eq!(receiver.num_pending_timeline_messages(), 0);
        let received = receiver.receive_with_tick().next().unwrap();
        assert_eq!(received.data, TestMessage("immediate"));
        assert_eq!(received.remote_tick, Tick(10));
    }

    #[test]
    fn releases_messages_when_interpolation_tick_reaches_target() {
        let mut receiver = MessageReceiver::<TestMessage>::default();
        let timeline = TimelineKind::from(TypeId::of::<TestTimeline>());
        receiver
            .push_received(
                TestMessage("future"),
                Tick(10),
                ChannelKind::of::<TestChannel>(),
                None,
                Some(timeline),
            )
            .unwrap();

        receiver.release_timeline_until(timeline, Tick(9));
        assert_eq!(receiver.num_messages(), 0);
        assert_eq!(receiver.num_pending_timeline_messages(), 1);

        receiver.release_timeline_until(timeline, Tick(10));
        let released = receiver.receive_with_tick().collect::<Vec<_>>();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].data, TestMessage("future"));
        assert_eq!(released[0].remote_tick, Tick(10));
    }

    #[test]
    fn releases_same_tick_messages_in_receive_order() {
        let mut receiver = MessageReceiver::<TestMessage>::default();
        let timeline = TimelineKind::from(TypeId::of::<TestTimeline>());
        for message in ["first", "second", "third"] {
            receiver
                .push_received(
                    TestMessage(message),
                    Tick(5),
                    ChannelKind::of::<TestChannel>(),
                    None,
                    Some(timeline),
                )
                .unwrap();
        }

        receiver.release_timeline_until(timeline, Tick(7));
        let released = receiver.receive().collect::<Vec<_>>();
        assert_eq!(
            released,
            vec![
                TestMessage("first"),
                TestMessage("second"),
                TestMessage("third")
            ]
        );
    }

    #[test]
    fn each_timeline_has_its_own_pending_buffer() {
        let mut receiver = MessageReceiver::<TestMessage>::default();
        let first = TimelineKind::from(TypeId::of::<TestTimeline>());
        let other = TimelineKind::from(TypeId::of::<OtherTimeline>());
        receiver
            .push_received(
                TestMessage("first"),
                Tick(3),
                ChannelKind::of::<TestChannel>(),
                None,
                Some(first),
            )
            .unwrap();
        receiver
            .push_received(
                TestMessage("other"),
                Tick(3),
                ChannelKind::of::<TestChannel>(),
                None,
                Some(other),
            )
            .unwrap();

        receiver.release_timeline_until(first, Tick(3));
        assert_eq!(
            receiver.receive().collect::<Vec<_>>(),
            vec![TestMessage("first")]
        );
        assert_eq!(receiver.num_pending_timeline_messages(), 1);

        receiver.release_timeline_until(other, Tick(3));
        assert_eq!(
            receiver.receive().collect::<Vec<_>>(),
            vec![TestMessage("other")]
        );
    }

    #[test]
    fn pending_timeline_messages_are_bounded_per_receiver() {
        let mut receiver = MessageReceiver::<TestMessage>::default();
        let timeline = TimelineKind::from(TypeId::of::<TestTimeline>());
        for _ in 0..MAX_PENDING_TIMELINE_PAYLOADS {
            receiver
                .push_received(
                    TestMessage("pending"),
                    Tick(10),
                    ChannelKind::of::<TestChannel>(),
                    None,
                    Some(timeline),
                )
                .unwrap();
        }
        let error = receiver
            .push_received(
                TestMessage("overflow"),
                Tick(11),
                ChannelKind::of::<TestChannel>(),
                None,
                Some(timeline),
            )
            .unwrap_err();

        assert!(matches!(
            error,
            MessageError::PendingTimelineOverflow {
                limit: MAX_PENDING_TIMELINE_PAYLOADS
            }
        ));
        assert_eq!(
            receiver.num_pending_timeline_messages(),
            MAX_PENDING_TIMELINE_PAYLOADS
        );
    }
}
