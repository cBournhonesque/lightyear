#[cfg(target_family = "wasm")]
pub use xwt_web_sys as web_sys;
#[cfg(not(target_family = "wasm"))]
pub use xwt_wtransport as wtransport;

#[cfg(target_family = "wasm")]
pub use self::web_sys as current;
#[cfg(not(target_family = "wasm"))]
pub use self::wtransport as current;
