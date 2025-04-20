/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
extern crate core;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "server")]
pub mod server;
pub mod input_buffer;
pub mod config;

#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
use bevy::ecs::component::Mutable;
use bevy::prelude::{Component, SystemSet};
use core::fmt::Debug;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub trait UserAction:
    Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + Debug + 'static
{
}

impl<A: Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + Debug + 'static> UserAction
    for A
{
}

pub trait UserActionState: UserAction + Component<Mutability = Mutable> + Default + Debug {
    type UserAction: UserAction;
}


pub mod prelude {
    pub use crate::config::InputConfig;
    pub use crate::input_buffer::InputBuffer;

    #[cfg(feature = "client")]
    pub mod client {
        pub use crate::client::InputSet;
    }
    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::InputSet;
    }
}
