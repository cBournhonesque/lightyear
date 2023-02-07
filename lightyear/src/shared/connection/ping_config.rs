use std::{default::Default, time::Duration};

/// Contains Config properties which will be used by a Server or Client
#[derive(Clone, Debug)]
pub struct PingConfig {
    /// The duration to wait before sending a ping message to the remote host,
    /// in order to estimate RTT time
    pub ping_interval: Duration,
    /// The initial estimate for the RTT
    pub rtt_initial_estimate: Duration,
    /// The initial estimate for Jitter
    pub jitter_initial_estimate: Duration,
    /// Factor to smooth out estimate of RTT. A higher number will
    /// smooth out measurements, but at the cost of responsiveness
    pub rtt_smoothing_factor: f32,
}

impl PingConfig {
    /// Creates a new MonitorConfig, used to monitor statistics about the
    /// network
    pub fn new(
        ping_interval: Duration,
        rtt_initial_estimate: Duration,
        jitter_initial_estimate: Duration,
        rtt_smoothing_factor: f32,
    ) -> Self {
        PingConfig {
            ping_interval,
            rtt_initial_estimate,
            jitter_initial_estimate,
            rtt_smoothing_factor,
        }
    }
}

impl Default for PingConfig {
    fn default() -> Self {
        Self {
            ping_interval: Duration::from_secs(1),
            rtt_initial_estimate: Duration::from_millis(200),
            jitter_initial_estimate: Duration::from_millis(20),
            rtt_smoothing_factor: 0.1,
        }
    }
}
