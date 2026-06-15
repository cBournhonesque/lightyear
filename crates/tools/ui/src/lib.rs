//! # Lightyear UI
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod debug;

pub mod prelude {
    pub use crate::debug::DebugUIPlugin;
}
