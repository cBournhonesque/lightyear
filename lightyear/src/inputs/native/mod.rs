/*!
Handles inputs (keyboard presses, mouse clicks) sent from a player (client) to server.

NOTE: You should use the `LeafwingInputPlugin` instead (requires the `leafwing` features), which
has mode features and is easier to use.

Lightyear does the following things for you:
- buffers the inputs of a player for each tick
- makes sures that input are replayed correctly during rollback
- sends the inputs to the server in a compressed and reliable form


### Sending inputs

There are several steps to use the `InputPlugin`:
- you need to buffer inputs for each tick. This is done by calling [`add_input`](crate::prelude::client::InputManager::add_input) in a system.
  That system must run in the [`BufferI


*/

use crate::prelude::Deserialize;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Component, Reflect};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fmt::Debug;

/// Defines an [`InputBuffer`](input_buffer::InputBuffer) buffer to store the inputs of a player for each tick
pub mod input_buffer;
pub(crate) mod input_message;

#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
pub struct ActionState<A> {
    value: Option<A>
}

// TODO: should we request that a user input is a message?
// TODO: the bound should be `BitSerializable`, not `Serialize + DeserializeOwned`
//  but it causes the derive macro for InputMessage to fail
pub trait UserAction:
    Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + Debug + MapEntities + 'static
{
}

impl<A: Serialize + DeserializeOwned + Clone + PartialEq + Send + Sync + Debug + 'static> UserAction
    for A
{
}
