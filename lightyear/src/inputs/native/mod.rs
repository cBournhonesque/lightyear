/*!
Handles dealing with inputs (keyboard presses, mouse clicks) sent from a player (client) to server.
*/

use bevy::prelude::TypePath;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;

pub use input_buffer::InputMessage;

use crate::protocol::BitSerializable;

/// Defines an [`InputBuffer`](input_buffer::InputBuffer) buffer to store the inputs of a player for each tick
pub mod input_buffer;

// TODO: should we request that a user input is a message?
// TODO: the bound should be `BitSerializable`, not `Serialize + DeserializeOwned`
//  but it causes the derive macro for InputMessage to fail
pub trait UserAction:
    Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + Debug + 'static
{
}

impl UserAction for () {}
