use bevy::prelude::{App, Reflect, Resource};
use bevy_enhanced_input::prelude::InputAction;
use core::any::TypeId;
use lightyear_utils::registry::{TypeKind, TypeMapper};

/// [`InputActionKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
pub(crate) struct InputActionKind(pub(crate) TypeId);

impl TypeKind for InputActionKind {}

impl From<TypeId> for InputActionKind {
    fn from(value: TypeId) -> Self {
        Self(value)
    }
}

/// Registry to assign a unique network ID for each [`InputAction`](bevy_enhanced_input::InputAction) type.
#[derive(Resource)]
pub struct InputRegistry {
    pub(crate) kind_map: TypeMapper<InputActionKind>,
}

pub trait InputRegistryExt {
    /// Registers a new input action type and returns its kind.
    fn register_input_action<A: InputAction>(self) -> Self;
}

impl InputRegistryExt for &mut App {
    fn register_input_action<A: InputAction>(self) -> Self {
        if !self.world().contains_resource::<InputRegistry>() {
            self.insert_resource(InputRegistry {
                kind_map: TypeMapper::default(),
            });
        }
        let mut registry = self.world_mut().resource_mut::<InputRegistry>();
        registry.kind_map.add::<A>();
        self
    }
}
