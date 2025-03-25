/*! Channels are used to add reliability/ordering on top of the transport layer
*/

use crate::channel::registry::ChannelKind;

pub mod builder;
pub(crate) mod receivers;
pub(crate) mod senders;

#[cfg(feature = "trace")]
pub mod stats;
pub(crate) mod plugin;
pub(crate) mod registry;



pub trait Channel: Send + Sync + 'static {
    fn name() -> &'static str;

    fn kind() -> ChannelKind
    where
        Self: Sized,
    {
        ChannelKind::of::<Self>()
    }
}