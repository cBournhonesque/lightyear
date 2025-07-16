use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::{EntityMapper, MapEntities},
};
use bevy_reflect::Reflect;
use core::fmt::Debug;
use core::marker::PhantomData;
use lightyear_inputs::input_buffer::InputData;

use serde::{Deserialize, Serialize};

/// The component that will store the current status of the action for the entity
///
/// Note that your action HAS to implement `MapEntities` and `Default`.
/// The `Default` value should be when no actions/inputs are active.
/// It is important to distinguish between "no input" (e.g. no keys pressed) and "input not received" (e.g. network packet loss).
#[derive(
    Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect, Deref, DerefMut,
)]
pub struct ActionState<A>(pub A);

impl<A: MapEntities> MapEntities for ActionState<A> {
    fn map_entities<E: EntityMapper>(&mut self, entity_mapper: &mut E) {
        self.0.map_entities(entity_mapper);
    }
}

impl<A: Clone> From<&ActionState<A>> for InputData<A> {
    fn from(value: &ActionState<A>) -> Self {
        InputData::Input(value.0.clone())
    }
}

/// Marker component to identify the ActionState that the player is actively updating
/// (as opposed to the ActionState of other players, for instance)
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct InputMarker<A> {
    marker: PhantomData<A>,
}
