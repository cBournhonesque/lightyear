//! Packetization, channels, ordering, and reliability for Lightyear links.
//!
//! `lightyear_transport` sits above `lightyear_link`. Transport plugins move opaque byte payloads
//! in and out of [`Link`](lightyear_link::Link) buffers; this crate turns those bytes into
//! channel-scoped messages, packet headers, acknowledgements, fragmentation, resend behavior, and
//! optional bandwidth prioritization.
//!
//! The main public entry points are:
//! - [`Transport`](crate::transport::Transport), the per-connection component that owns channel
//!   senders/receivers and packet state.
//! - [`Channel`](crate::channel::Channel), a marker trait implemented by application-defined
//!   channel types.
//! - [`ChannelRegistry`](crate::channel::registry::ChannelRegistry), the resource that maps channel
//!   types to stable network IDs and settings.
//! - [`TransportPlugin`](crate::plugin::TransportPlugin), the Bevy plugin that moves messages
//!   between channel buffers and [`Link`](lightyear_link::Link) buffers.
//!
//! Concrete IO crates such as `lightyear_udp`, `lightyear_crossbeam`, `lightyear_websocket`, and
//! `lightyear_webtransport` should remain unaware of these channel semantics.
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

/// Channel marker types, delivery settings, registry, and sender/receiver implementations.
pub mod channel;

/// Transport-level error types.
pub mod error;

#[cfg(feature = "client")]
mod client;
/// Packet headers, packet builders, message fragments, and priority filtering.
pub mod packet;
/// Bevy plugin and schedule sets for packet/channel processing.
pub mod plugin;
#[cfg(feature = "server")]
mod server;
/// Per-connection [`Transport`](crate::transport::Transport) component and message enqueue APIs.
pub mod transport;

/// Re-exports for applications that configure channels or send raw channel payloads.
pub mod prelude {
    pub use crate::channel::Channel;
    pub use crate::channel::builder::{ChannelMode, ChannelSettings, ReliableSettings};
    pub use crate::channel::registry::AppChannelExt;
    pub use crate::channel::registry::ChannelRegistry;
    pub use crate::packet::priority_manager::{PriorityConfig, PriorityManager};
    pub use crate::transport::Transport;
}
