use alloc::format;
use bevy_platform::time::Instant;

/// Struct that can be created to track the time of a specific operation.
///
/// If `incremental` is true, the internal timer gauge will be incremented
pub struct TimerGauge {
    pub name: &'static str,
    start: Instant,
}

impl TimerGauge {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            start: Instant::now(),
        }
    }
}

// TODO: if incremental, we want to reset the gauge to 0 at the end of the frame.
impl Drop for TimerGauge {
    fn drop(&mut self) {
        metrics::gauge!(format!("{}::time_ms", self.name))
            .set(self.start.elapsed().as_millis() as f64);
    }
}
