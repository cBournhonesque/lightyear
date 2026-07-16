use crate::MessageManager;
use crate::plugin::{MessagePlugin, TimelineMessageConfig};
use crate::registry::{
    MessageError, MessageReceiverKind, MessageRegistry, ReceiveTriggerMetadata, TimelineKind,
};
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
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_core::tick::Tick;
use lightyear_serde::ToBytes;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::prelude::Transport;
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
use core::marker::PhantomData;

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
/// The messages will be cleared every frame in the `Last` schedule. `T`
/// selects when messages become visible. The default [`LocalTimeline`] receiver
/// exposes messages immediately during the normal receive phase, while a
/// buffered network timeline keeps messages pending until its tick reaches the
/// sender tick.
#[derive(Component)]
#[require(MessageManager)]
#[component(on_add = MessageReceiver::<M, T>::on_add_hook)]
pub struct MessageReceiver<M: Message, T: IntoMessageReceiverTimeline = LocalTimeline> {
    pub(crate) buffer: T::Buffer<M>,
    /// Makes the timeline policy `T` an explicit part of this storage type.
    ///
    /// No `T` value is stored directly: `T` otherwise appears only through the
    /// associated-type projection `T::Buffer<M>`. `fn() -> T` makes this marker
    /// covariant in `T` without modeling ownership of a `T` or propagating its
    /// auto-trait and drop-check constraints. The receiver never consumes or
    /// mutably exposes a `T`, so it needs neither contravariance nor invariance.
    marker: PhantomData<fn() -> T>,
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

/// Buffer implementation selected by [`IntoMessageReceiverTimeline`].
///
/// This is public so custom timeline crates can implement the timeline policy,
/// but it is not intended to be used directly by applications.
#[doc(hidden)]
pub trait MessageReceiverBuffer<M: Message>: Default + Send + Sync + 'static {
    fn ready(&self) -> &Vec<ReceivedMessage<M>>;
    fn ready_mut(&mut self) -> &mut Vec<ReceivedMessage<M>>;
    fn push(
        &mut self,
        message: ReceivedMessage<M>,
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError>;
    fn release_until(&mut self, tick: Tick);
    fn clear_pending(&mut self);
    fn num_pending(&self) -> usize;
}

/// Type-level policy used by [`MessageReceiver`] to select its storage layout.
pub trait IntoMessageReceiverTimeline: Send + Sync + 'static {
    type Buffer<M: Message>: MessageReceiverBuffer<M>;

    /// Runtime timeline identity used to route channel payloads to this
    /// receiver. `None` is the normal immediate receive phase.
    fn timeline_kind() -> Option<TimelineKind>;
}

/// Marker implemented by network timelines that can buffer typed messages.
///
/// Implementing this trait automatically provides an
/// [`IntoMessageReceiverTimeline`] implementation backed by a timeline queue.
pub trait BufferedMessageTimeline: NetworkTimeline {}

/// Storage for the default immediate receiver.
#[doc(hidden)]
pub struct ImmediateMessageBuffer<M: Message> {
    ready: Vec<ReceivedMessage<M>>,
}

impl<M: Message> Default for ImmediateMessageBuffer<M> {
    fn default() -> Self {
        Self { ready: Vec::new() }
    }
}

impl<M: Message> MessageReceiverBuffer<M> for ImmediateMessageBuffer<M> {
    fn ready(&self) -> &Vec<ReceivedMessage<M>> {
        &self.ready
    }

    fn ready_mut(&mut self) -> &mut Vec<ReceivedMessage<M>> {
        &mut self.ready
    }

    fn push(
        &mut self,
        message: ReceivedMessage<M>,
        _config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        self.ready.push(message);
        Ok(())
    }

    fn release_until(&mut self, _tick: Tick) {}

    fn clear_pending(&mut self) {}

    fn num_pending(&self) -> usize {
        0
    }
}

impl IntoMessageReceiverTimeline for LocalTimeline {
    type Buffer<M: Message> = ImmediateMessageBuffer<M>;

    fn timeline_kind() -> Option<TimelineKind> {
        None
    }
}

/// Storage for receivers released by a buffered network timeline.
#[doc(hidden)]
pub struct TimelineMessageBuffer<M: Message> {
    ready: Vec<ReceivedMessage<M>>,
    pending: ReadyBuffer<(Tick, u64), ReceivedMessage<M>>,
    next_sequence: u64,
}

impl<M: Message> Default for TimelineMessageBuffer<M> {
    fn default() -> Self {
        Self {
            ready: Vec::new(),
            pending: ReadyBuffer::default(),
            next_sequence: 0,
        }
    }
}

impl<M: Message> MessageReceiverBuffer<M> for TimelineMessageBuffer<M> {
    fn ready(&self) -> &Vec<ReceivedMessage<M>> {
        &self.ready
    }

    fn ready_mut(&mut self) -> &mut Vec<ReceivedMessage<M>> {
        &mut self.ready
    }

    fn push(
        &mut self,
        message: ReceivedMessage<M>,
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        if self.pending.len() >= config.max_pending_per_receiver {
            return Err(MessageError::PendingTimelineOverflow {
                limit: config.max_pending_per_receiver,
            });
        }
        let key = (message.remote_tick, self.next_sequence);
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.pending.push(key, message);
        Ok(())
    }

    fn release_until(&mut self, tick: Tick) {
        self.ready.extend(
            self.pending
                .drain_until(&(tick, u64::MAX))
                .into_iter()
                .map(|(_, message)| message),
        );
    }

    fn clear_pending(&mut self) {
        self.pending.clear();
        self.next_sequence = 0;
    }

    fn num_pending(&self) -> usize {
        self.pending.len()
    }
}

impl<T: BufferedMessageTimeline> IntoMessageReceiverTimeline for T {
    type Buffer<M: Message> = TimelineMessageBuffer<M>;

    fn timeline_kind() -> Option<TimelineKind> {
        Some(TimelineKind::of::<T>())
    }
}

impl<M: Message, T: IntoMessageReceiverTimeline> Default for MessageReceiver<M, T> {
    fn default() -> Self {
        Self {
            buffer: T::Buffer::default(),
            marker: PhantomData,
        }
    }
}

// TODO: do we care about the channel that the message was sent from? user-specified message usually don't
impl<M: Message, T: IntoMessageReceiverTimeline> MessageReceiver<M, T> {
    fn ready(&self) -> &Vec<ReceivedMessage<M>> {
        self.buffer.ready()
    }

    fn ready_mut(&mut self) -> &mut Vec<ReceivedMessage<M>> {
        self.buffer.ready_mut()
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

    /// Returns the number of messages waiting for this receiver's delivery timeline.
    pub fn num_pending_timeline_messages(&self) -> usize {
        self.buffer.num_pending()
    }

    /// Releases messages whose sender tick has become visible.
    pub(crate) fn release_timeline_until(&mut self, tick: Tick) {
        self.buffer.release_until(tick);
    }

    /// Drops all messages waiting for a timeline, such as when a connection ends.
    pub(crate) fn clear_pending_timelines(&mut self) {
        self.buffer.clear_pending();
    }

    pub(crate) fn push_received(
        &mut self,
        data: M,
        remote_tick: Tick,
        channel_kind: ChannelKind,
        message_id: Option<MessageId>,
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        let received_message = ReceivedMessage {
            data,
            remote_tick,
            channel_kind,
            message_id,
        };
        self.buffer.push(received_message, config)
    }

    fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let mut entity_mut = world.entity_mut(context.entity);
            let mut message_manager = entity_mut.get_mut::<MessageManager>().unwrap();
            let receiver_kind = MessageReceiverKind::of::<M, T>();
            let receiver_present = message_manager
                .receive_messages
                .iter()
                .any(|(kind, _)| *kind == receiver_kind);
            if !receiver_present {
                message_manager
                    .receive_messages
                    .push((receiver_kind, context.component_id));
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
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>;

pub(crate) type ReceiveLocalMessageFn = unsafe fn(
    receiver: Option<MutUntyped>,
    commands: &ParallelCommands,
    entity: Entity,
    message: &mut dyn Any,
    remote_tick: Tick,
    channel_kind: ChannelKind,
    message_id: Option<MessageId>,
    config: &TimelineMessageConfig,
) -> Result<(), MessageError>;

/// Clear all messages in the [`MessageReceiver<M>`] buffer
pub(crate) type ClearMessageFn = unsafe fn(receiver: MutUntyped);

/// Release interpolation-timed messages in the [`MessageReceiver<M>`] buffer.
pub(crate) type ReleaseTimelineMessageFn = unsafe fn(receiver: MutUntyped, tick: Tick);

/// Drop messages waiting for a delivery timeline.
pub(crate) type ClearPendingTimelineMessageFn = unsafe fn(receiver: MutUntyped);

impl<M: Message, T: IntoMessageReceiverTimeline> MessageReceiver<M, T> {
    fn queue_received(
        commands: &ParallelCommands,
        entity: Entity,
        message: M,
        remote_tick: Tick,
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
                    .expect("message receiver was just inserted");
                if let Err(error) =
                    receiver.push_received(message, remote_tick, channel_kind, message_id, &config)
                {
                    error!(
                        "Error buffering message {:?} on lazily inserted receiver: {error:?}",
                        DebugName::type_name::<M>()
                    );
                }
            });
        });
    }

    fn ensure_new_receiver_capacity(config: &TimelineMessageConfig) -> Result<(), MessageError> {
        if T::timeline_kind().is_some() && config.max_pending_per_receiver == 0 {
            return Err(MessageError::PendingTimelineOverflow { limit: 0 });
        }
        Ok(())
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
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut ReceiveEntityMap,
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        let insert_receiver = receiver.is_none();
        if insert_receiver {
            Self::ensure_new_receiver_capacity(config)?;
        }
        let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
        if let Some(receiver) = receiver {
            // SAFETY: the callback and component id are registered for Self.
            let mut receiver = unsafe { receiver.with_type::<Self>() };
            receiver.push_received(message, remote_tick, channel_kind, message_id, config)?;
        } else {
            Self::queue_received(
                commands,
                entity,
                message,
                remote_tick,
                channel_kind,
                message_id,
                *config,
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
            target_timeline = ?T::timeline_kind(),
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
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        if let Some(receiver) = receiver.as_ref() {
            if T::timeline_kind().is_some() {
                // SAFETY: the callback and component id are registered for Self.
                let receiver = unsafe { receiver.as_ref().deref::<Self>() };
                if receiver.num_pending_timeline_messages() >= config.max_pending_per_receiver {
                    return Err(MessageError::PendingTimelineOverflow {
                        limit: config.max_pending_per_receiver,
                    });
                }
            }
        } else {
            Self::ensure_new_receiver_capacity(config)?;
        }
        let message = message
            .downcast_mut::<Option<M>>()
            .ok_or(MessageError::IncorrectType)?
            .take()
            .ok_or(MessageError::IncorrectType)?;
        if let Some(receiver) = receiver {
            // SAFETY: the callback and component id are registered for Self.
            let mut receiver = unsafe { receiver.with_type::<Self>() };
            receiver.push_received(message, remote_tick, channel_kind, message_id, config)
        } else {
            Self::queue_received(
                commands,
                entity,
                message,
                remote_tick,
                channel_kind,
                message_id,
                *config,
            );
            Ok(())
        }
    }

    pub(crate) unsafe fn clear_typed(receiver: MutUntyped) {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>() };
        receiver.ready_mut().clear();
    }

    pub(crate) unsafe fn release_timeline_typed(receiver: MutUntyped, tick: Tick) {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>() };
        receiver.release_timeline_until(tick);
    }

    pub(crate) unsafe fn clear_pending_timelines_typed(receiver: MutUntyped) {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>() };
        receiver.clear_pending_timelines();
    }
}

impl MessagePlugin {
    fn receive_message_bytes(
        bytes: Bytes,
        registry: &MessageRegistry,
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
        config: &TimelineMessageConfig,
    ) -> Result<(), MessageError> {
        trace!(
            "Received message (id:{message_id:?}) from peer {:?} on channel {channel_kind:?}. {entity:?}",
            remote_peer_id
        );
        let mut reader = Reader::from(bytes);
        // we receive the message NetId, and then deserialize the message
        let message_net_id = MessageNetId::from_bytes(&mut reader)?;
        if let Some(timeline) = target_timeline {
            let metadata = registry
                .timeline_metadata
                .get(&timeline)
                .ok_or(MessageError::TimelineNotRegistered(timeline))?;
            let entity_mut = receiver_query.get_mut(entity).unwrap();
            let Some(timeline_ptr) = entity_mut.get_by_id(metadata.component_id) else {
                return Err(MessageError::MissingTimeline(timeline));
            };
            // SAFETY: the callback is registered together with this timeline component id.
            let current_tick = unsafe { (metadata.tick_fn)(timeline_ptr) };
            let delta = tick - current_tick;
            if delta > 0 && delta as u32 > config.max_future_ticks {
                return Err(MessageError::TimelineTooFarAhead {
                    target: tick,
                    current: current_tick,
                    max_future_ticks: config.max_future_ticks,
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
        let receiver_kind = MessageReceiverKind::new(*message_kind, target_timeline);

        if let Some(recv_metadata) = registry.receive_metadata.get(&receiver_kind) {
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
                    serialize_fns,
                    &mut message_manager.entity_mapper.remote_to_local,
                    config,
                )
            }
        } else if let Some(trigger_metadata) = registry.receive_trigger.get(&receiver_kind) {
            match trigger_metadata {
                ReceiveTriggerMetadata::Immediate(metadata) => {
                    // SAFETY: the callback is registered for this event type.
                    unsafe {
                        (metadata.receive_trigger_fn)(
                            commands,
                            &mut reader,
                            channel_kind,
                            channel_name,
                            tick,
                            message_id,
                            serialize_fns,
                            &mut message_manager.entity_mapper.remote_to_local,
                            remote_peer_id,
                            config,
                        )
                    }
                }
                ReceiveTriggerMetadata::Timeline(metadata) => {
                    let mut entity_mut = receiver_query.get_mut(entity).unwrap();
                    let receiver = entity_mut.get_mut_by_id(metadata.component_id);
                    // SAFETY: when present, the receiver corresponds to the callback's queue type.
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
                            serialize_fns,
                            &mut message_manager.entity_mapper.remote_to_local,
                            remote_peer_id,
                            config,
                        )
                    }
                }
            }
        } else if let Some(timeline) = target_timeline {
            let has_message_receiver = registry
                .receive_metadata
                .keys()
                .any(|kind| kind.message == *message_kind);
            if has_message_receiver {
                Err(MessageError::MissingTimelineMessageReceiver(timeline))
            } else {
                Err(MessageError::MissingTimelineEventRegistration(timeline))
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
        commands: ParallelCommands,
        config: Res<TimelineMessageConfig>,
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
                            .and_then(|settings| settings.delivery_timeline())
                            .map(TimelineKind::from);
                        // we fake the message_id for host-client messages
                        if let Err(error) = Self::receive_message_bytes(
                            bytes,
                            &registry,
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
                            &config,
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
                                    .and_then(|settings| settings.delivery_timeline())
                                    .map(TimelineKind::from);
                                Self::receive_message_bytes(
                                    bytes,
                                    &registry,
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
                                    &config,
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

    impl IntoMessageReceiverTimeline for TestTimeline {
        type Buffer<M: Message> = TimelineMessageBuffer<M>;

        fn timeline_kind() -> Option<TimelineKind> {
            Some(TimelineKind::from(TypeId::of::<Self>()))
        }
    }

    impl IntoMessageReceiverTimeline for OtherTimeline {
        type Buffer<M: Message> = TimelineMessageBuffer<M>;

        fn timeline_kind() -> Option<TimelineKind> {
            Some(TimelineKind::from(TypeId::of::<Self>()))
        }
    }

    #[test]
    fn local_timeline_messages_are_immediately_ready() {
        let mut receiver = MessageReceiver::<TestMessage>::default();
        receiver
            .push_received(
                TestMessage("immediate"),
                Tick(10),
                ChannelKind::of::<TestChannel>(),
                None,
                &TimelineMessageConfig::default(),
            )
            .unwrap();

        assert_eq!(receiver.num_pending_timeline_messages(), 0);
        let received = receiver.receive_with_tick().next().unwrap();
        assert_eq!(received.data, TestMessage("immediate"));
        assert_eq!(received.remote_tick, Tick(10));
    }

    #[test]
    fn releases_messages_when_interpolation_tick_reaches_target() {
        let mut receiver = MessageReceiver::<TestMessage, TestTimeline>::default();
        receiver
            .push_received(
                TestMessage("future"),
                Tick(10),
                ChannelKind::of::<TestChannel>(),
                None,
                &TimelineMessageConfig::default(),
            )
            .unwrap();

        receiver.release_timeline_until(Tick(9));
        assert_eq!(receiver.num_messages(), 0);
        assert_eq!(receiver.num_pending_timeline_messages(), 1);

        receiver.release_timeline_until(Tick(10));
        let released = receiver.receive_with_tick().collect::<Vec<_>>();
        assert_eq!(released.len(), 1);
        assert_eq!(released[0].data, TestMessage("future"));
        assert_eq!(released[0].remote_tick, Tick(10));
    }

    #[test]
    fn releases_same_tick_messages_in_receive_order() {
        let mut receiver = MessageReceiver::<TestMessage, TestTimeline>::default();
        for message in ["first", "second", "third"] {
            receiver
                .push_received(
                    TestMessage(message),
                    Tick(5),
                    ChannelKind::of::<TestChannel>(),
                    None,
                    &TimelineMessageConfig::default(),
                )
                .unwrap();
        }

        receiver.release_timeline_until(Tick(7));
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
    fn each_timeline_type_owns_its_pending_buffer() {
        let mut first = MessageReceiver::<TestMessage, TestTimeline>::default();
        let mut other = MessageReceiver::<TestMessage, OtherTimeline>::default();
        first
            .push_received(
                TestMessage("first"),
                Tick(3),
                ChannelKind::of::<TestChannel>(),
                None,
                &TimelineMessageConfig::default(),
            )
            .unwrap();
        other
            .push_received(
                TestMessage("other"),
                Tick(3),
                ChannelKind::of::<TestChannel>(),
                None,
                &TimelineMessageConfig::default(),
            )
            .unwrap();

        first.release_timeline_until(Tick(3));
        assert_eq!(
            first.receive().collect::<Vec<_>>(),
            vec![TestMessage("first")]
        );
        assert_eq!(other.num_pending_timeline_messages(), 1);

        other.release_timeline_until(Tick(3));
        assert_eq!(
            other.receive().collect::<Vec<_>>(),
            vec![TestMessage("other")]
        );
    }

    #[test]
    fn clearing_pending_timelines_drops_messages_and_resets_sequence() {
        let mut receiver = MessageReceiver::<TestMessage, TestTimeline>::default();
        receiver
            .push_received(
                TestMessage("stale"),
                Tick(20),
                ChannelKind::of::<TestChannel>(),
                None,
                &TimelineMessageConfig::default(),
            )
            .unwrap();
        receiver.clear_pending_timelines();

        assert_eq!(receiver.num_pending_timeline_messages(), 0);
        assert_eq!(receiver.buffer.next_sequence, 0);
    }

    #[test]
    fn pending_timeline_messages_are_bounded_per_receiver() {
        let mut receiver = MessageReceiver::<TestMessage, TestTimeline>::default();
        let config = TimelineMessageConfig {
            max_pending_per_receiver: 1,
            ..Default::default()
        };
        receiver
            .push_received(
                TestMessage("first"),
                Tick(10),
                ChannelKind::of::<TestChannel>(),
                None,
                &config,
            )
            .unwrap();
        let error = receiver
            .push_received(
                TestMessage("overflow"),
                Tick(11),
                ChannelKind::of::<TestChannel>(),
                None,
                &config,
            )
            .unwrap_err();

        assert!(matches!(
            error,
            MessageError::PendingTimelineOverflow { limit: 1 }
        ));
        assert_eq!(receiver.num_pending_timeline_messages(), 1);
    }
}
