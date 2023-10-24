use std::fmt::Debug;

use bevy::prelude::App;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::{ComponentProtocol, ComponentProtocolKind};
use crate::protocol::message::MessageProtocol;
use crate::replication::ReplicationSend;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::{Channel, ChannelSettings};

pub(crate) mod channel;
pub mod component;
pub(crate) mod message;
pub(crate) mod registry;

pub trait Protocol: Send + Sync + Clone + 'static {
    type Message: MessageProtocol + Send + Sync;
    type Components: ComponentProtocol<Protocol = Self> + Send + Sync;
    type ComponentKinds: ComponentProtocolKind<Protocol = Self> + Send + Sync;
    fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) -> &mut Self;
    fn channel_registry(&self) -> &ChannelRegistry;
    fn add_replication_send_systems<R: ReplicationSend<Self>>(app: &mut App);
}

// TODO: give an option to change names of types
#[macro_export]
macro_rules! protocolize {

    ($protocol:ident, $message:ty, $components:ty, $shared_crate_name:ident) => {
        use $shared_crate_name::paste;
        paste! {
        mod [<$protocol _module>] {
            use super::*;
            use $shared_crate_name::{
                App, Channel, ChannelRegistry, ChannelSettings, ComponentProtocol, ComponentProtocolKind,
                DefaultReliableChannel, Entity, MessageProtocol, Protocol, ReplicationSend,
                ReliableSettings, ChannelDirection, ChannelMode,
            };

            #[derive(Clone)]
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

                fn add_replication_send_systems<R: ReplicationSend<Self>>(app: &mut App) {
                    Self::Components::add_replication_send_systems::<R>(app);
                }
            }

            impl Default for $protocol {
                fn default() -> Self {
                    let mut protocol = Self {
                        channel_registry: ChannelRegistry::default(),
                    };
                    protocol.add_channel::<DefaultReliableChannel>(ChannelSettings {
                        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
                        direction: ChannelDirection::Bidirectional,
                    });
                    protocol
                }
            }
        }
        pub use [<$protocol _module>]::$protocol;
        }
    };
    ($protocol:ident, $message:ty, $components:ty) => {
        protocolize!($protocol, $message, $components, lightyear_shared);
    };
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

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::{ChannelDirection, ChannelMode, Message, ReliableSettings};
    use bevy::prelude::Component;
    use lightyear_derive::{
        component_protocol_internal, message_protocol_internal, ChannelInternal, MessageInternal,
    };

    // Messages
    #[derive(MessageInternal, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Message1(pub String);

    #[derive(MessageInternal, Serialize, Deserialize, Debug, PartialEq, Clone)]
    pub struct Message2(pub u32);

    #[derive(Debug, PartialEq)]
    #[message_protocol_internal]
    pub enum MyMessageProtocol {
        Message1(Message1),
        Message2(Message2),
    }

    #[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct Component1;

    #[derive(Debug, PartialEq)]
    #[component_protocol_internal(protocol = MyProtocol)]
    pub enum MyComponentsProtocol {
        Component1(Component1),
    }

    protocolize!(MyProtocol, MyMessageProtocol, MyComponentsProtocol, crate);

    // Channels
    #[derive(ChannelInternal)]
    pub struct Channel1;

    #[derive(ChannelInternal)]
    pub struct Channel2;

    pub fn test_protocol() -> MyProtocol {
        let mut p = MyProtocol::default();
        p.add_channel::<Channel1>(ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            direction: ChannelDirection::Bidirectional,
        });
        p.add_channel::<Channel2>(ChannelSettings {
            mode: ChannelMode::UnorderedUnreliable,
            direction: ChannelDirection::Bidirectional,
        });
        p
    }
}
