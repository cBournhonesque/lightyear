/*! # Lightyear Connection
Connection handling for the lightyear networking library.
This crate provides core types for managing long-term connections on top of a Link and Transport.

It also introduces helpers to setup a Client-Server architecture.

This crates provide concepts that are only useful for a client-server architecture (client/server).
*/
#![cfg_attr(not(feature = "std"), no_std)]
// TODO: maybe lightyear_connection only stores primitives for a long-running Connection (ConnectionError, etc.)
//  on top of a Link
//  And client-server logic is only in lightyear_client, lightyear_server, lightyear_shared
//  OR:
//   for example the direction stuff should be lightyear_client + lightyear_server + lightyear_shared?
//  --
//   Fundamentally is it easier to find direction logic in lightyear_client/direction + lightyear_server/direction
//   or in lightyear_direction/client + lightyear_direction/server?
//   Maybe each crate can have #[client] and #[server] features for client-server specific stuff
//   And then lightyear_client just calls the relevant functions from all the other crates (inputs, etc.)


extern crate alloc;
extern crate core;

use bevy::prelude::SystemSet;

pub mod client;

#[cfg(feature = "server")]
pub mod server;

pub mod network_target;
pub mod direction;

pub mod client_of;
pub mod identity;
pub mod shared;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum ConnectionSet {
    // PRE UPDATE
    /// Receive bytes from the Link, process them as Packets and buffer them into the Transport
    Receive,

    // PostUpdate
    /// Flush the messages that were buffered into the Transport, process them as Packets and
    /// buffer them to the Link
    Send,
}

pub mod prelude {
    pub use crate::direction::NetworkDirection;
    pub use crate::network_target::NetworkTarget;
    pub use crate::ConnectionSet;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::{Client, Connect, Connected, Connecting, ConnectionError, Disconnect, Disconnected};
    }
    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::client_of::{ClientOf, Server};
        pub use crate::server::{ConnectionError, Start, Started, Starting, Stop, Stopped};
    }
}


