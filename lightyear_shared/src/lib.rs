/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::{Component, SystemSet};
use bytes::Bytes;

//! Shared code between the server and client.

pub mod config;

pub mod events;

pub mod ping;

pub mod plugin;

pub mod replication;

pub mod sets;

pub mod tick_manager;

pub mod identity;
pub mod input;
pub(crate) mod message;
pub mod run_conditions;
pub mod time_manager;
