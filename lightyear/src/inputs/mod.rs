/*!
Handles dealing with inputs (keyboard presses, mouse clicks) sent from a player (client) to server
*/

/// Defines an [`InputBuffer`](input_buffer::InputBuffer) buffer to store the inputs of a player for each tick
pub mod input_buffer;

use crate::protocol::BitSerializable;
use std::fmt::Debug;

// TODO: should we request that a user input is a message?
pub trait UserInput:
    BitSerializable + Clone + Eq + PartialEq + Send + Sync + Debug + 'static
{
}

impl UserInput for () {}
