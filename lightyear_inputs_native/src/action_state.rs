use bevy::ecs::component::Mutable;
use bevy::prelude::{Component, Reflect};
use core::fmt::Debug;
use core::marker::PhantomData;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The component that will store the current status of the action for the entity
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
#[require(InputBuffer<ActionState<A>>)]
pub struct ActionState<A: Send + Sync> {
    pub value: Option<A>,
}

impl<A> From<&ActionState<A>> for InputData<A> {
    fn from(value: &ActionState<A>) -> Self {
        value
            .value
            .as_ref()
            .map_or(InputData::Absent, |v| InputData::Input(v.clone()))
    }
}

impl<A> Default for ActionState<A> {
    fn default() -> Self {
        Self { value: None }
    }
}

/// Marker component to identify the ActionState that the player is actively updating
/// (as opposed to the ActionState of other players, for instance)
#[derive(Component, Clone, Copy, Debug, PartialEq, Reflect)]
#[require(ActionState<A>)]
pub struct InputMarker<A> {
    marker: PhantomData<A>,
}

impl<A> Default for InputMarker<A> {
    fn default() -> Self {
        Self {
            marker: PhantomData,
        }
    }
}