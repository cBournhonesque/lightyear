use core::time::Duration;
use tracing::{error, trace};

const RTT_EWMA_ALPHA: f64 = 1.0 / 12.0;
const RTT_DEV_EWMA_BETA: f64 = 1.0 / 6.0;

// --- Constants for Outlier Clamping ---
// An RTT sample is considered an outlier if it's greater than:
// smoothed_rtt + OUTLIER_STDDEV_FACTOR * rtt_abs_deviation
// This factor determines how many "standard deviations" (using abs_deviation as a proxy)
// away a sample can be before being clamped.
const OUTLIER_STDDEV_FACTOR: f64 = 3.0; // e.g., 3 times the current deviation

// Additionally, a sample won't be allowed to be more than X times the current smoothed RTT.
// This helps when rtt_abs_deviation is very small, making the stddev factor too restrictive.
const MAX_RTT_RELATIVE_INCREASE_FACTOR: f64 = 3.0; // e.g., sample clamped if > 3 * smoothed_rtt

// And an absolute cap on the increase to prevent excessive clamping if SRTT is tiny.
// The sample will be clamped if it's greater than smoothed_rtt + MAX_RTT_ABSOLUTE_INCREASE_SECS
// This provides a safety net if SRTT is very low.
const MAX_RTT_ABSOLUTE_INCREASE_SECS: f64 = 0.5; // e.g., 500ms absolute increase allowed over SRTT

/// Holds the final computed RTT and Jitter.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FinalStats {
    pub rtt: Duration,
    pub jitter: Duration,
}

/// Estimates RTT and Jitter using Exponentially Weighted Moving Averages.
#[derive(Debug, Default)]
pub struct RttEstimatorEwma {
    /// Smoothed RTT estimate.
    smoothed_rtt: Option<Duration>,
    /// Smoothed absolute deviation of RTT samples from the smoothed_rtt.
    /// This is used as the basis for jitter.
    rtt_abs_deviation: Option<Duration>,
    /// The latest computed statistics.
    pub final_stats: FinalStats,
    samples_processed: u64,
}

impl RttEstimatorEwma {
    /// Creates a new RTT estimator with no initial data.
    pub fn new() -> Self {
        RttEstimatorEwma {
            smoothed_rtt: None,
            rtt_abs_deviation: None,
            final_stats: FinalStats::default(),
            samples_processed: 0,
        }
    }

    /// Updates the RTT and jitter estimates with a new RTT sample.
    ///
    /// - `new_rtt_sample`: The most recent RTT measurement.
    pub fn update_with_new_sample(&mut self, new_rtt_sample: Duration) {
        let mut rtt_sample_secs = new_rtt_sample.as_secs_f64();

        // RTT samples should be non-negative.
        if rtt_sample_secs < 0.0 {
            error!(
                "Received negative RTT sample, ignoring: {:?}",
                new_rtt_sample
            );
            // Optionally, you might want to reset or handle this as an error.
            return;
        }

        self.samples_processed += 1;

        let (prev_srtt_secs, prev_rtt_abs_dev_secs) = (
            self.smoothed_rtt.map(|d| d.as_secs_f64()),
            self.rtt_abs_deviation.map(|d| d.as_secs_f64()),
        );

        // --- Outlier Clamping Logic ---
        // Only apply clamping if we have established estimates (e.g., after a few samples)
        // and if there are previous SRTT and Dev values to compare against.
        if self.samples_processed > 2 {
            let prev_srtt_secs = prev_srtt_secs.unwrap();
            let prev_rtt_abs_dev_secs = prev_rtt_abs_dev_secs.unwrap();

            // Calculate dynamic upper bound based on deviation
            let dev_based_upper_bound =
                prev_srtt_secs + OUTLIER_STDDEV_FACTOR * prev_rtt_abs_dev_secs;

            // Calculate relative upper bound based on SRTT itself
            let relative_upper_bound = prev_srtt_secs * MAX_RTT_RELATIVE_INCREASE_FACTOR;

            // Calculate absolute increase upper bound
            let absolute_increase_upper_bound = prev_srtt_secs + MAX_RTT_ABSOLUTE_INCREASE_SECS;

            // The actual upper bound is the minimum of these protective caps,
            // but ensure it's at least somewhat larger than prev_srtt_secs.
            // We want the *tightest reasonable cap*.
            let mut clamp_upper_bound = dev_based_upper_bound
                .min(relative_upper_bound)
                .min(absolute_increase_upper_bound);

            // Ensure the clamp bound is not overly restrictive, e.g., must allow some increase.
            // This also handles cases where prev_rtt_abs_dev_secs might be zero.
            clamp_upper_bound = clamp_upper_bound.max(prev_srtt_secs * 1.2); // Must allow at least 20% increase

            if rtt_sample_secs > clamp_upper_bound {
                trace!(
                    original_sample_ms = rtt_sample_secs * 1000.0,
                    clamped_to_ms = clamp_upper_bound * 1000.0,
                    prev_srtt_ms = prev_srtt_secs * 1000.0,
                    prev_dev_ms = prev_rtt_abs_dev_secs * 1000.0,
                    "RTT sample clamped as outlier."
                );
                rtt_sample_secs = clamp_upper_bound;
            }
        }

        let (current_srtt_secs, current_rtt_abs_dev_secs) =
            match (self.smoothed_rtt, self.rtt_abs_deviation) {
                (Some(prev_srtt_duration), Some(prev_rtt_abs_dev_duration)) => {
                    // We have previous estimates, update them.
                    let prev_srtt_secs = prev_srtt_duration.as_secs_f64();
                    let prev_rtt_abs_dev_secs = prev_rtt_abs_dev_duration.as_secs_f64();

                    // Calculate the absolute difference (error) between the new sample and the smoothed RTT.
                    let rtt_error_secs = (rtt_sample_secs - prev_srtt_secs).abs();

                    // Update smoothed RTT (SRTT in TCP terms):
                    // SRTT = (1 - alpha) * SRTT_prev + alpha * RTT_sample
                    let updated_srtt_secs =
                        (1.0 - RTT_EWMA_ALPHA) * prev_srtt_secs + RTT_EWMA_ALPHA * rtt_sample_secs;

                    // Update smoothed RTT absolute deviation (RTTVAR in TCP terms, though RTTVAR is a mean deviation):
                    // RTTVAR = (1 - beta) * RTTVAR_prev + beta * |RTT_sample - SRTT_prev|
                    let updated_rtt_abs_dev_secs = (1.0 - RTT_DEV_EWMA_BETA)
                        * prev_rtt_abs_dev_secs
                        + RTT_DEV_EWMA_BETA * rtt_error_secs;

                    (updated_srtt_secs, updated_rtt_abs_dev_secs)
                }
                _ => {
                    // This is the first RTT sample.
                    // Initialize SRTT to this sample.
                    let initial_srtt_secs = rtt_sample_secs;
                    // Initialize RTTVAR (deviation) to half of the first RTT sample (a common heuristic, e.g., TCP).
                    let initial_rtt_abs_dev_secs = rtt_sample_secs / 2.0;

                    (initial_srtt_secs, initial_rtt_abs_dev_secs)
                }
            };

        // Store the updated EWMA values as Durations.
        // Ensure values are non-negative before converting, as Duration::from_secs_f64 panics on negative.
        self.smoothed_rtt = Some(Duration::from_secs_f64(current_srtt_secs.max(0.0)));
        self.rtt_abs_deviation = Some(Duration::from_secs_f64(current_rtt_abs_dev_secs.max(0.0)));

        let final_rtt = self.smoothed_rtt.unwrap();

        // Jitter is often estimated as half of the RTT's mean absolute deviation,
        // assuming jitter is somewhat symmetrical between send and receive paths.
        let rtt_deviation_for_jitter_secs = self.rtt_abs_deviation.unwrap().as_secs_f64();
        let final_jitter = Duration::from_secs_f64((rtt_deviation_for_jitter_secs / 2.0).max(0.0));

        self.final_stats = FinalStats {
            rtt: final_rtt,
            jitter: final_jitter,
        };

        trace!(
            rtt = ?self.final_stats.rtt,
            jitter = ?self.final_stats.jitter,
            new_sample_ms = rtt_sample_secs * 1000.0,
            "RTT stats updated!"
        );
    }

    /// Returns the latest computed RTT and Jitter statistics.
    pub fn get_current_stats(&self) -> &FinalStats {
        &self.final_stats
    }

    /// Resets the estimator to its initial state (no samples).
    pub fn reset(&mut self) {
        self.smoothed_rtt = None;
        self.rtt_abs_deviation = None;
        self.final_stats = FinalStats::default();
        self.samples_processed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ewma_rtt_estimator_initialization() {
        let estimator = RttEstimatorEwma::new();
        assert_eq!(estimator.get_current_stats().rtt, Duration::ZERO);
        assert_eq!(estimator.get_current_stats().jitter, Duration::ZERO);
        assert!(estimator.smoothed_rtt.is_none());
        assert!(estimator.rtt_abs_deviation.is_none());
    }

    #[test]
    fn test_ewma_first_sample() {
        let mut estimator = RttEstimatorEwma::new();
        let sample1 = Duration::from_millis(100);
        estimator.update_with_new_sample(sample1);

        let stats = estimator.get_current_stats();
        assert_eq!(stats.rtt, Duration::from_millis(100)); // SRTT = sample1
        // Initial rtt_abs_deviation = sample1 / 2 = 50ms
        // Jitter = rtt_abs_deviation / 2 = 50ms / 2 = 25ms
        assert_eq!(stats.jitter, Duration::from_millis(25));

        assert_eq!(estimator.smoothed_rtt, Some(Duration::from_millis(100)));
        assert_eq!(estimator.rtt_abs_deviation, Some(Duration::from_millis(50)));
    }

    #[test]
    fn test_ewma_multiple_samples_stable() {
        // setup_logger();
        let mut estimator = RttEstimatorEwma::new();
        estimator.update_with_new_sample(Duration::from_millis(100)); // RTT: 100ms, Jitter: 25ms
        estimator.update_with_new_sample(Duration::from_millis(100)); // RTT: 100ms, Jitter: 18.75ms
        estimator.update_with_new_sample(Duration::from_millis(100)); // RTT: 100ms, Jitter: ~14.06ms

        // After 1st sample (100ms):
        // prev_srtt = 100ms, prev_abs_dev = 50ms
        // Stats: RTT = 100ms, Jitter = 25ms

        // After 2nd sample (100ms):
        // rtt_sample_secs = 0.1
        // prev_srtt_secs = 0.1, prev_rtt_abs_dev_secs = 0.05
        // rtt_error_secs = (0.1 - 0.1).abs() = 0.0
        // updated_srtt_secs = (1-0.125)*0.1 + 0.125*0.1 = 0.1
        // updated_rtt_abs_dev_secs = (1-0.25)*0.05 + 0.25*0.0 = 0.0375
        // Internal: srtt = 100ms, abs_dev = 37.5ms
        // Stats: RTT = 100ms, Jitter = 37.5ms / 2 = 18.75ms
        let stats1 = estimator.get_current_stats().clone(); // Clone to check after next update
        assert_eq!(stats1.rtt, Duration::from_millis(100));
        assert_eq!(stats1.jitter, Duration::from_secs_f64(0.0375 / 2.0)); // 18.75ms

        // After 3rd sample (100ms):
        // rtt_sample_secs = 0.1
        // prev_srtt_secs = 0.1, prev_rtt_abs_dev_secs = 0.0375
        // rtt_error_secs = (0.1 - 0.1).abs() = 0.0
        // updated_srtt_secs = 0.1
        // updated_rtt_abs_dev_secs = (1-0.25)*0.0375 + 0.25*0.0 = 0.75 * 0.0375 = 0.028125
        // Internal: srtt = 100ms, abs_dev = 28.125ms
        // Stats: RTT = 100ms, Jitter = 28.125ms / 2 = 14.0625ms
        estimator.update_with_new_sample(Duration::from_millis(100));
        let stats2 = estimator.get_current_stats();
        assert_eq!(stats2.rtt, Duration::from_millis(100));
        assert_eq!(stats2.jitter, Duration::from_secs_f64(0.028125 / 2.0)); // 14.0625ms
    }
}
