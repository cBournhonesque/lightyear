/*! # Lightyear Packet

Packet handling for the lightyear networking library.
This crate builds up on top of lightyear-io, to add packet fragmentation, channels, and reliability.
*/
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod channel;

pub mod error;

#[cfg(feature = "client")]
mod client;
pub mod packet;
pub mod plugin;
#[cfg(feature = "server")]
mod server;

pub mod prelude {
    pub use crate::channel::Channel;
    pub use crate::channel::builder::{ChannelMode, ChannelSettings, ReliableSettings, Transport};
    pub use crate::channel::registry::AppChannelExt;
    pub use crate::channel::registry::ChannelRegistry;
    pub use crate::packet::priority_manager::{PriorityConfig, PriorityManager};
}
