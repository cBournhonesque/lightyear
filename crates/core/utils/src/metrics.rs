use bevy_platform::sync::atomic::{AtomicBool, Ordering};
use bevy_platform::time::Instant;

/// Struct that can be created to track the time of a specific operation.
///
/// If `incremental` is true, the internal timer gauge will be incremented
pub struct TimerGauge {
    pub name: &'static str,
    start: Instant,
}

impl TimerGauge {
    #[doc(hidden)]
    pub fn from_metric_name(name: &'static str) -> Self {
        Self {
            name,
            start: Instant::now(),
        }
    }
}

/// Creates a [`TimerGauge`](crate::metrics::TimerGauge) from a metric-name prefix.
///
/// The emitted gauge is named `<prefix>/time_ms`. The prefix must be a string
/// literal so the complete name can be assembled at compile time without an
/// allocation.
#[macro_export]
macro_rules! timer_gauge {
    ($name:literal $(,)?) => {
        $crate::metrics::TimerGauge::from_metric_name(::core::concat!($name, "/time_ms"))
    };
}

// TODO: if incremental, we want to reset the gauge to 0 at the end of the frame.
impl Drop for TimerGauge {
    fn drop(&mut self) {
        metrics::gauge!(self.name).set(self.start.elapsed().as_secs_f64() * 1e3_f64);
    }
}

/// Will emit a metric when dropped
pub struct DormantTimerGauge {
    timer: TimerGauge,
    inactive: AtomicBool,
}

impl DormantTimerGauge {
    #[doc(hidden)]
    pub fn from_metric_name(name: &'static str) -> Self {
        Self {
            timer: TimerGauge::from_metric_name(name),
            inactive: AtomicBool::new(true),
        }
    }

    /// Activate the timer; it will now emit a metric when dropped
    pub fn activate(&self) {
        self.inactive.store(false, Ordering::Relaxed)
    }
}

/// Creates a dormant [`DormantTimerGauge`](crate::metrics::DormantTimerGauge) from a metric-name prefix.
///
/// The emitted gauge is named `<prefix>/time_ms`. The timer emits only if it
/// is activated before being dropped.
#[macro_export]
macro_rules! dormant_timer_gauge {
    ($name:literal $(,)?) => {
        $crate::metrics::DormantTimerGauge::from_metric_name(::core::concat!($name, "/time_ms"))
    };
}

impl Drop for DormantTimerGauge {
    fn drop(&mut self) {
        if !self.inactive.load(Ordering::Relaxed) {
            metrics::gauge!(self.timer.name)
                .set(self.timer.start.elapsed().as_secs_f64() * 1e3_f64);
        }
    }
}
