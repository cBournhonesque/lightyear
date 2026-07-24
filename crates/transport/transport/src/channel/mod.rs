//! Channels add delivery and ordering policies on top of the packet transport.

pub use crate::channel::registry::ChannelKind;

pub mod builder;
pub(crate) mod fragments;
pub mod receive;
pub mod send;
mod send_reliable;

pub mod registry;
#[cfg(feature = "trace")]
pub mod stats;

/// A Channel is used to specify some properties of how the bytes are sent over the network.
///
/// The properties can be specified using the [`ChannelSettings`](crate::prelude::ChannelSettings).
pub trait Channel: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Channel for T {}
