/*!
Handles dealing with inputs (keyboard presses, mouse clicks) sent from a player (client) to server.
*/

use std::fmt::Debug;

pub use input_buffer::InputMessage;

use crate::protocol::BitSerializable;

/// Defines an [`InputBuffer`](input_buffer::InputBuffer) buffer to store the inputs of a player for each tick
pub mod input_buffer;

// TODO: should we request that a user input is a message?
pub trait UserAction: BitSerializable + Clone + PartialEq + Send + Sync + Debug + 'static {}

impl UserAction for () {}
