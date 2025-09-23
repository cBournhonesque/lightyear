//! Handles interpolation of entities between server updates
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

use bevy_ecs::component::{Component, Mutable};

mod despawn;
/// Contains interpolation logic.
pub mod interpolate;
/// Defines `ConfirmedHistory` for storing historical states of confirmed entities.
pub mod interpolation_history;
/// Provides the `InterpolationPlugin` and related systems for Bevy integration.
pub mod plugin;
pub mod registry;
pub mod timeline;

/// Commonly used items for client-side interpolation.
pub mod prelude {
    pub use crate::Interpolated;
    pub use crate::interpolate::interpolation_fraction;
    pub use crate::interpolation_history::ConfirmedHistory;
    pub use crate::plugin::{InterpolationDelay, InterpolationPlugin, InterpolationSet};
    pub use crate::registry::{InterpolationRegistrationExt, InterpolationRegistry};
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
