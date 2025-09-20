//! Handles interpolation of entities between server updates
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_ecs::{
    component::{Component, HookContext, Mutable},
    world::DeferredWorld,
};
use lightyear_replication::prelude::Replicated;
use tracing::{debug, error};

use crate::manager::InterpolationManager;

mod despawn;
/// Contains interpolation logic.
pub mod interpolate;
/// Defines `ConfirmedHistory` for storing historical states of confirmed entities.
pub mod interpolation_history;
mod manager;
/// Provides the `InterpolationPlugin` and related systems for Bevy integration.
pub mod plugin;
pub mod registry;
mod spawn;
mod sync;
pub mod timeline;

/// Commonly used items for client-side interpolation.
pub mod prelude {
    pub use crate::interpolate::interpolation_fraction;
    pub use crate::interpolation_history::ConfirmedHistory;
    pub use crate::manager::InterpolationManager;
    pub use crate::plugin::{InterpolationDelay, InterpolationPlugin, InterpolationSet};
    pub use crate::registry::{InterpolationRegistrationExt, InterpolationRegistry};
    pub use crate::timeline::InterpolationTimeline;
    pub use crate::{Interpolated, InterpolationMode};
}

pub use lightyear_core::interpolation::Interpolated;

pub(crate) fn interpolated_on_add_hook(mut deferred_world: DeferredWorld, context: HookContext) {
    let interpolated = context.entity;
    let confirmed = deferred_world
        .get::<Interpolated>(interpolated)
        .unwrap()
        .confirmed_entity;
    // It is possible that by the time the interpolation entity gets spawned, the confirmed entity was despawned?
    // TODO: maybe we need InitialReplicated? in case Replicated gets removed?
    let Some(replicated) = deferred_world.get::<Replicated>(confirmed) else {
        error!(
            "Add Interpolated. Could not find the receiver associated with the interpolated entity {:?}",
            interpolated
        );
        return;
    };
    if let Some(mut manager) = deferred_world.get_mut::<InterpolationManager>(replicated.receiver) {
        manager
            .interpolated_entity_map
            .get_mut()
            .confirmed_to_interpolated
            .insert(confirmed, interpolated);
    };
}

pub(crate) fn interpolated_on_remove_hook(mut deferred_world: DeferredWorld, context: HookContext) {
    let interpolated = context.entity;
    let confirmed = deferred_world
        .get::<Interpolated>(interpolated)
        .unwrap()
        .confirmed_entity;
    let Some(replicated) = deferred_world.get::<Replicated>(confirmed) else {
        // this can happen if the confirmed entity is despawned, which despawns the interpolated entity
        debug!(
            "Remove Interpolated. Could not find the receiver associated with the interpolated entity {:?}",
            interpolated
        );
        return;
    };
    if let Some(mut manager) = deferred_world.get_mut::<InterpolationManager>(replicated.receiver) {
        manager
            .interpolated_entity_map
            .get_mut()
            .confirmed_to_interpolated
            .remove(&confirmed);
    };
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
