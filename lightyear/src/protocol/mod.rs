/*!
The Protocol is used to define all the types that can be sent over the network

A protocol is composed of a few main parts:
- a [`MessageRegistry`](message::MessageRegistry) that contains the list of all the messages that can be sent over the network, along with how to serialize and deserialize them
- a [`ComponentRegistry`](component::ComponentRegistry) that contains the list of all the components that can be sent over the network, along with how to serialize and deserialize them.
  You can also define additional behaviour for each component (such as how to run interpolation for them, etc.)
- a list of inputs that can be sent from client to server
- a [`ChannelRegistry`](channel::ChannelRegistry) that contains the list of channels that define how the data will be sent over the network (reliability, ordering, etc.)

*/

/// Defines the various channels that can be used to send data over the network
pub(crate) mod channel;

/// Defines the various components that can be sent over the network
pub(crate) mod component;

/// Defines the various messages that can be sent over the network
pub(crate) mod message;

/// Provides a mapping from a type to a unique identifier that can be serialized
pub(crate) mod registry;
pub(crate) mod serialize;

pub use serialize::SerializeFns;

/// Data that can be used in an Event
/// Same as `Event`, but we implement it automatically for all compatible types
pub trait EventContext: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> EventContext for T {}
