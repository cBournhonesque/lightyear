extern "C" {
    pub fn naia_now() -> f64;
}

pub struct Timestamp;

impl Timestamp {
    pub fn now() -> u64 {
        unsafe { naia_now() as u64 }
    }
}
