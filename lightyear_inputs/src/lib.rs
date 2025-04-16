/*! # Lightyear IO

Low-level IO primitives for the lightyear networking library.
This crate provides abstractions for sending and receiving raw bytes over the network.
*/
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;
extern crate core;

#[cfg(feature = "client")]
mod client;

#[cfg(feature = "server")]
mod server;
mod input_buffer;
mod config;

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

