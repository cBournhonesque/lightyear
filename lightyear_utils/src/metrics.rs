use alloc::format;
use bevy_platform::time::Instant;
use bevy_platform::sync::atomic::{AtomicBool, Ordering};
use tracing::info;

/// Struct that can be created to track the time of a specific operation.
///
/// If `incremental` is true, the internal timer gauge will be incremented
pub struct TimerGauge {
    pub name: &'static str,
    start: Instant,
}

impl TimerGauge {
    /// Create a new `TimerGauge` that will emit a metric when dropped
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
            .set(self.start.elapsed().as_secs_f64() * 1e3 as f64);
    }
}

/// Will emit a metric when dropped
pub struct DormantTimerGauge {
    timer: TimerGauge,
    inactive: AtomicBool,
}

impl DormantTimerGauge {
    /// Create a new [`DormantTimerGauge`] that starts dormant. It will only emit a metric when dropped
    /// if `activate` is called
    pub fn new(name: &'static str) -> Self {
        Self {
            timer: TimerGauge::new(name),
            inactive: AtomicBool::new(true),
        }
    }

    /// Activate the timer; it will now emit a metric when dropped
    pub fn activate(&self) {
        self.inactive.store(false, Ordering::Relaxed)
    }
}

impl Drop for DormantTimerGauge {
    fn drop(&mut self) {
        if !self.inactive.load(Ordering::Relaxed) {
            metrics::gauge!(format!("{}::time_ms", self.timer.name))
                .set(self.timer.start.elapsed().as_secs_f64() * 1e3 as f64);
        }
    }
}
