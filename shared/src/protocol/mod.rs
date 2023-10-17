use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::{ComponentProtocol, ComponentProtocolKind};
use crate::protocol::message::MessageProtocol;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::{Channel, ChannelSettings};

pub(crate) mod channel;
pub mod component;
pub(crate) mod message;
pub(crate) mod registry;

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
    ($protocol:ident, $message:ty, $components:ty) => {
        use lightyear_shared::paste;
        paste! {
        mod [<$protocol _module>] {
            use super::*;
            use lightyear_shared::{
                Channel, ChannelRegistry, ChannelSettings, ComponentProtocol, ComponentProtocolKind, Entity,
                MessageProtocol, Protocol,
            };

            #[derive(Default, Clone)]
            pub struct $protocol {
                channel_registry: ChannelRegistry,
            }

            impl Protocol for $protocol {
                type Message = $message;
                type Components = $components;
                type ComponentKinds = [<$components Kind>];

                fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) -> &mut Self {
                    self.channel_registry.add::<C>(settings);
                    self
                }

                fn channel_registry(&self) -> &ChannelRegistry {
                    &self.channel_registry
                }
            }
        }
        pub use [<$protocol _module>]::$protocol;
        }
    };
}

pub trait Protocol: Send + Sync + Clone + 'static {
    type Message: MessageProtocol + Send + Sync;
    type Components: ComponentProtocol + Send + Sync;
    type ComponentKinds: ComponentProtocolKind + Send + Sync;
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

impl<T> BitSerializable for T
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
