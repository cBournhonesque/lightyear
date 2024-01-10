//! Handles buffering and networking of inputs from client to server, using `leafwing_input_manager`

pub(crate) mod input_buffer;

pub use input_buffer::InputMessage;

use crate::protocol::BitSerializable;
use bevy::prelude::{FromReflect, TypePath};
use bevy::reflect::Reflect;
use leafwing_input_manager::Actionlike;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub trait LeafwingUserAction:
    BitSerializable
    + Copy
    + Clone
    + Eq
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
