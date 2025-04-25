use crate::registry::MessageError;
use crate::Message;
use bevy::prelude::ParallelCommands;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_transport::channel::ChannelKind;


#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use crate::trigger::TriggerMessage;
use bevy::prelude::Event;
use lightyear_core::id::PeerId;
use lightyear_serde::registry::ErasedSerializeFns;
use lightyear_transport::packet::message::MessageId;

/// Bevy Event emitted when a `TriggerMessage<M>` is received and processed.
/// Contains the original trigger `M` and the `PeerId` of the sender.
#[derive(Event)]
pub struct RemoteTrigger<M: Message> {
    pub trigger: M,
    pub from: PeerId,
}


pub(crate) type ReceiveTriggerFn = unsafe fn(
    commands: &ParallelCommands,
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
    commands: &ParallelCommands,
    reader: &mut Reader,
    channel_kind: ChannelKind, // Keep args consistent, though not all might be used
    remote_tick: Tick,
    message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId, // Add sender PeerId
) -> Result<(), MessageError> {
    // we deserialize the message and send a MessageEvent
    let message = unsafe { serialize_metadata.deserialize::<_, TriggerMessage<M>, M>(reader, entity_map)? };
    let trigger = RemoteTrigger {
        trigger: message.trigger,
        from,
    };
    commands.command_scope(|mut c| {
        c.trigger_targets(trigger, message.target_entities);
    });
    // commands.trigger_targets(trigger, message.target_entities);
    Ok(())
}