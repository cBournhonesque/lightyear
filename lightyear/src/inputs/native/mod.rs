/*!
Handles dealing with inputs (keyboard presses, mouse clicks) sent from a player (client) to server.
*/

use bevy::prelude::{FromReflect, Reflect, TypePath};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

pub use input_buffer::InputMessage;

use crate::protocol::BitSerializable;

/// Defines an [`InputBuffer`](input_buffer::InputBuffer) buffer to store the inputs of a player for each tick
pub mod input_buffer;

// TODO: should we request that a user input is a message?
pub trait UserAction:
    BitSerializable + Clone + PartialEq + Send + Sync + Debug + TypePath + FromReflect + 'static
{
}

#[derive(Serialize, Deserialize, Debug, Eq, PartialEq, Clone, Copy, Hash, Reflect)]
pub struct NoAction;

impl UserAction for NoAction {}
