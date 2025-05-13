//! # Lightyear Avian Integration
//!
//! This crate provides integration between Lightyear and the Avian physics engine.
//!
//! It currently includes utilities for lag compensation.
/// Provides systems and components for lag compensation with Avian.
#[cfg(feature = "lag_compensation")]
pub mod lag_compensation;


#[cfg(feature = "2d")]
pub mod avian2d;

#[cfg(feature = "3d")]
pub mod avian3d;

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
}
