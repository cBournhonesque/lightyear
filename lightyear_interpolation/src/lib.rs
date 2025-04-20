//! Handles interpolation of entities between server updates
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::ecs::component::{HookContext, Mutable, StorageType};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Entity, Reflect, ReflectComponent};
use core::ops::{Add, Mul};
pub use interpolate::InterpolateStatus;
pub use interpolation_history::ConfirmedHistory;
use lightyear_replication::prelude::{InitialReplicated, Replicated};
pub use plugin::{add_interpolation_systems, add_prepare_interpolation_systems};
use tracing::error;

use crate::manager::InterpolationManager;

mod despawn;
pub mod interpolate;
pub mod interpolation_history;
pub mod plugin;
mod manager;
mod spawn;
mod registry;

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
            // TODO: maybe we need InitialReplicated?
            let Some(replicated) = deferred_world.get::<Replicated>(confirmed) else {
                error!("Could not find the receiver assocaited with the interpolated entity {:?}", interpolated);
                return;
            };
            if let Some(mut manager) = deferred_world.get_mut::<InterpolationManager>(replicated.receiver) {
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
            let Some(replicated) = deferred_world.get::<Replicated>(confirmed) else {
                error!("Could not find the receiver assocaited with the interpolated entity {:?}", interpolated);
                return;
            };
            if let Some(mut manager) = deferred_world.get_mut::<InterpolationManager>(replicated.receiver) {
                manager
                    .interpolated_entity_map
                    .get_mut()
                    .confirmed_to_interpolated
                    .insert(confirmed, interpolated);
            };
        });
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
/// Defines how interpolated component will be copied from the confirmed entity to the interpolated entity
pub enum InterpolationMode {
    /// Sync the component from the confirmed to the interpolated entity with the most precision
    /// Interpolated: we will run interpolation between the last 2 confirmed states
    Full,

    /// Simple sync: whenever the confirmed entity gets updated, we propagate the update to the interpolated entity
    /// Use this for components that don't get updated often or are not time-sensitive
    ///
    /// Interpolated: that means the component might not be rendered smoothly as it will only be updated after we receive a server update
    Simple,

    /// The component will be copied only-once from the confirmed to the interpolated entity, and then won't stay in sync
    /// Useful for components that you want to modify yourself on the interpolated entity
    Once,

    #[default]
    /// The component is not copied from the Confirmed entity to the interpolated entity
    None,
}


pub trait SyncComponent: Component<Mutability=Mutable> + Clone + PartialEq {}
impl<T> SyncComponent for T where T: Component<Mutability=Mutable> + Clone + PartialEq {}