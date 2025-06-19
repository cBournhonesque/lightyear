use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy_enhanced_input::input_context::actions::UntypedActions;
use bevy_enhanced_input::prelude::{ActionBinding, InputAction};
use core::any::TypeId;
use lightyear_utils::registry::{TypeKind, TypeMapper};

/// [`InputActionKind`] is an internal wrapper around the type of the message
#[derive(Debug, Eq, Hash, Copy, Clone, PartialEq, Reflect)]
pub struct InputActionKind(pub TypeId);

impl TypeKind for InputActionKind {}

impl From<TypeId> for InputActionKind {
    fn from(value: TypeId) -> Self {
        Self(value)
    }
}

/// Registry to assign a unique network ID for each [`InputAction`] type.
#[derive(Resource, Default)]
pub struct InputRegistry {
    pub(crate) kind_map: TypeMapper<InputActionKind>,
    input_action_metadata: HashMap<InputActionKind, InputActionMetadata>,
}

#[derive(thiserror::Error, Debug)]
pub enum InputRegistryError {
    #[error("No metadata found for input action with Kind: {0:?}")]
    NoMetadataFound(InputActionKind),
}

impl InputRegistry {
    pub(crate) fn add<A: InputAction>(&mut self) {
        let kind = self.kind_map.add::<A>();
        let metadata = InputActionMetadata::new::<A>();
        self.input_action_metadata.insert(kind, metadata);
    }

    pub(crate) fn bind<'r: 'a, 'a>(
        &'r self,
        kind: InputActionKind,
        actions: &'a mut UntypedActions,
    ) -> Result<&'a mut ActionBinding, InputRegistryError> {
        let metadata = self
            .input_action_metadata
            .get(&kind)
            .ok_or(InputRegistryError::NoMetadataFound(kind))?;

        let bind_fn: BindFn<'a> = unsafe { core::mem::transmute(metadata.bind_fn) };
        Ok(bind_fn(actions))
    }
}

struct InputActionMetadata {
    bind_fn: fn(),
}

type BindFn<'a> = fn(&'a mut UntypedActions) -> &'a mut ActionBinding;

impl InputActionMetadata {
    fn new<A: InputAction>() -> Self {
        // This is a placeholder for the actual binding function.
        // In a real implementation, this would be replaced with the actual binding logic.
        let bind_fn: for<'a> fn(&'a mut UntypedActions) -> &'a mut ActionBinding =
            UntypedActions::bind::<A>;
        Self {
            bind_fn: unsafe { core::mem::transmute(bind_fn) },
        }
    }
}

pub trait InputRegistryExt {
    /// Registers a new input action type and returns its kind.
    fn register_input_action<A: InputAction>(self) -> Self;
}

impl InputRegistryExt for &mut App {
    fn register_input_action<A: InputAction>(self) -> Self {
        if !self.world().contains_resource::<InputRegistry>() {
            self.insert_resource(InputRegistry::default());
        }
        let mut registry = self.world_mut().resource_mut::<InputRegistry>();
        registry.add::<A>();
        trace!(
            "Registered input action: {:?}. registry kinds: {:?}",
            core::any::type_name::<A>(),
            registry.kind_map
        );
        self
    }
}
