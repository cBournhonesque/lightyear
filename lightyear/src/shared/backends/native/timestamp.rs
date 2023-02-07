use std::time::SystemTime;

pub struct Timestamp;

impl Timestamp {
    pub fn now() -> u64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("timing error!")
            .as_secs()
    }
}
