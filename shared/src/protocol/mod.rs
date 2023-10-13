use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::protocol::channel::ChannelProtocol;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::{Channel, ChannelRegistry, ChannelSettings};

pub(crate) mod channel;
pub(crate) mod component;
pub(crate) mod message;

// create this struct using a macro, where we provide the message and component enums
// protocolize!(MyMessage, MyComponents)
// which generates

// pub struct MyProtocol {
//   channel_registry
// }

// impl Protocol for MyProtocol {
//   type Message: MyMessage;
// }

// create a struct MyProtocol that satisfies trait Protocol
// - we need the struct to access the channel registry
// - we need the trait Protocol to access the associated types

// TODO: give an option to change names of types
#[macro_export]
macro_rules! protocolize {
    ($message:ty, $components:ty) => {
        mod my_protocol {
            use super::*;
            use lightyear_shared::{
                paste, Channel, ChannelRegistry, ChannelSettings, ComponentProtocol, Entity,
                MessageProtocol, Protocol,
            };
            use serde::{Deserialize, Serialize};

            #[derive(Default)]
            pub struct MyProtocol {
                channel_registry: ChannelRegistry,
            }

            #[derive(Serialize, Deserialize, Clone)]
            pub enum GeneratedComponentsProtocol {
                EntitySpawned(Entity),
                EntityDespawned(Entity),
                ComponentInserted(Entity, $components),
                ComponentRemoved(Entity, paste! { [<$components Kind>] }),
                // ComponentRemoved(Entity, Discriminant<$components>),
                EntityUpdate(Entity, Vec<$components>),
            }

            impl ComponentProtocol for GeneratedComponentsProtocol {}

            impl Protocol for MyProtocol {
                type Message = $message;
                type Components = GeneratedComponentsProtocol;

                fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) -> &mut Self {
                    self.channel_registry.add::<C>(settings);
                    self
                }

                fn channel_registry(&self) -> &ChannelRegistry {
                    &self.channel_registry
                }
            }
        }
        pub use my_protocol::MyProtocol;
    };
}

pub trait Protocol {
    type Message: MessageProtocol;
    type Components: ComponentProtocol;
    fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) -> &mut Self;
    fn channel_registry(&self) -> &ChannelRegistry;
}

/// Something that can be serialized bit by bit
pub trait BitSerializable: Clone {
    fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()>;

    fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
    where
        Self: Sized;
}

// TODO: if this all we need from message protocol, then use this!
//  then we can have messageProtocols require to implement a last marker trait called IsMessageProtocol

impl<'a, T> BitSerializable for T
where
    T: Serialize + DeserializeOwned + Clone,
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
