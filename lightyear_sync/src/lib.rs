/*! # Lightyear Sync

This crate provides the synchronization layer for the Lightyear networking library.
It defines a [`Timeline`] trait, etc.

This is agnostic to the client or server, any peer can sync a timeline to another timeline.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;

use bevy::prelude::{Component, SystemSet};

pub mod ping;
#[cfg(feature = "client")]
pub mod client;
pub mod timeline;
mod plugin;
