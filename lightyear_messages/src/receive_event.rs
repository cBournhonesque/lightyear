use crate::Message;
use crate::registry::MessageError;
use bevy_ecs::entity::Entity;
use bevy_ecs::event::EntityEvent;
use bevy_ecs::{event::Event, system::ParallelCommands};
use bevy_utils::prelude::DebugName;
use lightyear_core::tick::Tick;
use lightyear_serde::entity_map::ReceiveEntityMap;
use lightyear_serde::reader::Reader;
use lightyear_transport::channel::ChannelKind;

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

/// Receive a `TriggerEvent<M>`, deserialize it, and emit a `RemoteEvent<M>` event.
///
/// SAFETY: The `reader` must contain a valid serialized `TriggerEvent<M>`.
/// The `serialize_metadata` must correspond to the `TriggerEvent<M>` type.
pub(crate) unsafe fn receive_event_typed<M: Message + Event>(
    commands: &ParallelCommands,
    reader: &mut Reader,
    _channel_kind: ChannelKind,
    _remote_tick: Tick,
    _message_id: Option<MessageId>,
    serialize_metadata: &ErasedSerializeFns,
    entity_map: &mut ReceiveEntityMap,
    from: PeerId,
) -> Result<(), MessageError> {
    // we deserialize the message and send a MessageEvent
    let message = unsafe { serialize_metadata.deserialize::<_, M, M>(reader, entity_map)? };
    trace!(
        "Received trigger message: {:?} from: {from:?}",
        DebugName::type_name::<M>()
    );
    let trigger = RemoteEvent {
        trigger: message,
        from,
    };
    commands.command_scope(|mut c| {
        c.trigger(trigger);
    });
    // commands.trigger(trigger);
    Ok(())
}
