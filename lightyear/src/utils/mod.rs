//! Contains a set of useful utilities

pub mod named;

pub(crate) mod ready_buffer;

pub(crate) mod sequence_buffer;

pub mod bevy;

#[cfg(all(feature = "xpbd_2d", not(feature = "xpbd_3d")))]
pub mod bevy_xpbd_2d;

#[cfg(all(feature = "xpbd_3d", not(feature = "xpbd_2d")))]
pub mod bevy_xpbd_3d;

pub mod wrapping_id;
