//! The Protocol is used to define all the types that can be sent over the network
//! # Protocol
//!
//! Protocol is the main struct that defines the various channels, inputs, messages and components that will be used in the game.
//! Inputs, Messages and Components are all data structures that can be serialized and sent over the network.
//! Channels are an abstraction over how the data will be sent over the network (reliability, ordering, etc.)

use anyhow::Context;

use crate::_internal::{ReadWordBuffer, WriteWordBuffer};
use bevy::prelude::{App, Resource};
use bevy::reflect::TypePath;
use bitcode::encoding::Fixed;
use bitcode::{Decode, Encode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::channel::builder::{Channel, ChannelSettings};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::message::MessageRegistry;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::shared::replication::ReplicationSend;

/// Defines the various channels that can be used to send data over the network
pub(crate) mod channel;

/// Defines the various components that can be sent over the network
pub(crate) mod component;

/// Defines the various messages that can be sent over the network
pub(crate) mod message;

mod prediction;
/// Provides a mapping from a type to a unique identifier that can be serialized
pub(crate) mod registry;
mod serialize;

/// Something that can be serialized bit by bit
pub trait BitSerializable: Clone {
    fn encode(&self, writer: &mut WriteWordBuffer) -> anyhow::Result<()>;

    fn decode(reader: &mut ReadWordBuffer) -> anyhow::Result<Self>
    where
        Self: Sized;
}

// TODO: allow for either decode/encode directly, or use serde if we add an attribute with_serde?
impl<T> BitSerializable for T
where
    T: Serialize + DeserializeOwned + Clone,
{
    fn encode(&self, writer: &mut WriteWordBuffer) -> anyhow::Result<()> {
        writer.serialize(self)
    }

    fn decode(reader: &mut ReadWordBuffer) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        reader.deserialize::<Self>()
    }
}

// impl<T> BitSerializable for T
// where
//     T: Encode + Decode + Clone,
// {
//     fn encode(&self, writer: &mut WriteWordBuffer) -> anyhow::Result<()> {
//         self.encode(Fixed, writer).context("could not encode")
//     }
//
//     fn decode(reader: &mut ReadWordBuffer) -> anyhow::Result<Self>
//     where
//         Self: Sized,
//     {
//         <Self as Decode>::decode(Fixed, reader).context("could not decode")
//     }
// }

/// Data that can be used in an Event
/// Same as `Event`, but we implement it automatically for all compatible types
pub trait EventContext: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> EventContext for T {}
