//! Handles interpolation of entities between server updates
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_ecs::component::{Component, Mutable};

#[doc(hidden)]
pub mod archetypes;
/// Handles delayed despawns for interpolated entities.
pub mod despawn;
/// Contains interpolation logic.
pub mod interpolate;
/// Provides the `InterpolationPlugin` and related systems for Bevy integration.
pub mod plugin;
pub mod registry;
/// Interpolation rule types and bundle support.
pub mod rules;
pub mod timeline;

/// Commonly used items for client-side interpolation.
pub mod prelude {
    pub use crate::Interpolated;
    pub use crate::interpolate::interpolation_fraction;
    pub use crate::plugin::{InterpolationDelay, InterpolationPlugin, InterpolationSystems};
    pub use crate::registry::{
        AppInterpolationExt, InterpolationRegistrationExt, InterpolationRegistry,
    };
    pub use crate::rules::{
        InterpolationBundle, InterpolationFns, InterpolationFnsExt, InterpolationRuleConfig,
    };
    pub use crate::timeline::InterpolationTimeline;
}

pub use lightyear_core::interpolation::Interpolated;

/// Trait for components that can be synchronized for interpolation.
///
/// This is a marker trait, requiring `Component<Mutability=Mutable> + Clone + PartialEq`.
/// Components implementing this trait can have their state managed by the interpolation systems
/// according to the specified `InterpolationMode`.
pub trait SyncComponent: Component<Mutability = Mutable> + Clone + PartialEq {}
impl<T> SyncComponent for T where T: Component<Mutability = Mutable> + Clone + PartialEq {}
