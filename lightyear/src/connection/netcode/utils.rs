/// Return the number of seconds since unix epoch
pub(crate) fn now() -> u64 {
    // number of seconds since unix epoch
    instant::SystemTime::now()
        .duration_since(instant::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64() as u64
}
