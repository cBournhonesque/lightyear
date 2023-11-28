//! # Packet
//! This module defines the concept of a [`packet`] which is a byte array that will be sent over the network.
//! A [`packet`] has a maximum size that depends on the transport (around 1400 bytes for UDP), and is
//! composed of a header and a payload.
//!
//! The header will compute important information such as the packet sequence number, the packet type, etc.
//! as well as information to handle the ack system.
//!
//! The payload is a list of messages that are included in the packet. Messages will be included in the packet
//! in order of [`channel`] priority.
//!
//! Packets that are over the maximum packet size will be fragmented into multiple [`FragmentedPacket`].
//!
//! [`packet`]: crate::packet::packet::Packet
//! [`channel`]: crate::channel::builder::Channel
//! [`FragmentedPacket`]: crate::packet::packet::FragmentedPacket

/// Manages the [`PacketHeader`](crate::packet::header::PacketHeader) which includes important packet information
pub mod header;

/// Defines the [`Message`](message::Message) struct, which is a piece of serializable data
pub mod message;

/// Manages sending and receiving [`Packets`](crate::packet::packet::Packet) over the network
pub mod message_manager;

/// Defines the [`Packet`](crate::packet::packet::Packet) struct
pub mod packet;

/// Manages building a single [`Packet`](crate::packet::packet::Packet) from multiple [`Messages`](crate::packet::message::Message)
pub(crate) mod packet_manager;

/// Defines the [`PacketType`](crate::packet::packet_type::PacketType) enum
mod packet_type;
