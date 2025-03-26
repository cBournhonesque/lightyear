/*! # Lightyear Connection

Connection handling for the lightyear networking library.
This crate provides abstractions for managing long-term connections.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::SystemSet;

pub mod client;

pub mod server;

pub mod id;
pub mod network_target;

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