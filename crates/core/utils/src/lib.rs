//! Contains a set of useful utilities

#![no_std]

extern crate alloc;
extern crate core;
#[cfg(feature = "std")]
extern crate std;

pub mod free_list;

pub mod ready_buffer;

pub mod sequence_buffer;

pub mod captures;
pub mod collections;

pub mod ecs;
pub mod registry;
pub mod wrapping_id;

#[cfg(feature = "metrics")]
pub mod metrics;
