//! Handles client-side prediction
use crate::client::prediction::resource::PredictionManager;
use bevy::ecs::component::{HookContext, Mutable, StorageType};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Entity, Reflect, ReflectComponent};
use core::fmt::Debug;

pub(crate) mod archetypes;
pub mod correction;
pub mod despawn;
pub mod diagnostics;
pub mod plugin;
pub mod pre_prediction;
pub mod predicted_history;
pub mod prespawn;
pub(crate) mod resource;
pub mod resource_history;
pub mod rollback;
pub mod spawn;

/// Marks an entity that is being predicted by the client
#[derive(Debug, Reflect)]
#[reflect(Component)]
pub struct Predicted {
    // This is an option because we could spawn pre-predicted entities on the client that exist before we receive
    // the corresponding confirmed entity
    pub confirmed_entity: Option<Entity>,
}

impl Component for Predicted {
    const STORAGE_TYPE: StorageType = StorageType::Table;

    type Mutability = Mutable;

    fn register_component_hooks(hooks: &mut bevy::ecs::component::ComponentHooks) {
        hooks.on_add(
            |mut deferred_world: DeferredWorld, hook_context: HookContext| {
                let predicted = hook_context.entity;
                if let Some(confirmed) = deferred_world
                    .get::<Predicted>(predicted)
                    .unwrap()
                    .confirmed_entity
                {
                    if let Some(mut manager) =
                        deferred_world.get_resource_mut::<PredictionManager>()
                    {
                        manager
                            .predicted_entity_map
                            .get_mut()
                            .confirmed_to_predicted
                            .insert(confirmed, predicted);
                    };
                }
            },
        );
        hooks.on_remove(
            |mut deferred_world: DeferredWorld, hook_context: HookContext| {
                let predicted = hook_context.entity;
                if let Some(confirmed) = deferred_world
                    .get::<Predicted>(predicted)
                    .unwrap()
                    .confirmed_entity
                {
                    if let Some(mut manager) =
                        deferred_world.get_resource_mut::<PredictionManager>()
                    {
                        manager
                            .predicted_entity_map
                            .get_mut()
                            .confirmed_to_predicted
                            .remove(&confirmed);
                    };
                }
            },
        );
    }
}
