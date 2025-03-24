//! Handles interpolation of entities between server updates
use bevy::ecs::component::{HookContext, Mutable, StorageType};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Entity, Reflect, ReflectComponent};
use core::ops::{Add, Mul};

pub use interpolate::InterpolateStatus;
pub use interpolation_history::ConfirmedHistory;
pub use plugin::{add_interpolation_systems, add_prepare_interpolation_systems};
pub use visual_interpolation::{VisualInterpolateStatus, VisualInterpolationPlugin};

use crate::client::components::LerpFn;
use crate::client::interpolation::resource::InterpolationManager;

mod despawn;
pub mod interpolate;
pub mod interpolation_history;
pub mod plugin;
mod resource;
mod spawn;
pub mod visual_interpolation;

/// Interpolator that performs linear interpolation.
pub struct LinearInterpolator;
impl<C> LerpFn<C> for LinearInterpolator
where
    for<'a> &'a C: Mul<f32, Output = C>,
    C: Add<C, Output = C>,
{
    fn lerp(start: &C, other: &C, t: f32) -> C {
        start * (1.0 - t) + other * t
    }
}

/// Marker component for an entity that is being interpolated by the client
#[derive(Debug, Reflect)]
#[reflect(Component)]
pub struct Interpolated {
    // TODO: maybe here add an interpolation function?
    pub confirmed_entity: Entity,
    // TODO: add config about despawn behaviour here:
    //  - despawn immediately all components
    //  - leave the entity alive until the confirmed entity catches up to it and then it gets removed.
    //    - or do this only for certain components (audio, animation, particles..) -> mode on PredictedComponent
}

impl Component for Interpolated {
    const STORAGE_TYPE: StorageType = StorageType::Table;

    type Mutability = Mutable;
    fn register_component_hooks(hooks: &mut bevy::ecs::component::ComponentHooks) {
        hooks.on_add(|mut deferred_world: DeferredWorld, context: HookContext| {
            let interpolated = context.entity;
            let confirmed = deferred_world
                .get::<Interpolated>(interpolated)
                .unwrap()
                .confirmed_entity;

            if let Some(mut manager) = deferred_world.get_resource_mut::<InterpolationManager>() {
                manager
                    .interpolated_entity_map
                    .get_mut()
                    .confirmed_to_interpolated
                    .insert(confirmed, interpolated);
            };
        });
        hooks.on_remove(|mut deferred_world: DeferredWorld, context: HookContext| {
            let interpolated = context.entity;
            let confirmed = deferred_world
                .get::<Interpolated>(interpolated)
                .unwrap()
                .confirmed_entity;
            if let Some(mut manager) = deferred_world.get_resource_mut::<InterpolationManager>() {
                manager
                    .interpolated_entity_map
                    .get_mut()
                    .confirmed_to_interpolated
                    .insert(confirmed, interpolated);
            };
        });
    }
}
