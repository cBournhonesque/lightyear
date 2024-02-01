use std::any::TypeId;
use std::fmt::Debug;

use bevy::prelude::{App, World};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::inputs::native::input_buffer::InputMessage;
use crate::packet::message::Message;
use crate::prelude::MapEntities;
use crate::protocol::registry::TypeKind;
use crate::protocol::{BitSerializable, EventContext, Protocol};
#[cfg(feature = "leafwing")]
use crate::shared::events::InputMessageEvent;
use crate::shared::events::IterMessageEvent;
use crate::utils::named::Named;

// client writes an Enum containing all their message type
// each message must derive message

pub enum InputMessageKind {
    #[cfg(feature = "leafwing")]
    Leafwing,
    Native,
    None,
}

// that big enum will implement MessageProtocol via a proc macro
pub trait MessageProtocol:
    BitSerializable
    + Serialize
    + DeserializeOwned
    + Clone
    + for<'a> MapEntities<'a>
    + Debug
    + Send
    + Sync
    + From<InputMessage<<<Self as MessageProtocol>::Protocol as Protocol>::Input>>
    + TryInto<InputMessage<<<Self as MessageProtocol>::Protocol as Protocol>::Input>, Error = ()>
{
    type Protocol: Protocol;

    /// Get the name of the Message
    fn name(&self) -> &'static str;

    /// Returns the MessageKind of the Message
    fn kind(&self) -> MessageKind;

    /// Returns true if the message is an input message
    fn input_message_kind(&self) -> InputMessageKind;

    // TODO: combine these 2 into a single function that takes app?
    /// Add events to the app
    fn add_events<Ctx: EventContext>(app: &mut App);

    /// Takes messages that were written and writes MessageEvents
    fn push_message_events<E: IterMessageEvent<Self::Protocol, Ctx>, Ctx: EventContext>(
        world: &mut World,
        events: &mut E,
    );
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
