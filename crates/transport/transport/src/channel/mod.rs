//! Channels add delivery and ordering policies on top of the packet transport.
//!
//! # Migrating from the enum-based channel API
//!
//! The concrete sender/receiver enums and the public `Transport::senders` and
//! `Transport::receivers` maps have been replaced by [`send::ChannelSend`] and
//! [`receive::ChannelReceive`]. This is an intentional breaking API change.
//!
//! - Replace `add_sender(sender, mode, channel_id)` with
//!   [`Transport::add_channel_send`](builder::Transport::add_channel_send), passing the channel's
//!   complete [`ChannelSettings`](builder::ChannelSettings).
//! - Replace `add_receiver(receiver, channel_id)` with
//!   [`Transport::add_channel_receive`](builder::Transport::add_channel_receive), also passing
//!   [`ChannelSettings`](builder::ChannelSettings).
//! - Use [`Transport::channel_send`](builder::Transport::channel_send),
//!   [`Transport::channel_receive`](builder::Transport::channel_receive), and their mutable or
//!   iterator variants instead of accessing the maps directly.
//! - Replace `has_sender` and `has_receiver` with
//!   [`Transport::has_channel_send`](builder::Transport::has_channel_send) and
//!   [`Transport::has_channel_receive`](builder::Transport::has_channel_receive).
//!
//! The deprecated `add_sender_from_registry` and `add_receiver_from_registry` methods retain
//! their original signatures; new code should use `add_channel_send_from_registry` and
//! `add_channel_receive_from_registry`.

pub use crate::channel::registry::ChannelKind;

pub mod builder;
pub(crate) mod fragments;
pub mod receive;
pub mod send;
mod send_reliable;

pub mod registry;
#[cfg(feature = "trace")]
pub mod stats;

/// A Channel is used to specify some properties of how the bytes are sent over the network.
///
/// The properties can be specified using the [`ChannelSettings`](crate::prelude::ChannelSettings).
pub trait Channel: Send + Sync + 'static {}
impl<T: Send + Sync + 'static> Channel for T {}
