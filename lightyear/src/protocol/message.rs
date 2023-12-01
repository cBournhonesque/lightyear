use std::any::TypeId;

use bevy::prelude::{App, World};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::connection::events::IterMessageEvent;
use crate::inputs::input_buffer::InputMessage;
use crate::packet::message::Message;
use crate::prelude::MapEntities;
use crate::protocol::registry::TypeKind;
use crate::protocol::{BitSerializable, EventContext, Protocol};
use crate::utils::named::Named;

// client writes an Enum containing all their message type
// each message must derive message

// that big enum will implement MessageProtocol via a proc macro
pub trait MessageProtocol:
    BitSerializable
    + Serialize
    + DeserializeOwned
    + Clone
    + MapEntities
    + MessageBehaviour
    + Named
    + Send
    + Sync
    + From<InputMessage<<<Self as MessageProtocol>::Protocol as Protocol>::Input>>
    + TryInto<InputMessage<<<Self as MessageProtocol>::Protocol as Protocol>::Input>, Error = ()>
{
    type Protocol: Protocol;

    // TODO: combine these 2 into a single function that takes app?
    /// Add events to the app
    fn add_events<Ctx: EventContext>(app: &mut App);

    /// Takes messages that were written and writes MessageEvents
    fn push_message_events<E: IterMessageEvent<Self::Protocol, Ctx>, Ctx: EventContext>(
        world: &mut World,
        events: &mut E,
    );
}

/// Trait to delegate a method from the messageProtocol enum to the inner Message type
#[enum_delegate::register]
pub trait MessageBehaviour {
    fn kind(&self) -> MessageKind;
}

impl<M: Message> MessageBehaviour for M {
    fn kind(&self) -> MessageKind {
        MessageKind::of::<M>()
    }
}

/// MessageKind - internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct MessageKind(TypeId);

impl MessageKind {
    pub fn of<M: Message>() -> Self {
        Self(TypeId::of::<M>())
    }
}

impl TypeKind for MessageKind {}

impl From<TypeId> for MessageKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}
