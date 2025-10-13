//! # Lightyear Sync
//!
//! This crate provides the synchronization layer for the Lightyear networking library.
//! It handles time synchronization between peers, including:
//! - Ping and RTT estimation (`ping`).
//! - Timeline synchronization to align game state across different peers (`timeline`).
//! - Client and server-specific synchronization logic.
//!
//! The core idea is to allow peers to maintain a synchronized understanding of game time,
//! which is crucial for consistent simulation and prediction.
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

#[cfg(feature = "client")]
/// Client-specific synchronization logic.
pub mod client;
/// Manages pinging and RTT estimation between peers.
pub mod ping;

// TODO: server: we might want each ClientOf to use the timeline of the parent

#[cfg(feature = "server")]
/// Server-specific synchronization logic.
pub mod server;

/// Provides the `SyncPlugin` for integrating synchronization into a Bevy app.
pub mod plugin;
/// Defines timelines and their synchronization mechanisms.
pub mod timeline;

/// Commonly used items from the `lightyear_sync` crate.
pub mod prelude {
    pub use crate::ping::PingChannel;
    pub use crate::ping::manager::{PingConfig, PingManager};
    pub use crate::ping::message::{Ping, Pong};
    pub use crate::plugin::{SyncSystems, TimelineSyncPlugin};
    pub use crate::timeline::sync::{IsSynced, SyncConfig};
    pub use crate::timeline::{
        DrivingTimeline,
        input::{Input, InputTimeline},
    };

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::timeline::input::{Input, InputDelayConfig, InputTimeline};
        pub use crate::timeline::remote::{RemoteEstimate, RemoteTimeline};
        pub use crate::timeline::sync::IsSynced;
    }

    #[cfg(feature = "server")]
    pub mod server {}
}
