/*! Channels are used to add reliability/ordering on top of the transport layer
*/

pub use crate::channel::registry::ChannelKind;

pub mod builder;
pub mod receivers;
pub mod senders;

pub mod registry;
#[cfg(feature = "trace")]
pub mod stats;

pub trait Channel: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Channel for T {}
