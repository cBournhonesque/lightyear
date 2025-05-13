use bevy::ecs::component::Mutable;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::{Component, EntityMapper, Reflect};
use core::fmt::Debug;
use core::marker::PhantomData;
use lightyear_inputs::input_buffer::{InputBuffer, InputData};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The component that will store the current status of the action for the entity
#[derive(Component, Clone, Debug, PartialEq, Serialize, Deserialize, Reflect)]
pub struct ActionState<A> {
    pub value: Option<A>,
}

impl<A: MapEntities> MapEntities for ActionState<A> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {
        if let Some(value) = &mut self.value {
            value.map_entities(entity_mapper);
        }
    }
}

impl<A: Clone> From<&ActionState<A>> for InputData<A> {
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
