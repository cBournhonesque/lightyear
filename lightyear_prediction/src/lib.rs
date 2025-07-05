//! Handles client-side prediction
#![no_std]

extern crate alloc;
extern crate core;
#[cfg(feature = "std")]
extern crate std;

use crate::manager::{PredictionManager, PredictionResource};
use core::fmt::Debug;

#[allow(unused)]
pub(crate) mod archetypes;
pub mod correction;
pub mod despawn;
pub mod diagnostics;
pub mod manager;
pub mod plugin;
pub mod pre_prediction;
pub mod predicted_history;
pub mod prespawn;
mod registry;
pub mod resource_history;
pub mod rollback;
pub mod spawn;

#[cfg(feature = "server")]
pub mod server;
mod shared;

pub mod prelude {
    pub use crate::Predicted;
    pub use crate::PredictionMode;
    pub use crate::despawn::{PredictionDespawnCommandsExt, PredictionDisable};
    pub use crate::manager::{PredictionManager, RollbackMode, RollbackPolicy};
    pub use crate::plugin::{PredictionPlugin, PredictionSet};
    pub use crate::prespawn::PreSpawned;
    pub use crate::registry::{PredictionAppRegistrationExt, PredictionRegistrationExt};

    #[cfg(feature = "server")]
    pub mod server {
        pub use crate::server::ServerPlugin;
    }
}

use bevy_ecs::{
    component::{Component, HookContext, Mutable},
    world::DeferredWorld,
};
pub use lightyear_core::prediction::Predicted;

pub(crate) fn predicted_on_add_hook(mut deferred_world: DeferredWorld, hook_context: HookContext) {
    let predicted = hook_context.entity;
    let Some(confirmed) = deferred_world
        .get::<Predicted>(predicted)
        .unwrap()
        .confirmed_entity
    else {
        return;
    };
    let Some(resource) = deferred_world.get_resource::<PredictionResource>() else {
        return;
    };
    let Some(mut manager) = deferred_world.get_mut::<PredictionManager>(resource.link_entity)
    else {
        return;
    };
    manager
        .predicted_entity_map
        .get_mut()
        .confirmed_to_predicted
        .insert(confirmed, predicted);
}

pub(crate) fn predicted_on_remove_hook(
    mut deferred_world: DeferredWorld,
    hook_context: HookContext,
) {
    let predicted = hook_context.entity;
    let Some(confirmed) = deferred_world
        .get::<Predicted>(predicted)
        .unwrap()
        .confirmed_entity
    else {
        return;
    };
    let Some(resource) = deferred_world.get_resource::<PredictionResource>() else {
        return;
    };
    let Some(mut manager) = deferred_world.get_mut::<PredictionManager>(resource.link_entity)
    else {
        return;
    };
    manager
        .predicted_entity_map
        .get_mut()
        .confirmed_to_predicted
        .remove(&confirmed);
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
pub trait SyncComponent: Component<Mutability = Mutable> + Clone + PartialEq + Debug {}
impl<T> SyncComponent for T where T: Component<Mutability = Mutable> + Clone + PartialEq + Debug {}
