/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

mod registry;


pub(crate) mod archetypes;
pub mod components;

pub(crate) mod authority;
pub mod error;
pub(crate) mod hierarchy;
pub(crate) mod plugin;
pub(crate) mod receive;

pub(crate) mod resources;
pub(crate) mod send;

pub(crate) mod systems;
pub(crate) mod message;
mod buffer;
pub(crate) mod delta;

pub mod prelude {
    pub use crate::buffer::Replicate;
    pub use crate::components::*;
    pub use crate::plugin::{ReplicationManager, ReplicationSet};
    pub use crate::receive::{ReplicationReceivePlugin, ReplicationReceiver};
    pub use crate::send::{ReplicationBufferSet, ReplicationSendPlugin, ReplicationSender};
}
