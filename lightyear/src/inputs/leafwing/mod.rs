//! Handles buffering and networking of inputs from client to server, using `leafwing_input_manager`

use std::fmt::Debug;

use leafwing_input_manager::Actionlike;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub(crate) mod action_diff;
pub(crate) mod input_buffer;
pub(crate) mod input_message;

/// An enum that represents a list of user actions.
///
/// See more information in the leafwing_input_manager crate: [`Actionlike`]
pub trait LeafwingUserAction: Serialize + DeserializeOwned + Copy + Debug + Actionlike {}

impl<A: Serialize + DeserializeOwned + Copy + Debug + Actionlike> LeafwingUserAction for A {}
