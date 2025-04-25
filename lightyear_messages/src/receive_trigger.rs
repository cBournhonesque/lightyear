use crate::plugin::MessagePlugin;
use crate::registry::{MessageError, MessageRegistry};
use crate::MessageManager;
use crate::{Message, MessageNetId};
use bevy::ecs::change_detection::MutUntyped;
use bevy::ecs::world::FilteredEntityMut;
use bevy::prelude::{Commands, Component, Entity, Query, Res};
use bytes::Bytes;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_serde::ToBytes;
use lightyear_transport::channel::receivers::ChannelReceive;
use lightyear_transport::channel::ChannelKind;
use lightyear_transport::prelude::Transport;
use tracing::{error, info, trace};

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use alloc::sync::Arc;
use bevy::prelude::Event;
use lightyear_core::id::PeerId;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_transport::packet::message::MessageId;
use crate::trigger::TriggerMessage;

/// Bevy Event emitted when a `TriggerMessage<M>` is received and processed.
/// Contains the original trigger `M` and the `PeerId` of the sender.
#[derive(Event)]
pub struct RemoteTrigger<M: Message> {
    pub trigger: M,
    pub from: PeerId,
}


#[derive(Debug)]
pub struct ReceivedMessage<M> {
    pub data: M,
    /// Tick on the remote peer when the message was sent,
    pub remote_tick: Tick,
    /// Channel that was used to send the message
    pub channel_kind: ChannelKind,
    /// MessageId of the message
    pub message_id: Option<MessageId>,
}


pub(crate) type ReceiveTriggerFn = unsafe fn(
    commands: &mut Commands,
    reader: &mut Reader,
    channel_kind: ChannelKind,
    remote_tick: Tick,
    message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId, // Add sender PeerId
) -> Result<(), MessageError>;



/// Receive a `TriggerMessage<M>`, deserialize it, and emit a `RemoteTrigger<M>` event.
///
/// SAFETY: The `reader` must contain a valid serialized `TriggerMessage<M>`.
/// The `serialize_metadata` must correspond to the `TriggerMessage<M>` type.
pub(crate) unsafe fn receive_trigger_typed<M: Message + Event>(
    commands: &mut Commands,
    reader: &mut Reader,
    channel_kind: ChannelKind, // Keep args consistent, though not all might be used
    remote_tick: Tick,
    message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId, // Add sender PeerId
) -> Result<(), MessageError> {
    // we deserialize the message and send a MessageEvent
    let message = unsafe { serialize_metadata.deserialize::<_, TriggerMessage<M>>(entity_map, reader)? };
    let trigger = RemoteTrigger {
        trigger: message.trigger,
        from,
    };
    commands.trigger_targets(trigger, message.target_entities);
    Ok(())
}

impl MessagePlugin {
    /// Receive bytes from each channel of the Transport
    /// Deserialize the bytes into Messages.
    /// - If the message is a `RemoteTrigger<M>`, emit a `TriggerEvent<M>` via `Commands`.
    /// - Otherwise, buffer the message in the `MessageReceiver<M>` component.
    pub fn recv(
        // NOTE: we only need the mut bound on MessageManager because EntityMapper requires mut
        mut transport_query: Query<(Entity, &mut MessageManager, &mut Transport)>,
        registry: Res<MessageRegistry>,
        mut commands: Commands,
    ) {
        transport_query.par_iter_mut().for_each(|(entity, mut message_manager, mut transport)| {
            // enable split borrows
            let transport = &mut *transport;
            // TODO: we can run this in parallel using rayon!
            transport.receivers.values_mut().try_for_each(|receiver_metadata| {
                let channel_kind = receiver_metadata.channel_kind;
                // TODO: ChannelReceive::read_message needs to return PeerId! Using placeholder for now.
                let placeholder_peer_id = PeerId::Entity;
                while let Some((tick, bytes, message_id)) = receiver_metadata.receiver.read_message() {
                    trace!("Received message {message_id:?} from placeholder {:?} on channel {channel_kind:?}", placeholder_peer_id);
                    let mut reader = Reader::from(bytes);
                    // we receive the message NetId, and then deserialize the message
                    let message_net_id = MessageNetId::from_bytes(&mut reader)?;
                    let message_kind = registry.kind_map.kind(message_net_id).ok_or(MessageError::UnrecognizedMessageId(message_net_id))?;
                    let recv_metadata = registry.receive_metadata.get(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;
                    let component_id = recv_metadata.component_id;
                    let mut entity_mut = receiver_query.get_mut(entity).unwrap();
                    let receiver = entity_mut
                        .get_mut_by_id(component_id)
                        .ok_or(MessageError::MissingComponent(component_id))?;

                    let serialize_fns = registry.serialize_fns_map.get(message_kind).ok_or(MessageError::UnrecognizedMessage(*message_kind))?;

                    // Check if there's a specific trigger handler for this message kind
                    if let Some(trigger_fn) = recv_metadata.trigger_fn {
                        // SAFETY: We assume the trigger handler function is correctly implemented
                        // for the RemoteTrigger<M> type associated with this message_kind.
                        // It requires the inner M to be Clone + 'static.
                        unsafe { trigger_fn(
                            &mut commands, // Pass commands for triggering
                            &mut reader,
                            channel_kind,
                            tick,
                            message_id,
                            serialize_fns,
                            &mut message_manager.entity_mapper.remote_to_local,
                            placeholder_peer_id, // Pass the placeholder PeerId
                        )?; }
                    } else {
                        // Otherwise, use the standard receive function to buffer in MessageReceiver<M>
                        // SAFETY: we know the receiver corresponds to the correct `MessageReceiver<M>` type
                        unsafe { (recv_metadata.receive_message_fn)(
                            receiver, // This is the MutUntyped receiver component
                            &mut reader,
                            channel_kind,
                            tick,
                            message_id,
                            serialize_fns,
                            &mut message_manager.entity_mapper.remote_to_local
                        )?; }
                    }
                }
                Ok::<_, MessageError>(())
            }).inspect_err(|e| error!("Error receiving messages: {e:?}")).ok();
        })
    }

}