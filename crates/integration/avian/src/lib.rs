//! # Lightyear Avian Integration
//!
//! This crate integrates Lightyear replication, prediction, rollback, frame interpolation, and
//! lag compensation with the Avian physics engine.
//!
//! For networked physics, install [`plugin::LightyearAvianPlugin`] and disable Avian's
//! `PhysicsTransformPlugin` and `PhysicsInterpolationPlugin`. The default and recommended
//! [`plugin::AvianReplicationMode::Position`] keeps Avian `Position` and `Rotation` authoritative
//! and writes their final visual values to Bevy `Transform` in `PostUpdate`. Setting its
//! `sync_to_transform` field to `true` synchronizes that authoritative pose to `Transform` before
//! `FixedUpdate`, allowing fixed-tick gameplay to use `Transform`.
//!
//! Position mode also registers the standard rigid-body `Position`, `Rotation`,
//! `LinearVelocity`, and `AngularVelocity` networking rules by default. Add this plugin after the
//! Lightyear networking plugins or your component protocol. Specialized protocols can disable
//! those defaults with [`plugin::LightyearAvianPlugin::register_physics_components`].
#![allow(unexpected_cfgs)]
#![no_std]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

/// Provides systems and components for lag compensation with Avian.
#[cfg(feature = "lag_compensation")]
pub mod lag_compensation;

#[cfg(feature = "2d")]
pub mod types_2d;
#[cfg(feature = "2d")]
pub use types_2d as types;

#[cfg(feature = "3d")]
pub mod types_3d;

#[cfg(feature = "3d")]
pub use types_3d as types;

#[cfg(any(feature = "2d", feature = "3d"))]
pub mod plugin;

/// Commonly used items for Lightyear Avian integration.
pub mod prelude {
    #[cfg(feature = "lag_compensation")]
    pub use crate::lag_compensation::{
        history::{
            AabbEnvelopeHolder, LagCompensationConfig, LagCompensationHistory,
            LagCompensationPlugin, LagCompensationSystems,
        },
        query::LagCompensationSpatialQuery,
    };
    #[cfg(any(feature = "2d", feature = "3d"))]
    pub use crate::plugin::LightyearAvianPlugin;
}
