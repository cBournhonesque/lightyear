/*! # Lightyear Sync

This crate provides the synchronization layer for the Lightyear networking library.
It defines a [`Timeline`] trait, etc.

This is agnostic to the client or server, any peer can sync a timeline to another timeline.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;


pub mod ping;
#[cfg(feature = "client")]
pub mod client;

// TODO: server: we might want each ClientOf to use the timeline of the parent

#[cfg(feature = "server")]
pub mod server;


pub mod timeline;
pub mod plugin;

pub mod prelude {
    pub use crate::ping::manager::{PingConfig, PingManager};
    pub use crate::ping::message::{Ping, Pong};
    pub use crate::ping::PingChannel;
    pub use crate::plugin::SyncPlugin;
    pub use crate::timeline::{input::InputTimeline, DrivingTimeline};

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::timeline::input::{Input, InputTimeline};
        #[cfg(feature = "interpolation")]
        pub use crate::timeline::interpolation::{Interpolation, InterpolationTimeline};
        pub use crate::timeline::remote::{RemoteEstimate, RemoteTimeline};
        pub use crate::timeline::sync::IsSynced;
    }

    #[cfg(feature = "server")]
    pub mod server {}
}
