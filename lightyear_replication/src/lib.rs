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

#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

/// Handles the registration of components for replication.
pub mod registry;

/// Defines components related to replication, such as `Replicate` and `ParentSync`.
pub mod components;

pub(crate) mod authority;
/// Defines error types that can occur during replication.
pub mod error;
pub(crate) mod hierarchy;
pub(crate) mod plugin;
/// Handles receiving and applying replication updates on the client.
pub mod receive;

pub mod send;

pub mod delta;

/// Defines the structure of messages used for replication.
pub mod message;

/// Manages entity control and ownership.
pub mod control;
pub mod host;
mod impls;

pub mod prespawn;
/// Manages entity visibility for replication (e.g., interest management, rooms).
pub mod visibility;

/// Commonly used items for replication.
pub mod prelude {
    pub use crate::authority::{
        AuthorityBroker, AuthorityPlugin, AuthorityTransfer, GiveAuthority, HasAuthority,
        RequestAuthority,
    };
    pub use crate::components::*;
    pub use crate::control::{Controlled, ControlledBy, ControlledByRemote, Lifetime};
    pub use crate::delta::{DeltaComponentHistory, DeltaManager, Diffable};
    pub use crate::hierarchy::{
        DisableReplicateHierarchy, HierarchySendPlugin, ReplicateLike, ReplicateLikeChildren,
    };
    pub use crate::message::*;
    pub use crate::plugin::ReplicationSystems;
    pub use crate::prespawn::PreSpawned;
    pub use crate::receive::{ReplicationReceivePlugin, ReplicationReceiver};
    pub use crate::registry::registry::{
        AppComponentExt, ComponentRegistration, ComponentRegistry, TransformLinearInterpolation,
    };
    pub use crate::send::components::*;
    pub use crate::send::plugin::{ReplicationBufferSystems, ReplicationSendPlugin};
    pub use crate::send::sender::{ReplicationSender, SendUpdatesMode};
    pub use crate::visibility::immediate::{NetworkVisibility, NetworkVisibilityPlugin};
    pub use crate::visibility::room::{Room, RoomEvent, RoomPlugin, RoomTarget};
}
