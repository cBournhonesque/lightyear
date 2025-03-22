use crate::protocol::registry::TypeKind;
use core::any::TypeId;

pub(crate) mod client;

pub(crate) mod registry;

pub(crate) mod server;

pub(crate) mod resource;

pub(crate) mod trigger;

#[derive(thiserror::Error, Debug)]
pub enum MessageError {
    #[error("the message if of the wrong type")]
    IncorrectType,
    #[error("message is not registered in the protocol")]
    NotRegistered,
    #[error("missing serialization functions for message")]
    MissingSerializationFns,
    #[error(transparent)]
    Serialization(#[from] crate::serialize::SerializationError),
    #[error(transparent)]
    Packet(#[from] crate::packet::error::PacketError),
}

/// [`MessageKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq)]
pub struct MessageKind(TypeId);

impl MessageKind {
    pub fn of<M: 'static>() -> Self {
        Self(TypeId::of::<M>())
    }
}

impl TypeKind for MessageKind {}

impl From<TypeId> for MessageKind {
    fn from(type_id: TypeId) -> Self {
        Self(type_id)
    }
}
