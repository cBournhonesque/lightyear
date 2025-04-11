//! Contains a set of shared types

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod tick;


pub mod network;

pub mod plugin;
pub mod time;
mod prediction;
mod history_buffer;
pub mod timeline;

pub mod prelude {
    pub use crate::tick::Tick;
    pub use crate::timeline::{LocalTimeline, NetworkTimeline, NetworkTimelinePlugin, Timeline};
}