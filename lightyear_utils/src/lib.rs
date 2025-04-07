//! Contains a set of useful utilities

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

// re-exports
#[doc(hidden)]
pub(crate) mod _internal {
}

pub mod free_list;

pub mod ready_buffer;

pub mod sequence_buffer;

// pub mod bevy;

#[cfg_attr(docsrs, doc(cfg(feature = "avian2d")))]
#[cfg(feature = "avian2d")]
pub mod avian2d;

#[cfg_attr(docsrs, doc(cfg(feature = "avian3d")))]
#[cfg(feature = "avian3d")]
pub mod avian3d;

pub mod captures;
pub mod collections;
// pub(crate) mod pool;


pub mod wrapping_id;
pub mod registry;
mod easings;