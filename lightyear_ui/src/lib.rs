//! # Lightyear UI
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod debug;
pub mod metrics;

pub mod prelude {
    pub use crate::debug::DebugUIPlugin;
    pub use crate::metrics::plugin::{ClearBucketsSystem, RegistryPlugin};
    pub use crate::metrics::registry::{MetricsRegistry, SearchResult};
}
