use js_sys::Date;

pub struct Timestamp;

impl Timestamp {
    pub fn now() -> u64 {
        Date::now() as u64
    }
}
