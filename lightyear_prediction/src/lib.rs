//! Handles client-side prediction
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

use crate::manager::{PredictionManager, PredictionResource};
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
pub(crate) mod manager;
pub mod resource_history;
pub mod rollback;
pub mod spawn;
mod registry;


pub mod prelude {
    pub use crate::despawn::PredictionDespawnCommandsExt;
    pub use crate::manager::PredictionManager;
    pub use crate::plugin::PredictionPlugin;
    pub use crate::registry::PredictionRegistrationExt;
    pub use crate::{Predicted, PredictionMode};
}

/// Component added to client-side entities that are predicted.
///
/// Prediction allows the client to simulate the game state locally without waiting for server confirmation,
/// reducing perceived latency. This component links the predicted entity to its server-confirmed counterpart.
///
/// When an entity is marked as `Predicted`, the `PredictionPlugin` will:
/// - Store its component history.
/// - Rollback and re-simulate the entity when a server correction is received.
/// - Manage the relationship between the predicted entity and its corresponding confirmed entity received from the server.
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
                let Some(confirmed) = deferred_world
                    .get::<Predicted>(predicted)
                    .unwrap()
                    .confirmed_entity else {
                    return
                };
                let Some(mut resource) =
                        deferred_world.get_resource::<PredictionResource>() else {
                    return
                };
                let Some(mut manager) =
                    deferred_world.get_mut::<PredictionManager>(resource.link_entity) else {
                    return
                };
                manager
                    .predicted_entity_map
                    .get_mut()
                    .confirmed_to_predicted
                    .insert(confirmed, predicted);
            },
        );
        hooks.on_remove(
            |mut deferred_world: DeferredWorld, hook_context: HookContext| {
                let predicted = hook_context.entity;
                let Some(confirmed) = deferred_world
                    .get::<Predicted>(predicted)
                    .unwrap()
                    .confirmed_entity else {
                    return
                };
                let Some(mut resource) =
                        deferred_world.get_resource::<PredictionResource>() else {
                    return
                };
                let Some(mut manager) =
                    deferred_world.get_mut::<PredictionManager>(resource.link_entity) else {
                    return
                };
                manager
                    .predicted_entity_map
                    .get_mut()
                    .confirmed_to_predicted
                    .remove(&confirmed);

            },
        );
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
/// Defines how a predicted or interpolated component will be replicated from confirmed to predicted/interpolated
///
/// We use a single enum instead of 2 separate enums because we want to be able to use the same enum for both predicted and interpolated components
/// Otherwise it would be pretty tedious to have to set the values for both prediction and interpolation.
pub enum PredictionMode {
    /// Sync the component from the confirmed to the interpolated/predicted entity with the most precision
    /// Predicted: we will check for rollback every tick
    Full,

    /// Simple sync: whenever the confirmed entity gets updated, we propagate the update to the interpolated/predicted entity
    /// Use this for components that don't get updated often or are not time-sensitive
    ///
    /// Predicted: that means the component's state will be ~1-RTT behind the predicted entity's timeline
    Simple,

    /// The component will be copied only-once from the confirmed to the interpolated/predicted entity, and then won't stay in sync
    /// Useful for components that you want to modify yourself on the predicted/interpolated entity
    Once,

    #[default]
    /// The component is not copied from the Confirmed entity to the interpolated/predicted entity
    None,
}

/// Trait for components that can be synchronized between a confirmed entity and its predicted/interpolated counterpart.
///
/// This is a marker trait, requiring `Component<Mutability=Mutable> + Clone + PartialEq`.
/// Components implementing this trait can have their state managed by the prediction and interpolation systems
/// according to the specified `PredictionMode`.
pub trait SyncComponent: Component<Mutability=Mutable> + Clone + PartialEq {}
impl<T> SyncComponent for T where T: Component<Mutability=Mutable> + Clone + PartialEq {}
