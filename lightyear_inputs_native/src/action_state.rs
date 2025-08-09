use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    entity::{EntityMapper, MapEntities},
    prelude::Mut,
};
use bevy_reflect::Reflect;
use core::fmt::Debug;
use core::marker::PhantomData;
use bevy_ecs::query::QueryData;
use lightyear_inputs::input_buffer::InputData;

use serde::{Deserialize, Serialize};
use lightyear_inputs::input_message::ActionStateQueryData;

/// The component that will store the current status of the action for the entity
///
/// Note that your action HAS to implement `MapEntities` and `Default`.
/// The `Default` value should be when no actions/inputs are active.
/// It is important to distinguish between "no input" (e.g. no keys pressed) and "input not received" (e.g. network packet loss).
#[derive(
    Component, Clone, Debug, Default, PartialEq, Serialize, Deserialize, Reflect, Deref, DerefMut,
)]
pub struct ActionState<A>(pub A);

impl<A: Default + Send + Sync + 'static> ActionStateQueryData for ActionState<A> {
    type Mut = &'static mut Self;
    type MutItemInner<'w> = &'w mut ActionState<A>;
    type Main = ActionState<A>;
    type Bundle = ActionState<A>;

    fn as_read_only<'w, 'a: 'w>(state: &'a Mut<'w, ActionState<A>>) -> &'w ActionState<A> {
        &state
    }

    fn into_inner<'w>(mut_item: <Self::Mut as QueryData>::Item<'w>) -> Self::MutItemInner<'w> {
        mut_item.into_inner()
    }

    fn as_mut<'w>(bundle: &'w mut Self::Bundle) -> Self::MutItemInner<'w> {
        bundle
    }

    fn base_value() -> Self::Bundle {
        ActionState::<A>::default()
    }
}

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
