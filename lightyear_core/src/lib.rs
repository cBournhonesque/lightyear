//! Contains a set of shared types

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
extern crate core;

pub mod tick;


pub mod network;

pub mod plugin;
pub mod time;
pub mod history_buffer;
pub mod timeline;
pub mod id;

pub mod prelude {
    pub use crate::id::PeerId;
    pub use crate::tick::Tick;
    pub use crate::timeline::{LocalTimeline, NetworkTimeline, NetworkTimelinePlugin, RollbackState, Timeline};
}