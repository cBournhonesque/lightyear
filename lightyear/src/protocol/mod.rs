//! The Protocol is used to define all the types that can be sent over the network
//! # Protocol
//!
//! Protocol is the main struct that defines the various channels, inputs, messages and components that will be used in the game.
//! Inputs, Messages and Components are all data structures that can be serialized and sent over the network.
//! Channels are an abstraction over how the data will be sent over the network (reliability, ordering, etc.)

use anyhow::Context;
use std::fmt::Debug;

use bevy::prelude::{App, Resource};
use bitcode::encoding::Fixed;
use bitcode::{Decode, Encode};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::channel::builder::{Channel, ChannelSettings};
use crate::protocol::channel::ChannelRegistry;
use crate::protocol::component::{ComponentProtocol, ComponentProtocolKind};
use crate::protocol::message::MessageProtocol;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::writer::WriteBuffer;
use crate::shared::replication::ReplicationSend;

/// Defines the various channels that can be used to send data over the network
pub(crate) mod channel;

/// Defines the various components that can be sent over the network
pub(crate) mod component;

/// Defines the various messages that can be sent over the network
pub(crate) mod message;

/// Provides a mapping from a type to a unique identifier that can be serialized
pub(crate) mod registry;

// TODO: how to make components or messages or inputs optional? Just by having an implementation for () ?
/// The [`Protocol`] trait defines the various channels, inputs, messages and components that will be used to transmit information between
/// the client and server.
///
/// ## What is a Protocol?
///
/// A protocol will contain:
/// - a [`ChannelRegistry`]: a registry to list all the [`Channel`]s that will be used to send data over the network
/// - a [`MessageProtocol`]: an enum containing all the [`Message`]s that can be sent over the network
/// - a [`ComponentProtocol`]: an enum containing all the [`Component`]s that can be sent over the network for automatic world replication. Each [`Component`] must also be a [`Message`].
/// - optionally, one or multiple enums that represent a list of user actions that can be sent over the network. It is recommended to use the "leafwing" feature and provide
///   a [`crate::inputs::leafwing::LeafwingUserAction`] enum
///
/// It is required to provide a [`MessageProtocol`] and a [`ComponentProtocol`]; providing an Input protocol is optional.
///
/// # Examples
///
/// Here is an example of a protocol that defines a single channel, a single message and a single component:
/// ```rust,ignore
///# use serde::{Serialize, Deserialize};
///# use bevy::prelude::Component;
///# use lightyear::prelude::*;
///
///#[derive(Message, Serialize, Deserialize, Clone, PartialEq)]
///pub struct Message1(pub String);
///
///#[message_protocol(protocol = "MyProtocol")]
///pub enum MyMessageProtocol {
///    Message1(Message1),
///}
///
///#[derive(Component, Message, Serialize, Deserialize, Clone, PartialEq)]
///pub struct Component1;
///
///#[component_protocol(protocol = "MyProtocol")]
///pub enum MyComponentsProtocol {
///    Component1(Component1),
///}
///
///protocolize! {
///    Self = MyProtocol,
///    Message = MyMessageProtocol,
///    Component = MyComponentsProtocol,
///}
///
///# fn main() {}
/// ```
///
/// [`Message`]: crate::prelude::Message
/// [`Component`]: bevy::prelude::Component
pub trait Protocol: Send + Sync + Clone + Debug + 'static {
    type Input: crate::inputs::native::UserAction;
    #[cfg(feature = "leafwing")]
    type LeafwingInput1: crate::inputs::leafwing::LeafwingUserAction;
    #[cfg(feature = "leafwing")]
    type LeafwingInput2: crate::inputs::leafwing::LeafwingUserAction;

    type Message: MessageProtocol<Protocol = Self>;
    type Components: ComponentProtocol<Protocol = Self>;
    type ComponentKinds: ComponentProtocolKind<Protocol = Self>;

    fn add_channel<C: Channel>(&mut self, settings: ChannelSettings) -> &mut Self;
    fn channel_registry(&self) -> &ChannelRegistry;
    fn add_per_component_replication_send_systems<R: ReplicationSend<Self>>(app: &mut App);
}

// TODO: give an option to change names of types
/// This macro is used to build the [`Protocol`] struct.
/// For convenience, it will re-export some types that need to have the Protocol as a generic parameter, so that you
/// don't have to type `<MyProtocol>` everywhere.
/// Notably:
/// - `Replicate` is a type alias for [`Replicate<Protocol>`](crate::shared::replication::components::Replicate)
/// - `ClientConnectionManager` is a type alias for [`ConnectionManager<Protocol>`](crate::client::connection::ConnectionManager)
/// - `ServerConnectionManager` is a type alias for [`ConnectionManager<Protocol>`](crate::server::connection::ConnectionManager)
#[macro_export]
macro_rules! protocolize {
        (
        Self = $protocol:ident,
        Message = $message:ty,
        Component = $components:ty,
        Input = $input:ty,
        Crate = $shared_crate_name:ident,
    ) => {
        use $shared_crate_name::_reexport::paste;
        paste! {
        mod [<$protocol:lower _module>] {
            use super::*;
            use bevy::prelude::*;
            use $shared_crate_name::prelude::*;
            use $shared_crate_name::_reexport::*;

            #[derive(Debug, Clone, Resource, PartialEq)]
            pub struct $protocol {
                channel_registry: ChannelRegistry,
            }

            impl Protocol for $protocol {
                type Input = $input;
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

                fn add_per_component_replication_send_systems<R: ReplicationSend<Self>>(app: &mut App) {
                    Self::Components::add_per_component_replication_send_systems::<R>(app);
                }
            }

            impl Default for $protocol {
                fn default() -> Self {
                    let mut protocol = Self {
                        channel_registry: ChannelRegistry::default(),
                    };
                    protocol.add_channel::<EntityActionsChannel>(ChannelSettings {
                        mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
                        direction: ChannelDirection::Bidirectional,
                        // we want to send the entity actions as soon as possible
                        priority: 10.0,
                    });
                    protocol.add_channel::<EntityUpdatesChannel>(ChannelSettings {
                        mode: ChannelMode::UnorderedUnreliableWithAcks,
                        direction: ChannelDirection::Bidirectional,
                        priority: 1.0,
                    });
                    protocol.add_channel::<PingChannel>(ChannelSettings {
                        mode: ChannelMode::SequencedUnreliable,
                        direction: ChannelDirection::Bidirectional,
                        // we always want to include the ping in the packet
                        priority: 1000.0,
                    });
                    protocol.add_channel::<InputChannel>(ChannelSettings {
                        // we want to use unordered unreliable because the server has a buffer to re-order the inputs anyway
                        // Also multiple 'types' of inputs share the same channel
                        // TODO: maybe we should have a different input channel per input, and use sequenced?
                        //  because our messages contain the last 10 ticks of input anyway, so we don't need to read older ones.
                        mode: ChannelMode::UnorderedUnreliable,
                        direction: ChannelDirection::ClientToServer,
                        priority: 3.0,
                    });
                    protocol.add_channel::<DefaultUnorderedUnreliableChannel>(ChannelSettings {
                        mode: ChannelMode::UnorderedUnreliable,
                        direction: ChannelDirection::Bidirectional,
                        priority: 1.0,
                    });
                    protocol.add_channel::<TickBufferChannel>(ChannelSettings {
                        mode: ChannelMode::TickBuffered,
                        direction: ChannelDirection::ClientToServer,
                        priority: 1.0,
                    });
                    protocol
                }
            }
        }
        pub use [<$protocol:lower _module>]::$protocol;
        pub type Replicate = $shared_crate_name::shared::replication::components::Replicate<$protocol>;
        pub type ClientConnectionManager = $shared_crate_name::client::connection::ConnectionManager<$protocol>;
        pub type ServerConnectionManager = $shared_crate_name::server::connection::ConnectionManager<$protocol>;
        }
    };

    (
        Self = $protocol:ident,
        Message = $message:ty,
        Component = $components:ty,
        Input = $input:ty,
        LeafwingInput1 = $leafwing_input_1:ty,
        LeafwingInput2 = $leafwing_input_2:ty,
        Crate = $shared_crate_name:ident,
    ) => {
        use $shared_crate_name::_reexport::paste;
        paste! {
        mod [<$protocol:lower _module>] {
            use super::*;
            use bevy::prelude::*;
            use $shared_crate_name::prelude::*;
            use $shared_crate_name::_reexport::*;
            use $shared_crate_name::inputs::leafwing::{NoAction1, NoAction2};

            #[derive(Debug, Clone, Resource, PartialEq)]
            pub struct $protocol {
                channel_registry: ChannelRegistry,
            }

            impl Protocol for $protocol {
                type Input = $input;
                type LeafwingInput1 = $leafwing_input_1;
                type LeafwingInput2 = $leafwing_input_2;
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

                fn add_per_component_replication_send_systems<R: ReplicationSend<Self>>(app: &mut App) {
                    Self::Components::add_per_component_replication_send_systems::<R>(app);
                }
            }

            impl Default for $protocol {
                fn default() -> Self {
                    let mut protocol = Self {
                        channel_registry: ChannelRegistry::default(),
                    };
                    protocol.add_channel::<EntityActionsChannel>(ChannelSettings {
                        mode: ChannelMode::UnorderedReliable(ReliableSettings::default()),
                        direction: ChannelDirection::Bidirectional,
                        // we want to send the entity actions as soon as possible
                        priority: 10.0,
                    });
                    protocol.add_channel::<EntityUpdatesChannel>(ChannelSettings {
                        mode: ChannelMode::UnorderedUnreliableWithAcks,
                        direction: ChannelDirection::Bidirectional,
                        priority: 1.0,
                    });
                    protocol.add_channel::<PingChannel>(ChannelSettings {
                        mode: ChannelMode::SequencedUnreliable,
                        direction: ChannelDirection::Bidirectional,
                        // we always want to include the ping in the packet
                        priority: 1000.0,
                    });
                    protocol.add_channel::<InputChannel>(ChannelSettings {
                        mode: ChannelMode::UnorderedUnreliable,
                        direction: ChannelDirection::ClientToServer,
                        priority: 3.0,
                    });
                    protocol.add_channel::<DefaultUnorderedUnreliableChannel>(ChannelSettings {
                        mode: ChannelMode::UnorderedUnreliable,
                        direction: ChannelDirection::Bidirectional,
                        priority: 1.0,
                    });
                    protocol.add_channel::<TickBufferChannel>(ChannelSettings {
                        mode: ChannelMode::TickBuffered,
                        direction: ChannelDirection::ClientToServer,
                        priority: 1.0,
                    });
                    protocol
                }
            }
        }
        pub use [<$protocol:lower _module>]::$protocol;
        pub type Replicate = $shared_crate_name::shared::replication::components::Replicate<$protocol>;
        pub type ClientConnectionManager = $shared_crate_name::client::connection::ConnectionManager<$protocol>;
        pub type ServerConnectionManager = $shared_crate_name::server::connection::ConnectionManager<$protocol>;
        }
    };

    (
        Self = $protocol:ident,
        Message = $message:ty,
        Component = $components:ty,
        Input = $input:ty,
        LeafwingInput1 = $leafwing_input_1:ty,
        LeafwingInput2 = $leafwing_input_2:ty,
    ) => {
        lightyear::protocolize!{
            Self = $protocol,
            Message = $message,
            Component = $components,
            Input = $input,
            LeafwingInput1 = $leafwing_input_1,
            LeafwingInput2 = $leafwing_input_2,
            Crate = lightyear,
        }
    };

    (
        Self = $protocol:ident,
        Message = $message:ty,
        Component = $components:ty,
        LeafwingInput1 = $leafwing_input_1:ty,
    ) => {
        lightyear::protocolize!{
            Self = $protocol,
            Message = $message,
            Component = $components,
            Input = (),
            LeafwingInput1 = $leafwing_input_1,
            LeafwingInput2 = NoAction2,
            Crate = lightyear,
        }
    };

    (
        Self = $protocol:ident,
        Message = $message:ty,
        Component = $components:ty,
        LeafwingInput1 = $leafwing_input_1:ty,
        LeafwingInput2 = $leafwing_input_2:ty,
    ) => {
        lightyear::protocolize!{
            Self = $protocol,
            Message = $message,
            Component = $components,
            Input = (),
            LeafwingInput1 = $leafwing_input_1,
            LeafwingInput2 = $leafwing_input_2,
            Crate = lightyear,
        }
    };

    (
        Self = $protocol:ident,
        Message = $message:ty,
        Component = $components:ty,
        Crate = $shared_crate_name:ident,
    ) => {
        $shared_crate_name::protocolize!{
            Self = $protocol,
            Message = $message,
            Component = $components,
            Input = (),
            Crate = $shared_crate_name,
        }
    };

    (
        Self = $protocol:ident,
        Message = $message:ty,
        Component = $components:ty,
        Input = $input:ty,
    ) => {
        lightyear::protocolize!{
            Self = $protocol,
            Message = $message,
            Component = $components,
            Input = $input,
            Crate = lightyear,
        }
    };

    (
        Self = $protocol:ident,
        Message = $message:ty,
        Component = $components:ty,
    ) => {
        lightyear::protocolize!{
            Self = $protocol,
            Message = $message,
            Component = $components,
            Crate = lightyear,
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

// TODO: allow for either decode/encode directly, or use serde if we add an attribute with_serde?
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

// impl<T> BitSerializable for T
// where
//     T: Encode + Decode + Clone,
// {
//     fn encode(&self, writer: &mut impl WriteBuffer) -> anyhow::Result<()> {
//         self.encode(Fixed, writer).context("could not encode")
//     }
//
//     fn decode(reader: &mut impl ReadBuffer) -> anyhow::Result<Self>
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
