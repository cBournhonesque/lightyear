/*!
The Protocol is used to define all the types that can be sent over the network

A protocol is composed of a few main parts:
- a [`MessageRegistry`](message::MessageRegistry) that contains the list of all the messages that can be sent over the network, along with how to serialize and deserialize them
- a [`ComponentRegistry`](component::ComponentRegistry) that contains the list of all the components that can be sent over the network, along with how to serialize and deserialize them.
You can also define additional behaviour for each component (such as how to run interpolation for them, etc.)
- a list of inputs that can be sent from client to server
- a [`ChannelRegistry`](channel::ChannelRegistry) that contains the list of channels that define how the data will be sent over the network (reliability, ordering, etc.)

*/

use anyhow::Context;

use bevy::prelude::{App, Resource};
use bevy::reflect::TypePath;
use bitcode::encoding::Fixed;
use bitcode::{Decode, Encode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::channel::builder::{Channel, ChannelSettings};
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::shared::replication::ReplicationSend;

/// Defines the various channels that can be used to send data over the network
pub(crate) mod channel;

/// Defines the various components that can be sent over the network
pub(crate) mod component;

/// Defines the various messages that can be sent over the network
pub(crate) mod message;

pub(crate) mod delta;
/// Provides a mapping from a type to a unique identifier that can be serialized
pub(crate) mod registry;
pub(crate) mod serialize;

/// Something that can be serialized bit by bit
pub trait BitSerializable {
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()>;

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized;
}

// TODO: allow for either decode/encode directly, or use serde if we add an attribute with_serde?
impl<T> BitSerializable for T
where
    T: Serialize + DeserializeOwned,
{
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
        writer.serialize(self)
    }

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
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
