//! # Lightyear Avian Integration
//!
//! This crate provides integration between Lightyear and the Avian physics engine.
//!
//! It currently includes utilities for lag compensation.

/// Provides systems and components for lag compensation with Avian.
#[cfg(feature = "lag_compensation")]
pub mod lag_compensation;

#[cfg(feature = "2d")]
pub mod types_2d;
#[cfg(feature = "2d")]
pub use types_2d as types;

#[cfg(any(feature = "2d", feature = "3d"))]
mod sync;
#[cfg(feature = "3d")]
pub mod types_3d;

#[cfg(any(feature = "2d", feature = "3d"))]
mod plugin;

#[cfg(feature = "3d")]
pub use types_3d as types;


/// Commonly used items for Lightyear Avian integration.
pub mod prelude {
    #[cfg(feature = "lag_compensation")]
    pub use crate::lag_compensation::{
        history::{
            AabbEnvelopeHolder, LagCompensationConfig, LagCompensationHistory,
            LagCompensationPlugin, LagCompensationSet,
        },
        query::LagCompensationSpatialQuery,
    };
    #[cfg(any(feature = "2d", feature = "3d"))]
    pub use crate::plugin::LightyearAvianPlugin;
}