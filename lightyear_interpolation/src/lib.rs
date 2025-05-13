//! Handles interpolation of entities between server updates
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use bevy::ecs::component::{HookContext, Mutable, StorageType};
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::{Component, Entity, Reflect, ReflectComponent};
pub use interpolate::InterpolateStatus;
pub use interpolation_history::ConfirmedHistory;
use lightyear_replication::prelude::Replicated;
pub use plugin::{add_interpolation_systems, add_prepare_interpolation_systems};
use tracing::error;

use crate::manager::InterpolationManager;

mod despawn;
/// Contains the `InterpolateStatus` component and interpolation logic.
pub mod interpolate;
/// Defines `ConfirmedHistory` for storing historical states of confirmed entities.
pub mod interpolation_history;
mod manager;
/// Provides the `InterpolationPlugin` and related systems for Bevy integration.
pub mod plugin;
mod registry;
mod spawn;
mod timeline;

/// Commonly used items for client-side interpolation.
pub mod prelude {
    pub use crate::manager::InterpolationManager;
    pub use crate::plugin::InterpolationSet;
    pub use crate::registry::InterpolationRegistrationExt;
    pub use crate::timeline::InterpolationTimeline;
    pub use crate::{Interpolated, InterpolationMode};
}

/// Component added to client-side entities that are visually interpolated.
///
/// Interpolation is used to smooth the visual representation of entities received from the server.
/// Instead of snapping to new positions/states upon receiving a server update, the entity's
/// components are smoothly transitioned from their previous state to the new state over time.
///
/// This component links the interpolated entity to its server-confirmed counterpart.
/// The `InterpolationPlugin` uses this to:
/// - Store the component history of the confirmed entity.
/// - Apply interpolated values to the components of this entity based on the `InterpolationTimeline`.
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
                error!(
                    "Could not find the receiver assocaited with the interpolated entity {:?}",
                    interpolated
                );
                return;
            };
            if let Some(mut manager) =
                deferred_world.get_mut::<InterpolationManager>(replicated.receiver)
            {
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
                error!(
                    "Could not find the receiver assocaited with the interpolated entity {:?}",
                    interpolated
                );
                return;
            };
            if let Some(mut manager) =
                deferred_world.get_mut::<InterpolationManager>(replicated.receiver)
            {
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

/// Trait for components that can be synchronized for interpolation.
///
/// This is a marker trait, requiring `Component<Mutability=Mutable> + Clone + PartialEq`.
/// Components implementing this trait can have their state managed by the interpolation systems
/// according to the specified `InterpolationMode`.
pub trait SyncComponent: Component<Mutability = Mutable> + Clone + PartialEq {}
impl<T> SyncComponent for T where T: Component<Mutability = Mutable> + Clone + PartialEq {}
