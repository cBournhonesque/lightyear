/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::prelude::{Component, SystemSet};
use bytes::Bytes;
use core::net::SocketAddr;

#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(dead_code)]

pub(crate) mod host_server_stepper;
mod integration;

pub(crate) mod multi_stepper;
pub mod protocol;
pub(crate) mod stepper;
