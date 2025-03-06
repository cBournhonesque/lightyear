//! Handles buffering and networking of inputs from client to server, using `leafwing_input_manager`

use crate::inputs::native::UserActionState;
use crate::prelude::UserAction;
use leafwing_input_manager::prelude::ActionState;
use leafwing_input_manager::Actionlike;

pub(crate) mod action_diff;
pub mod input_buffer;
pub mod input_message;

/// An enum that represents a list of user actions.
///
/// See more information in the leafwing_input_manager crate: [`Actionlike`]
pub trait LeafwingUserAction:
    UserAction + Copy + Actionlike + bevy::reflect::GetTypeRegistration
{
}

impl<A: UserAction + Copy + Actionlike + bevy::reflect::GetTypeRegistration> LeafwingUserAction
    for A
{
}

impl<A: LeafwingUserAction> UserActionState for ActionState<A> {
    type UserAction = A;
}
