#[cfg(feature = "lag_compensation")]
pub mod lag_compensation;

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
