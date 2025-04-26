/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

pub mod registry;


pub(crate) mod archetypes;
pub mod components;

pub(crate) mod authority;
pub mod error;
pub(crate) mod hierarchy;
pub(crate) mod plugin;
pub mod receive;

pub(crate) mod send;

pub mod message;
pub(crate) mod buffer;
pub(crate) mod delta;

pub mod visibility;
pub mod control;

pub mod prelude {
    pub use crate::authority::HasAuthority;
    pub use crate::buffer::Replicate;
    pub use crate::components::*;
    pub use crate::hierarchy::{ChildOfSync, DisableReplicateHierarchy, HierarchySendPlugin, RelationshipReceivePlugin, RelationshipSendPlugin, RelationshipSync, ReplicateLike, ReplicateLikeChildren};
    pub use crate::message::*;
    pub use crate::plugin::ReplicationSet;
    pub use crate::receive::{ReplicationReceivePlugin, ReplicationReceiver};
    pub use crate::registry::registry::{AppComponentExt, ComponentRegistration};
    pub use crate::send::{ReplicationBufferSet, ReplicationSendPlugin, ReplicationSender, SendUpdatesMode};
    pub use crate::visibility::immediate::{NetworkVisibility, NetworkVisibilityPlugin};
    pub use crate::visibility::room::{Room, RoomPlugin};
}

