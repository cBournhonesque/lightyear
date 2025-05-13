//! # Lightyear Replication
//!
//! This crate handles the logic for replicating entities and components
//! from the server to clients.
//!
//! It includes systems for:
//! - Tracking changes to components.
//! - Serializing and sending replication messages.
//! - Receiving and applying replication updates on clients.
//! - Managing entity authority and visibility (e.g., interest management, rooms).
//! - Handling component registration for replication.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

/// Handles the registration of components for replication.
pub mod registry;

pub(crate) mod archetypes;
/// Defines components related to replication, such as `Replicate` and `ParentSync`.
pub mod components;

pub(crate) mod authority;
/// Defines error types that can occur during replication.
pub mod error;
pub(crate) mod hierarchy;
pub(crate) mod plugin;
/// Handles receiving and applying replication updates on the client.
pub mod receive;

pub(crate) mod send;

pub(crate) mod buffer;
pub(crate) mod delta;
/// Defines the structure of messages used for replication.
pub mod message;

/// Manages entity control and ownership.
pub mod control;
/// Manages entity visibility for replication (e.g., interest management, rooms).
pub mod visibility;

/// Commonly used items for replication.
pub mod prelude {
    pub use crate::authority::{
        AuthorityPlugin, AuthorityTransfer, AuthorityTransferRequest, AuthorityTransferResponse,
        GiveAuthority, RequestAuthority,
    };
    pub use crate::buffer::Replicate;
    pub use crate::components::*;
    pub use crate::control::{Controlled, Lifetime, Owned, OwnedBy};
    pub use crate::hierarchy::{
        ChildOfSync, DisableReplicateHierarchy, HierarchySendPlugin, RelationshipReceivePlugin,
        RelationshipSendPlugin, RelationshipSync, ReplicateLike, ReplicateLikeChildren,
    };
    pub use crate::message::*;
    pub use crate::plugin::ReplicationSet;
    pub use crate::receive::{ReplicationReceivePlugin, ReplicationReceiver};
    pub use crate::registry::registry::{AppComponentExt, ComponentRegistration};
    pub use crate::send::{
        ReplicationBufferSet, ReplicationSendPlugin, ReplicationSender, SendUpdatesMode,
    };
    pub use crate::visibility::immediate::{NetworkVisibility, NetworkVisibilityPlugin};
    pub use crate::visibility::room::{Room, RoomEvent, RoomPlugin};
}
