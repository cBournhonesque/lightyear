#[cfg(not(target_family = "wasm"))]
pub mod native;

#[cfg(not(target_family = "wasm"))]
pub use native as current;

#[cfg(target_family = "wasm")]
pub mod wasm;

#[cfg(target_family = "wasm")]
pub use wasm as current;
