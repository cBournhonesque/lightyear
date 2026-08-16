use core::time::Duration;

use lightyear_link::LinkStats;

/// Controls when an unacknowledged packet is presumed lost.
///
/// This is independent from reliable-channel resend timing. Packet NACK timing
/// classifies packet delivery and releases packet-level acknowledgement
/// bookkeeping; [`ReliableSettings`](crate::prelude::ReliableSettings) controls
/// when reliable messages become eligible for retransmission.
///
/// The timeout is `RTT * rtt_multiplier + jitter * jitter_multiplier`, clamped
/// to the inclusive range from `minimum_timeout` to `maximum_timeout`. The
/// default preserves the existing `clamp(1.5 * RTT, 10 ms, 3 s)` policy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PacketNackSettings {
    /// Multiplier applied to the current RTT estimate.
    pub rtt_multiplier: f32,
    /// Multiplier applied to the current jitter estimate.
    pub jitter_multiplier: f32,
    /// Minimum time to retain acknowledgement bookkeeping for a sent packet.
    pub minimum_timeout: Duration,
    /// Maximum time to retain acknowledgement bookkeeping for a sent packet.
    pub maximum_timeout: Duration,
}

impl Default for PacketNackSettings {
    fn default() -> Self {
        Self {
            rtt_multiplier: 1.5,
            jitter_multiplier: 0.0,
            minimum_timeout: Duration::from_millis(10),
            maximum_timeout: Duration::from_secs(3),
        }
    }
}

impl PacketNackSettings {
    pub(crate) fn timeout(self, link_stats: &LinkStats) -> Duration {
        link_stats
            .rtt
            .mul_f32(self.rtt_multiplier)
            .saturating_add(link_stats.jitter.mul_f32(self.jitter_multiplier))
            .min(self.maximum_timeout)
            .max(self.minimum_timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_preserves_existing_timeout_policy() {
        let settings = PacketNackSettings::default();
        let stats = LinkStats {
            rtt: Duration::from_millis(40),
            jitter: Duration::from_millis(10),
        };

        assert_eq!(settings.timeout(&stats), Duration::from_millis(60));
    }

    #[test]
    fn timeout_includes_rtt_jitter_and_bounds() {
        let settings = PacketNackSettings {
            rtt_multiplier: 1.5,
            jitter_multiplier: 2.0,
            minimum_timeout: Duration::from_millis(20),
            maximum_timeout: Duration::from_millis(200),
        };

        assert_eq!(
            settings.timeout(&LinkStats {
                rtt: Duration::from_millis(40),
                jitter: Duration::from_millis(10),
            }),
            Duration::from_millis(80)
        );
        assert_eq!(
            settings.timeout(&LinkStats::default()),
            Duration::from_millis(20)
        );
        assert_eq!(
            settings.timeout(&LinkStats {
                rtt: Duration::from_secs(1),
                jitter: Duration::ZERO,
            }),
            Duration::from_millis(200)
        );
    }
}
