use core::fmt::Debug;
use leafwing_input_manager::Actionlike;
use serde::Serialize;
use serde::de::DeserializeOwned;

pub trait LeafwingUserAction:
    Serialize
    + DeserializeOwned
    + Clone
    + PartialEq
    + Send
    + Sync
    + Debug
    + 'static
    + Copy
    + Actionlike
    + bevy::reflect::GetTypeRegistration
{
}

impl<
    A: Serialize
        + DeserializeOwned
        + Clone
        + PartialEq
        + Send
        + Sync
        + Debug
        + 'static
        + Copy
        + Actionlike
        + bevy::reflect::GetTypeRegistration,
> LeafwingUserAction for A
{
}
