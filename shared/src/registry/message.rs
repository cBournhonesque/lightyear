use std::any::TypeId;
use std::collections::HashMap;

use anyhow::bail;
use bitcode::{Decode, Encode};

use crate::packet::message::Message;
use crate::registry::NetId;

/// MessageKind - internal wrapper around the type of the channel
#[derive(Eq, Hash, Copy, Clone, PartialEq)]
pub struct MessageKind(TypeId);

impl MessageKind {
    pub fn of<M: Message>() -> Self {
        Self(TypeId::of::<M>())
    }
}
