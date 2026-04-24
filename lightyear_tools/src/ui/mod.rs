//! Runtime UI tools for Lightyear.

pub mod debug;

pub use debug::DebugUIPlugin;

pub mod prelude {
    pub use crate::ui::debug::DebugUIPlugin;
}
