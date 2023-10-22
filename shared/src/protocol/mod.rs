use std::fmt::Debug;

use bevy_app::App;
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
    ($protocol:ident, $message:ty, $components:ty) => {
        use lightyear_shared::paste;
        paste! {
        mod [<$protocol _module>] {
            use super::*;
            use lightyear_shared::{
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
