//! # Lightyear Tools
#![no_std]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod debug;
pub mod ui;

pub mod prelude {
    pub use crate::debug::prelude::*;
    pub use crate::ui::prelude::*;
}
