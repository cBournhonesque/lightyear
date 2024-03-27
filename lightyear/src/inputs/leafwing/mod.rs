//! Handles buffering and networking of inputs from client to server, using `leafwing_input_manager`

use std::fmt::Debug;

use bevy::prelude::{FromReflect, TypePath};
use bevy::reflect::Reflect;
use leafwing_input_manager::Actionlike;
use serde::{Deserialize, Serialize};

pub use input_buffer::InputMessage;

use crate::protocol::BitSerializable;

pub(crate) mod input_buffer;

/// An enum that represents a list of user actions.
///
/// See more information in the leafwing_input_manager crate: [`Actionlike`]
pub trait LeafwingUserAction:
    BitSerializable
    + Copy
    + Clone
    + PartialEq
    + Send
    + Sync
    + Debug
    + Actionlike
    + TypePath
    + FromReflect
    + 'static
{
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum NoAction1 {}

impl LeafwingUserAction for NoAction1 {}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Copy, Hash, Reflect, Actionlike)]
pub enum NoAction2 {}

impl LeafwingUserAction for NoAction2 {}
