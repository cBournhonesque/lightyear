#[cfg(target_arch = "wasm32")]
use web_time::SystemTime;

#[cfg(not(target_arch = "wasm32"))]
use std::time::SystemTime;

/// Return the number of seconds since unix epoch
pub(crate) fn now() -> u64 {
    // number of seconds since unix epoch
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64() as u64
}
