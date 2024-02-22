/// Return the number of seconds since unix epoch
pub(crate) fn now() -> u64 {
    // number of seconds since unix epoch
    bevy::utils::SystemTime::now()
        .duration_since(bevy::utils::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64() as u64
}
