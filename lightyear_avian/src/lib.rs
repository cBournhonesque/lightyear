pub mod lag_compensation;

pub mod prelude {
    pub use crate::lag_compensation::history::{
        LagCompensationConfig, LagCompensationHistory, LagCompensationPlugin, LagCompensationSet,
    };
    pub use crate::lag_compensation::query::LagCompensationSpatialQuery;
}
