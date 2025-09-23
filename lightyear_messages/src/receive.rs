use crate::MessageManager;
use crate::plugin::MessagePlugin;
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
use lightyear_core::tick::Tick;
use lightyear_serde::ToBytes;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::prelude::Transport;

use alloc::sync::Arc;
use bevy_ecs::lifecycle::HookContext;
use bytes::Bytes;
use lightyear_connection::client::Connected;
use lightyear_connection::host::HostClient;
use lightyear_core::id::{PeerId, RemoteId};
use lightyear_core::prelude::{LocalTimeline, NetworkTimeline};
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_transport::packet::message::MessageId;
use tracing::{error, trace};

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
/// The components received from the network will be buffered in the `recv` field.
/// You can call the `receive` method to drain the messages from the buffer and process them.
///
/// The messages will be cleared every frame in the `Last` schedule.
#[derive(Component)]
#[require(MessageManager)]
#[component(on_add = MessageReceiver::<M>::on_add_hook)]
pub struct MessageReceiver<M: Message> {
    // TODO: wrap this in bevy events buffer?
    pub(crate) recv: Vec<ReceivedMessage<M>>,
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

impl<M: Message> Default for MessageReceiver<M> {
    fn default() -> Self {
        Self { recv: Vec::new() }
    }
}

// TODO: do we care about the channel that the message was sent from? user-specified message usually don't
// TODO: we have access to the Tick, so we could decide at which timeline we want to receive the message!
impl<M: Message> MessageReceiver<M> {
    pub fn has_messages(&self) -> bool {
        !self.recv.is_empty()
    }

    /// Take all messages from the [`MessageReceiver<M>`], deserialize them, and return them
    pub fn receive(&mut self) -> impl Iterator<Item = M> {
        self.recv.drain(..).map(|m| m.data)
    }

    /// Take all messages from the [`MessageReceiver<M>`], deserialize them, and return them
    pub fn receive_with_tick(&mut self) -> impl Iterator<Item = ReceivedMessage<M>> {
        self.recv.drain(..)
    }

    pub fn num_messages(&self) -> usize {
        self.recv.len()
    }

    fn on_add_hook(mut world: DeferredWorld, context: HookContext) {
        world.commands().queue(move |world: &mut World| {
            let mut entity_mut = world.entity_mut(context.entity);
            let mut message_manager = entity_mut.get_mut::<MessageManager>().unwrap();
            let message_kind_present = message_manager
                .receive_messages
                .iter()
                .any(|(message_kind, _)| *message_kind == MessageKind::of::<M>());
            if !message_kind_present {
                message_manager
                    .receive_messages
                    .push((MessageKind::of::<M>(), context.component_id));
            }
        })
    }
}

pub(crate) type ReceiveMessageFn = unsafe fn(
    receiver: MutUntyped,
    reader: &mut Reader,
    channel_kind: ChannelKind,
    remote_tick: Tick,
    message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
) -> Result<(), MessageError>;

/// Clear all messages in the [`MessageReceiver<M>`] buffer
pub(crate) type ClearMessageFn = unsafe fn(receiver: MutUntyped);

impl<M: Message> MessageReceiver<M> {
    /// Receive a single message of type `M` from the channel
    ///
    /// SAFETY: the `receiver` must be of type [`MessageReceiver<M>`], and the `message_bytes` must be a valid serialized message of type `M`
    pub(crate) unsafe fn receive_message_typed(
        receiver: MutUntyped,
        reader: &mut Reader,
        channel_kind: ChannelKind,
        remote_tick: Tick,
        message_id: Option<MessageId>,
        serialize_metadata: &ErasedSerializeFns,
        entity_map: &mut ReceiveEntityMap,
    ) -> Result<(), MessageError> {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>() };
        // we deserialize the message and send a MessageEvent
        let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
        let received_message = ReceivedMessage {
            data: message,
            remote_tick,
            channel_kind,
            message_id,
        };
        trace!(
            "Received message {:?} on channel {channel_kind:?}",
            core::any::type_name::<M>()
        );
        receiver.recv.push(received_message);
        Ok(())
    }

    pub(crate) unsafe fn clear_typed(receiver: MutUntyped) {
        // SAFETY: we know the type of the receiver is MessageReceiver<M>
        let mut receiver = unsafe { receiver.with_type::<Self>() };
        receiver.recv.clear();
    }
}

impl MessagePlugin {
    fn receive_message_bytes(
        bytes: Bytes,
        registry: &MessageRegistry,
        receiver_query: &mut Query<FilteredEntityMut>,
        entity: Entity,
        channel_kind: ChannelKind,
        tick: Tick,
        message_id: Option<MessageId>,
        message_manager: &mut MessageManager,
        commands: &ParallelCommands,
        remote_peer_id: PeerId,
    ) -> core::result::Result<(), MessageError> {
        trace!(
            "Received message (id:{message_id:?}) from peer {:?} on channel {channel_kind:?}. {entity:?}",
            remote_peer_id
        );
        let mut reader = Reader::from(bytes);
        // we receive the message NetId, and then deserialize the message
        let message_net_id = MessageNetId::from_bytes(&mut reader)?;
        let message_kind = registry
            .kind_map
            .kind(message_net_id)
            .ok_or(MessageError::UnrecognizedMessageId(message_net_id))?;
        let serialize_fns = registry
            .serialize_fns_map
            .get(message_kind)
            .ok_or(MessageError::UnrecognizedMessage(*message_kind))?;

        if let Some(recv_metadata) = registry.receive_metadata.get(message_kind) {
            let component_id = recv_metadata.component_id;
            let mut entity_mut = receiver_query.get_mut(entity).unwrap();
            let receiver = entity_mut
                .get_mut_by_id(component_id)
                .ok_or(MessageError::MissingComponent(component_id))?;
            // SAFETY: we know the receiver corresponds to the correct `MessageReceiver<M>` type
            unsafe {
                (recv_metadata.receive_message_fn)(
                    receiver,
                    &mut reader,
                    channel_kind,
                    tick,
                    message_id,
                    serialize_fns,
                    &mut message_manager.entity_mapper.remote_to_local,
                )
            }
        } else if let Some(trigger_fn) = registry.receive_trigger.get(message_kind) {
            // SAFETY: We assume the trigger handler function is correctly implemented
            // for the RemoteOn<M> type associated with this message_kind.
            unsafe {
                trigger_fn(
                    commands,
                    &mut reader,
                    channel_kind,
                    tick,
                    message_id,
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
                &LocalTimeline,
                Option<&mut HostClient>,
            ),
            With<Connected>,
        >,
        // List of ChannelReceivers<M> present on that entity
        receiver_query: Query<FilteredEntityMut>,
        registry: Res<MessageRegistry>,
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
                timeline,
                mut host_client,
            )| {
                // SAFETY: we know that this won't lead to violating the aliasing rule
                let mut receiver_query = unsafe { receiver_query.reborrow_unsafe() };
                // enable split borrows
                let transport = &mut *transport;
                // TODO: we can run this in parallel using rayon!
                if let Some(host_client) = host_client.as_mut() {
                    let tick = timeline.tick();
                    // for host-clients, we might have to deserialize messages that are in the Transports' senders
                    transport
                        .senders
                        .iter_mut()
                        .try_for_each(|(channel_kind, sender_metadata)| {
                            host_client.buffer.drain(..).try_for_each(
                                |(bytes, channel_type_id)| {
                                    trace!("Received local message bytes from server on host-client {entity:?} on channel {channel_kind:?}");
                                    // we fake the tick and message_id for host-client messages
                                    Self::receive_message_bytes(
                                        bytes,
                                        &registry,
                                        &mut receiver_query,
                                        entity,
                                        ChannelKind(channel_type_id),
                                        tick,
                                        None,
                                        &mut message_manager,
                                        &commands,
                                        remote_peer_id.0,
                                    )
                                },
                            )?;
                            Ok::<_, MessageError>(())
                        })
                        .inspect_err(|e| error!("Error receiving messages: {e:?}"))
                        .ok();
                } else {
                    transport
                        .receivers
                        .values_mut()
                        .try_for_each(|receiver_metadata| {
                            let channel_kind = receiver_metadata.channel_kind;
                            while let Some((tick, bytes, message_id)) =
                                receiver_metadata.receiver.read_message()
                            {
                                Self::receive_message_bytes(
                                    bytes,
                                    &registry,
                                    &mut receiver_query,
                                    entity,
                                    channel_kind,
                                    tick,
                                    message_id,
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
