/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::prelude::{Component, SystemSet};
use bytes::Bytes;

//! Defines the Server bevy resource
//!
//! # Server
//! The server module contains all the code that is used to run the server.

pub mod config;

pub mod connection;

pub mod error;

pub mod events;

pub mod input;

pub(crate) mod io;

pub mod plugin;

pub mod message;
pub(crate) mod prediction;

pub mod clients;
pub(crate) mod networking;
pub mod relevance;
pub mod replication;
pub mod run_conditions;
