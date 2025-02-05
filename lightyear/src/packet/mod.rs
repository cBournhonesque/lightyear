/*!
Module to group messages into packets

# Packet
This module defines the concept of a [`Packet`] which is a byte array that will be sent over the network.
A [`Packet`] has a maximum size that depends on the transport (around 1400 bytes for UDP), and is
composed of a header and a payload.

The header will compute important information such as the packet sequence number, the packet type, etc.
as well as information to handle the ack system.

The payload is a list of messages that are included in the packet. Messages will be included in the packet
in order of [`Channel`] priority.

Packets that are over the maximum packet size will be fragmented into multiple [`FragmentData`].

[`Packet`]: packet::Packet
[`Channel`]: crate::channel::builder::Channel
[`FragmentData`]: message::FragmentData
*/

/// Manages the [`PacketHeader`](header::PacketHeader) which includes important packet information
pub(crate) mod header;

pub(crate) mod message;

/// Manages sending and receiving [`Packets`](packet::Packet) over the network
pub mod message_manager;

// "module has the same name as its containing module" style nit.
// clippy doesn't like this, but not much benefit to changing it now, so silence the warning.
#[allow(clippy::module_inception)]
pub mod packet;

pub(crate) mod error;
/// Manages building a single [`Packet`](packet::Packet) from multiple [`Messages`](message::Message)
pub(crate) mod packet_builder;
/// Defines the [`PacketType`](packet_type::PacketType) enum
mod packet_type;
pub(crate) mod priority_manager;
pub(crate) mod stats_manager;
