/*! # Lightyear Packet

Packet handling for the lightyear networking library.
This crate builds up on top of lightyear-io, to add packet fragmentation, channels, and reliability.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;


pub mod channel;

pub mod packet;




pub mod prelude {
    pub use crate::channel::builder::{ChannelMode, ChannelSettings, Transport};
    pub use crate::channel::plugin::TransportSet;
    pub use crate::channel::registry::ChannelRegistry;
    pub use crate::channel::Channel;

    pub use lightyear_macros::Channel;
}