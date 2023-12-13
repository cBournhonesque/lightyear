#[cfg(not(target_family = "wasm"))]
mod native;

#[cfg(target_family = "wasm")]
mod wasm;
