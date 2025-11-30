//! # Lightyear UI
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod plugin;
pub mod registry;

pub use metrics;
pub use metrics_util;

pub mod prelude {
    pub use crate::plugin::{ClearBucketsSystem, MetricsPlugin};
    #[cfg(feature = "std")]
    pub use crate::registry::GLOBAL_RECORDER;

    pub use crate::registry::{MetricsRegistry, SearchResult};
}
