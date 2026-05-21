//! Per-connection transport state.
//!
//! [`crate::transport::Transport`] is the component that connects Lightyear's channel layer to a
//! concrete [`Link`](lightyear_link::Link). It does not own sockets or sessions. Instead, it
//! packetizes channel messages into bytes for [`Link::send`](lightyear_link::Link::send), and
//! decodes bytes from [`Link::recv`](lightyear_link::Link::recv) back into channel receiver buffers.
//!
//! Channel behavior is configured elsewhere:
//! - [`ChannelSettings`](crate::channel::builder::ChannelSettings) defines delivery mode, send
//!   frequency, and channel priority.
//! - [`ChannelRegistry`](crate::channel::registry::ChannelRegistry) maps channel marker types to
//!   stable network IDs.
//! - [`TransportPlugin`](crate::plugin::TransportPlugin) drives packet receive/send systems around
//!   the lower-level `lightyear_link` schedules.

use crate::channel::Channel;
use crate::channel::builder::{ChannelMode, DEFAULT_MESSAGE_PRIORITY};
use crate::channel::receivers::ChannelReceiverEnum;
use crate::channel::registry::{ChannelId, ChannelKind, ChannelRegistry};
use crate::channel::senders::ChannelSend;
use crate::channel::senders::ChannelSenderEnum;
use crate::error::TransportError;
use crate::packet::message::{MessageAck, MessageId};
use crate::packet::packet::PacketId;
use crate::packet::packet_builder::{PacketBuilder, RecvPayload};
use crate::packet::priority_manager::{PriorityConfig, PriorityManager};
use alloc::{vec, vec::Vec};
use bevy_ecs::component::Component;
use bevy_platform::collections::HashMap;
use bevy_utils::prelude::DebugName;
use bytes::Bytes;
use crossbeam_channel::{Receiver, Sender};
use lightyear_link::{Link, SendPayload};
#[allow(unused_imports)]
use tracing::trace;

/// Per-connection packet and channel state.
///
/// Insert `Transport` on an entity that also owns a [`Link`]. The link is the
/// byte-oriented transport boundary; `Transport` is the message-oriented layer above it. It owns the
/// per-channel senders and receivers, packet header state, message-to-packet acknowledgement
/// bookkeeping, and optional bandwidth priority filtering for one remote peer.
///
/// In normal application code, channel setup is registry-driven:
/// [`AppChannelExt::add_channel`](crate::channel::registry::AppChannelExt::add_channel) registers a
/// channel type, and
/// [`ChannelRegistration::add_direction`](crate::channel::registry::ChannelRegistration::add_direction)
/// installs observers that populate new client/server transport entities with the matching sender
/// and receiver state.
///
/// To enqueue raw payload bytes, use [`send`](Self::send) or
/// [`send_with_priority`](Self::send_with_priority) with a channel marker type. Systems that need an
/// immediate tracked [`MessageId`] can use the `send_mut*` variants.
#[derive(Component)]
#[require(Link)]
pub struct Transport {
    /// Channel receivers keyed by the channel's stable network ID.
    pub receivers: HashMap<ChannelId, ReceiverMetadata>,
    /// Channel senders keyed by the channel's type-based identifier.
    pub senders: HashMap<ChannelKind, SenderMetadata>,
    /// Bandwidth priority filter shared by all senders on this transport.
    pub priority_manager: PriorityManager,
    /// Packet builder shared by all channels on this transport.
    pub(crate) packet_manager: PacketBuilder,

    // TODO: do a HashMap<MessageId, PacketId> instead?
    // - when we receive a packet, go through all messages and check which ones match the packet? there shouldn't be too many
    //   since packets are quickly acked/nacked. We could also have a map from packet_id to message_ids.
    // - if the message is fragmented, we need to ack num_fragment packets to actually receive the ack. Any nack results in a nack
    /// Mapping from sent packet IDs to tracked channel message IDs.
    ///
    /// Reliable and ack-watching channels use this to translate packet acknowledgements back into
    /// message acknowledgements. Entries are removed when the packet is acked or considered lost.
    pub(crate) packet_to_message_map: HashMap<PacketId, Vec<(ChannelKind, MessageAck)>>,
    /// Remaining fragment packet acknowledgements before a fragmented message is considered acked.
    pub(crate) fragment_acks: HashMap<MessageId, u64>,

    /// Thread-safe enqueue handle for messages that should be sent by this transport.
    ///
    /// Cloning this sender lets other systems or threads enqueue raw channel bytes without taking a
    /// mutable ECS borrow of [`Transport`]. The send system drains the paired
    /// [`recv_channel`](Self::recv_channel).
    pub send_channel: Sender<(ChannelKind, Bytes, f32)>,
    /// Receiver side of [`send_channel`](Self::send_channel), drained by the send system.
    pub recv_channel: Receiver<(ChannelKind, Bytes, f32)>,
    /// Legacy outgoing payload staging buffer.
    ///
    /// Current packet flushing writes directly to [`Link::send`](lightyear_link::Link::send). This
    /// buffer is still cleared during reset for compatibility with older code paths.
    pub send: Vec<SendPayload>,
    /// Legacy incoming payload staging buffer.
    ///
    /// Current receive processing drains [`Link::recv`](lightyear_link::Link::recv) directly. This
    /// buffer is still cleared during reset for compatibility with older code paths.
    pub recv: Vec<RecvPayload>,
}

impl Transport {
    /// Creates an empty transport using `priority_config` for bandwidth filtering.
    ///
    /// This does not add any channel senders or receivers. Register channels on the app and add
    /// directions through [`ChannelRegistration`](crate::channel::registry::ChannelRegistration), or
    /// manually call [`add_sender`](Self::add_sender) and [`add_receiver`](Self::add_receiver).
    pub fn new(priority_config: PriorityConfig) -> Self {
        let (send_channel, recv_channel) = crossbeam_channel::unbounded();
        Self {
            receivers: Default::default(),
            senders: Default::default(),
            priority_manager: PriorityManager::new(priority_config),
            packet_manager: PacketBuilder::default(),
            packet_to_message_map: Default::default(),
            fragment_acks: Default::default(),
            send_channel,
            recv_channel,
            send: vec![],
            recv: vec![],
        }
    }
}

impl Default for Transport {
    fn default() -> Self {
        Self::new(PriorityConfig::default())
    }
}

impl Transport {
    /// Returns `true` if this transport has a sender for channel `C`.
    pub fn has_sender<C: Channel>(&self) -> bool {
        self.senders.contains_key(&ChannelKind::of::<C>())
    }

    /// Returns `true` if this transport has a receiver for channel `C`.
    pub fn has_receiver<C: Channel>(&self) -> bool {
        self.receivers
            .values()
            .any(|m| m.channel_kind == ChannelKind::of::<C>())
    }

    /// Adds a pre-built sender implementation for channel `C`.
    ///
    /// Most applications should prefer registry-driven setup through
    /// [`ChannelRegistration::add_direction`](crate::channel::registry::ChannelRegistration::add_direction).
    /// This method is useful for lower-level integration tests and custom transport setup.
    pub fn add_sender<C: Channel>(
        &mut self,
        sender: ChannelSenderEnum,
        mode: ChannelMode,
        channel_id: ChannelId,
    ) {
        self.senders.insert(
            ChannelKind::of::<C>(),
            SenderMetadata {
                sender,
                message_acks: vec![],
                message_nacks: vec![],
                messages_sent: vec![],
                channel_id,
                mode,
                name: DebugName::type_name::<C>(),
            },
        );
    }

    // TODO: make this available via observer by triggering AddSender<C> on the Transport entity.
    /// Adds a sender for channel `C` using settings from `registry`.
    ///
    /// # Panics
    ///
    /// Panics if `C` has not been registered in `registry`.
    pub fn add_sender_from_registry<C: Channel>(&mut self, registry: &ChannelRegistry) {
        trace!(
            "Adding sender from registry for channel {}. Kind: {:?}",
            DebugName::type_name::<C>(),
            ChannelKind::of::<C>()
        );
        let Some(settings) = registry.settings(ChannelKind::of::<C>()) else {
            panic!(
                "ChannelSettings not found for channel {}",
                DebugName::type_name::<C>()
            );
        };
        let channel_id = *registry.get_net_from_kind(&ChannelKind::of::<C>()).unwrap();
        let sender = settings.into();
        self.add_sender::<C>(sender, settings.mode, channel_id);
    }

    /// Adds a pre-built receiver implementation for channel `C`.
    ///
    /// Most applications should prefer registry-driven setup through
    /// [`ChannelRegistration::add_direction`](crate::channel::registry::ChannelRegistration::add_direction).
    pub fn add_receiver<C: Channel>(
        &mut self,
        receiver: ChannelReceiverEnum,
        channel_id: ChannelId,
    ) {
        self.receivers.insert(
            channel_id,
            ReceiverMetadata {
                receiver,
                channel_kind: ChannelKind::of::<C>(),
            },
        );
    }

    /// Adds a receiver for channel `C` using settings from `registry`.
    ///
    /// # Panics
    ///
    /// Panics if `C` has not been registered in `registry`.
    pub fn add_receiver_from_registry<C: Channel>(&mut self, registry: &ChannelRegistry) {
        let Some(settings) = registry.settings(ChannelKind::of::<C>()) else {
            panic!(
                "ChannelSettings not found for channel {}",
                DebugName::type_name::<C>()
            );
        };
        let channel_id = *registry.get_net_from_kind(&ChannelKind::of::<C>()).unwrap();
        let receiver = settings.into();
        self.add_receiver::<C>(receiver, channel_id);
    }

    /// Queues `bytes` on channel `C` with an explicit message priority.
    ///
    /// The message is sent through an internal crossbeam channel, so this method only requires
    /// `&self` and can be called while other systems hold immutable access to the transport.
    /// Delivery, ack, and ordering semantics are determined by `C`'s
    /// [`ChannelMode`].
    pub fn send_with_priority<C: Channel>(
        &self,
        bytes: SendPayload,
        priority: f32,
    ) -> Result<(), TransportError> {
        self.send_erased(ChannelKind::of::<C>(), bytes, priority)
    }

    /// Queues `bytes` on channel `C` with [`DEFAULT_MESSAGE_PRIORITY`].
    pub fn send<C: Channel>(&self, bytes: SendPayload) -> Result<(), TransportError> {
        self.send_with_priority::<C>(bytes, DEFAULT_MESSAGE_PRIORITY)
    }

    /// Queues `bytes` on a channel identified by [`ChannelKind`].
    ///
    /// This erased variant is useful when the channel type is only known dynamically. The kind must
    /// correspond to a sender installed on this transport or the send system will later report
    /// [`TransportError::ChannelNotFound`].
    pub fn send_erased(
        &self,
        kind: ChannelKind,
        bytes: SendPayload,
        priority: f32,
    ) -> Result<(), TransportError> {
        self.send_channel.try_send((kind, bytes, priority))?;
        Ok(())
    }

    /// Queues `bytes` directly into channel `C`'s sender and returns its [`MessageId`] if tracked.
    ///
    /// This bypasses the internal crossbeam enqueue path and therefore requires `&mut self`. It is
    /// useful when callers need the message ID immediately, for example to correlate a reliable
    /// message with later ack/nack metadata.
    pub fn send_mut<C: Channel>(
        &mut self,
        bytes: SendPayload,
    ) -> Result<Option<MessageId>, TransportError> {
        self.send_mut_with_priority::<C>(bytes, DEFAULT_MESSAGE_PRIORITY)
    }

    /// Queues `bytes` directly into channel `C`'s sender with an explicit priority.
    pub fn send_mut_with_priority<C: Channel>(
        &mut self,
        bytes: SendPayload,
        priority: f32,
    ) -> Result<Option<MessageId>, TransportError> {
        self.send_mut_erased(ChannelKind::of::<C>(), bytes, priority)
    }

    /// Queues `bytes` directly into a dynamically selected channel sender.
    ///
    /// Returns [`TransportError::ChannelNotFound`] if this transport does not have a sender for
    /// `kind`.
    pub fn send_mut_erased(
        &mut self,
        kind: ChannelKind,
        bytes: SendPayload,
        priority: f32,
    ) -> Result<Option<MessageId>, TransportError> {
        let sender_metadata = self
            .senders
            .get_mut(&kind)
            .ok_or(TransportError::ChannelNotFound(kind))?;
        let message_id = sender_metadata.sender.buffer_send(bytes, priority);
        Ok(message_id)
    }

    /// Resets channel, packet, and queue state after disconnection.
    ///
    /// The channel topology is preserved from `registry`, but buffered messages, packet ack state,
    /// and legacy staging buffers are cleared.
    pub(crate) fn reset(&mut self, registry: &ChannelRegistry) {
        self.receivers.iter_mut().for_each(|(channel_id, r)| {
            let settings = registry.settings_from_net_id(*channel_id).unwrap();
            *r = ReceiverMetadata {
                receiver: settings.into(),
                channel_kind: r.channel_kind,
            };
        });
        self.senders.iter_mut().for_each(|(channel_kind, s)| {
            let settings = registry.settings(*channel_kind).unwrap();
            *s = SenderMetadata {
                sender: settings.into(),
                message_acks: vec![],
                message_nacks: vec![],
                messages_sent: vec![],
                channel_id: s.channel_id,
                mode: s.mode,
                name: s.name.clone(),
            };
        });
        self.priority_manager = Default::default();
        self.packet_manager = Default::default();
        self.packet_to_message_map = Default::default();
        let (send_channel, recv_channel) = crossbeam_channel::unbounded();
        self.send_channel = send_channel;
        self.recv_channel = recv_channel;
        self.recv.clear();
        self.send.clear();
    }
}

/// Receiver state and channel identity for one registered channel on a [`Transport`].
pub struct ReceiverMetadata {
    /// Concrete receiver implementation for this channel.
    pub receiver: ChannelReceiverEnum,
    /// Type-based channel identifier corresponding to [`receiver`](Self::receiver).
    pub channel_kind: ChannelKind,
}

#[doc(hidden)]
pub struct SenderMetadata {
    /// Concrete sender implementation for this channel.
    pub sender: ChannelSenderEnum,
    // TODO: these are currently only used by EntityUpdatesChannel. Maybe limit their computation only to that channel?
    /// List of messages that have been acked; cleared every frame.
    pub message_acks: Vec<MessageId>,
    /// List of messages that have been nacked; cleared every frame.
    pub message_nacks: Vec<MessageId>,
    /// List of messages that have been sent; cleared every frame.
    ///
    /// Buffering a message does not guarantee it is sent because [`PriorityManager`] may filter it.
    pub messages_sent: Vec<MessageId>,
    pub(crate) channel_id: ChannelId,
    pub(crate) mode: ChannelMode,
    pub(crate) name: DebugName,
}
