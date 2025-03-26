/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::{Component, SystemSet};
use bytes::Bytes;

/*! Modules related to the client
*/

pub mod components;

pub mod config;

pub mod connection;

pub mod events;

pub mod input;

pub mod interpolation;

pub mod plugin;

pub mod prediction;

pub mod sync;

pub mod diagnostics;
mod easings;

pub(crate) mod io;
pub mod message;
pub mod networking;
pub mod replication;

pub mod error;
pub mod run_conditions;
#[cfg(target_family = "wasm")]
pub mod web;
