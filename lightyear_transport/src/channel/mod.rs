//! Channel configuration and per-channel send/receive implementations.
//!
//! Channels add delivery semantics on top of packetized transport bytes. A channel is identified by
//! a marker type implementing [`Channel`](crate::channel::Channel), registered in
//! [`ChannelRegistry`](crate::channel::registry::ChannelRegistry), and configured with
//! [`ChannelSettings`](crate::channel::builder::ChannelSettings). A
//! [`Transport`](crate::transport::Transport) component then owns channel sender/receiver
//! state for a specific connection entity.

/// Stable type-based identifier for a registered [`Channel`].
pub use crate::channel::registry::ChannelKind;

/// Channel settings and delivery modes.
pub mod builder;
/// Channel receiver implementations.
pub mod receivers;
/// Channel sender implementations.
pub mod senders;

/// Type-to-network-ID channel registry.
pub mod registry;
#[cfg(feature = "trace")]
/// Channel statistics helpers used by tracing/metrics builds.
pub mod stats;

/// Marker trait for an application-defined channel.
///
/// The channel type itself carries no data. Register it in
/// [`ChannelRegistry`](crate::channel::registry::ChannelRegistry) with
/// [`AppChannelExt::add_channel`](crate::channel::registry::AppChannelExt::add_channel) to assign a
/// network ID and [`ChannelSettings`](crate::channel::builder::ChannelSettings). Add a direction with
/// [`ChannelRegistration::add_direction`](crate::channel::registry::ChannelRegistration::add_direction)
/// so new client/server transport entities get the right sender and/or receiver.
pub trait Channel: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Channel for T {}
