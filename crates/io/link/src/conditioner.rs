//! Receive-side packet conditioning for simulated network latency, jitter, and loss.

use bevy_reflect::Reflect;
use core::time::Duration;
use lightyear_core::time::Instant;
use lightyear_utils::ready_buffer::ReadyBuffer;
use rand::RngExt;

/// Configuration for the probability of network packet loss.
///
/// We use the [Gilbert–Elliott model] in order to simulate realistic network
/// behavior which is bursty in nature. The model keeps track of a hidden "good"
/// and "bad" state:
///
/// * If we are in a bad state, packet loss probability is high
/// * If we are in a good state, packet loss probability is low
/// * We switch between good and bad states based on probabilities unrelated to
///   whether or not a packet was lost.
///
/// # Choosing a constructor
///
/// Each constructor frees more of the four probabilities and can reproduce
/// one more kind of real-world loss pattern.
///
/// * [`fixed_loss_probability`](Self::fixed_loss_probability): The probability
///   of loss for every packet is the same. Use when only the overall rate
///   matters. At low rates it essentially never drops consecutive packets, so
///   it under-stresses anything tuned for burst tolerance (input redundancy,
///   interpolation buffers).
/// * [`simple_gilbert`](Self::simple_gilbert): Adds "bursts" which are
///   periods of complete packet loss separated by gaps where packet loss does
///   not occur. This is the default choice for simulating a real link; needs
///   only a loss rate and a mean burst length.
/// * [`gilbert`](Self::gilbert): Makes bursts leaky where only a fraction of a
///   burst's packets are lost instead of all of them. Pick when bursts
///   shouldn't be total blackouts (e.g. congestion that delays and thins
///   traffic rather than severing it).
/// * [`gilbert_elliott`](Self::gilbert_elliott): Adds a minimum packet-loss
///   probability to all packets even in the non-burst periods. This minimum
///   packet-loss probability is rarely distinguishable from short bursts in a
///   capture of real packet loss over any realistic length, so pick it only
///   when the minimum packet-loss probability is known.
///
/// [Gilbert–Elliott model]: https://en.wikipedia.org/wiki/Burst_error#Gilbert%E2%80%93Elliott_model
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect)]
pub struct PacketLossConfig {
    /// Probability that a packet is lost while in the good state (`0.0..=1.0`).
    pub good_loss: f32,

    /// Probability that a packet is lost while in the bad state (`0.0..=1.0`).
    pub bad_loss: f32,

    /// Probability of going from the good state to the bad state after each
    /// packet (`0.0..=1.0`).
    pub good_to_bad: f32,

    /// Probability of going from the bad state to the good state after each
    /// packet (`0.0..=1.0`).
    pub bad_to_good: f32,
}

impl PacketLossConfig {
    /// Returns a [`PacketLossConfig`] that makes the probability of a packet
    /// loss set to a fixed `loss_probability`.
    ///
    /// Every packet drops independently of its neighbors: pick this when
    /// only the overall rate matters, and a bursty form when consecutive-loss
    /// behavior does.
    pub fn fixed_loss_probability(loss_probability: f32) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&loss_probability),
            "Loss probability must be in 0.0..=1.0, got {loss_probability}"
        );
        PacketLossConfig {
            good_loss: loss_probability,
            bad_loss: loss_probability,
            good_to_bad: 0.0,
            bad_to_good: 0.0,
        }
    }

    /// Returns the Simple Gilbert model (Hasslinger & Hohlfeld, MMB 2008,
    /// Table 2). This adds "bursts" which are periods of complete packet loss
    /// separated by gaps where packet loss does not occur.
    ///
    /// Pick this when all you know about the link is how much it loses and
    /// how long the outages run. This is the usual case when hand-authoring a
    /// preset.
    ///
    /// `mean_loss` is the fraction of total packets lost and must be in `[0,
    /// 1)`. A loss rate of 1.0 means every packet is lost, i.e. an infinitely
    /// long burst, which this model cannot represent. Use
    /// [`fixed_loss_probability`](Self::fixed_loss_probability) for a
    /// fully-lossy link.
    ///
    /// `mean_burst_len` is the average number of consecutive packets in a
    /// burst, and must be `>= 1.0`. Because a burst here loses every packet,
    /// an observed run of consecutive losses *is* the whole burst.
    ///
    /// Equivalent to [`gilbert`](Self::gilbert) with `burst_loss = 1.0`.
    pub fn simple_gilbert(mean_loss: f32, mean_burst_len: f32) -> Self {
        Self::gilbert_elliott(mean_loss, mean_burst_len, 1.0, 0.0)
    }

    /// Returns a Gilbert model (Gilbert, BSTJ 39(5), 1960). It is similar to
    /// [`simple_gilbert`](Self::simple_gilbert) but bursts are now
    /// leaky where only a fraction of a burst's packets are lost instead
    /// of all of them.
    ///
    /// Pick this when bursts should thin traffic rather than sever it.
    ///
    /// `mean_loss` is the fraction of total packets lost and must be in
    /// `[0, burst_loss)`.
    ///
    /// `mean_burst_len` is the average number of consecutive packets spent in
    /// the burst (`>= 1.0`). Packets are not always lost in a burst now so an
    /// *observed* count of consecutive packet losses may be shorter than the
    /// burst that they are a part of.
    ///
    /// `burst_loss` is the probability that a packet will be lost during a
    /// burst.
    ///
    /// Equivalent to [`gilbert_elliott`](Self::gilbert_elliott) with
    /// `min_loss = 0.0`.
    pub fn gilbert(mean_loss: f32, mean_burst_len: f32, burst_loss: f32) -> Self {
        Self::gilbert_elliott(mean_loss, mean_burst_len, burst_loss, 0.0)
    }

    /// Returns a full Gilbert–Elliott model (Elliott, BSTJ 42(5), 1963). It is
    /// similar to [`gilbert`](Self::gilbert) but now there is a minimum
    /// packet-loss probability, `min_loss`, even during non-burst
    /// periods.
    ///
    /// Pick this only when the minimum packet loss probability is known
    /// independently of the bursts. A capture of real network packet loss
    /// over any realistic length rarely distinguishes a floor from frequent
    /// short bursts.
    ///
    /// `mean_loss` is the fraction of total packets lost and must be in
    /// `[min_loss, burst_loss)`. `mean_loss` is a mixture of the
    /// two state densities, so it can't be outside them.
    ///
    /// `mean_burst_len` is the average number of consecutive packets in a burst
    /// (`>= 1.0`).
    ///
    /// `burst_loss` is the probability that a packet will be lost during a
    /// burst.
    pub fn gilbert_elliott(
        mean_loss: f32,
        mean_burst_len: f32,
        burst_loss: f32,
        min_loss: f32,
    ) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&min_loss) && (0.0..=1.0).contains(&burst_loss),
            "Loss probabilities must be in 0.0..=1.0, got min_loss {min_loss}, burst_loss {burst_loss}"
        );
        debug_assert!(
            min_loss <= mean_loss && mean_loss < burst_loss,
            "Mean loss must be in range [min_loss, burst_loss) = [{min_loss}, {burst_loss}), got {mean_loss}"
        );
        debug_assert!(
            mean_burst_len >= 1.0,
            "Mean burst length must be >= 1.0 (a burst is at least one packet), got {mean_burst_len}"
        );

        // A bad-state visit is a run of consecutive packets in the bad state, and
        // that run length is geometrically distributed with mean `1.0 / bad_to_good`
        // (Hasslinger & Hohlfeld, MMB 2008, Sec. 3: their `r = 1/ABEL`, where r is
        // bad_to_good and ABEL is the average burst length).
        let bad_to_good = 1.0 / mean_burst_len;

        // Total loss rate is each state's loss density weighted by its stationary
        // probability: `mean_loss = (1 - stationary_bad) * good_loss +
        // stationary_bad * bad_loss` (Hasslinger & Hohlfeld, MMB 2008, Eq. 2),
        // solved here for the stationary bad-state probability...
        let stationary_bad = (mean_loss - min_loss) / (burst_loss - min_loss);

        // ...which equals `good_to_bad / (good_to_bad + bad_to_good)`, inverted for
        // the remaining transition probability.
        let good_to_bad = stationary_bad * bad_to_good / (1.0 - stationary_bad);

        PacketLossConfig {
            good_loss: min_loss,
            bad_loss: burst_loss,
            good_to_bad,
            bad_to_good,
        }
    }

    /// Returns the fraction of total packets this model drops.
    pub fn mean_loss(&self) -> f32 {
        let transition = self.good_to_bad + self.bad_to_good;
        if transition <= 0.0 {
            // No transitions means we never leave the good state.
            return self.good_loss;
        }

        // The total loss rate (`mean_loss`) is each state's loss probability weighted
        // by how often we're in that state. `stationary_bad` is the fraction of
        // packets that land in the bad state, good_to_bad / (good_to_bad +
        // bad_to_good); the good state covers the remaining `1 -
        // stationary_bad`. (Gilbert-Elliott: Elliott, BSTJ 42(5), 1963;
        // Hasslinger & Hohlfeld, MMB 2008, Eq. 2.)
        let stationary_bad = self.good_to_bad / transition;
        (1.0 - stationary_bad) * self.good_loss + stationary_bad * self.bad_loss
    }
}

/// The possible states of the Gilbert–Elliott model. See [`PacketLossConfig`]
/// for more info.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum PacketLossState {
    /// The good state. Packet loss has a probability of
    /// [`PacketLossConfig::good_loss`].
    #[default]
    Good,

    /// The bad state. Packet loss has a probability of
    /// [`PacketLossConfig::bad_loss`].
    Bad,
}

/// Configuration for receive-side packet conditioning.
///
/// The values describe the local inbound path: a payload inserted into a
/// [`LinkConditioner`] can be delayed by [`incoming_latency`](Self::incoming_latency),
/// randomly shifted by [`incoming_jitter`](Self::incoming_jitter), or dropped according to
/// [`incoming_loss`](Self::incoming_loss).
///
/// When modeling a full round trip, use one conditioner on
/// each peer or call [`half`](Self::half) to derive an approximate one-way configuration from an
/// end-to-end configuration.
#[derive(Clone, Debug, Default, Reflect)]
pub struct LinkConditionerConfig {
    /// Base delay applied to incoming payloads.
    ///
    /// The conditioner currently schedules packets with millisecond precision by using
    /// [`Duration::as_millis`]. For symmetric client/server simulations this is usually configured
    /// as half of the desired round-trip time.
    pub incoming_latency: Duration,

    /// Maximum random delay variation applied around [`incoming_latency`](Self::incoming_latency).
    ///
    /// For each payload, a random millisecond offset in `[-incoming_jitter, incoming_jitter)` is
    /// added to the base latency. Negative results are clamped by scheduling the packet at the
    /// current instant rather than in the past.
    pub incoming_jitter: Duration,

    /// Configuration for the probability that an incoming packet is lost.
    pub incoming_loss: PacketLossConfig,
}

/// Generic receive-side packet conditioner.
///
/// `LinkConditioner` delays and drops payloads according to a [`LinkConditionerConfig`].
#[derive(Debug, Clone)]
pub struct LinkConditioner<P: Eq> {
    config: LinkConditionerConfig,

    /// The current packet-loss state of the Gilbert–Elliott model as defined in [`PacketLossConfig`].
    packet_loss_state: PacketLossState,

    /// Payloads waiting for their simulated delivery time.
    ///
    /// The key is the delivery [`Instant`]. Once that instant is less than or equal to the instant
    /// passed to the polling code, the payload is ready to be moved into the receive buffer.
    ///
    /// This field is public for tests and low-level integrations, but normal link users should
    /// interact through [`crate::LinkReceiver`].
    pub time_queue: ReadyBuffer<Instant, P>,
}

impl<P: Eq> LinkConditioner<P> {
    /// Creates an empty conditioner using `config`.
    pub fn new(config: LinkConditionerConfig) -> Self {
        LinkConditioner {
            config,
            packet_loss_state: PacketLossState::default(),
            time_queue: ReadyBuffer::new(),
        }
    }

    /// Applies latency, jitter, and loss to `packet` relative to `instant`.
    ///
    /// Dropped packets are discarded immediately. Delivered packets are queued by their simulated
    /// delivery instant.
    pub(crate) fn condition_packet(&mut self, packet: P, instant: Instant) {
        let mut rng = rand::rng();

        // Execute the Gilbert–Elliott model to decide packet loss. See
        // `PacketLossConfig` for details.
        let loss = &self.config.incoming_loss;
        let (packet_loss_probability, state_transition_probability) = match self.packet_loss_state {
            PacketLossState::Good => (loss.good_loss, loss.good_to_bad),
            PacketLossState::Bad => (loss.bad_loss, loss.bad_to_good),
        };
        if rng.random_range(0.0..1.0) < state_transition_probability {
            self.packet_loss_state = match self.packet_loss_state {
                PacketLossState::Good => PacketLossState::Bad,
                PacketLossState::Bad => PacketLossState::Good,
            };
        }
        if rng.random_range(0.0..1.0) < packet_loss_probability {
            // Packet lost.
            return;
        }

        let mut latency: i32 = self.config.incoming_latency.as_millis() as i32;
        let mut packet_timestamp = instant;
        if self.config.incoming_jitter > Duration::default() {
            let jitter: i32 = self.config.incoming_jitter.as_millis() as i32;
            latency += rng.random_range(-jitter..jitter);
        }
        if latency > 0 {
            packet_timestamp += Duration::from_millis(latency as u64);
        }
        self.time_queue.push(packet_timestamp, packet);
    }

    /// Returns the next packet whose delivery instant has elapsed.
    pub(crate) fn pop_packet(&mut self, instant: Instant) -> Option<P> {
        self.time_queue.pop_item(&instant).map(|(_, packet)| packet)
    }
}

impl LinkConditionerConfig {
    /// Creates a configuration with explicit latency, jitter, and loss values.
    ///
    /// `incoming_latency` is the base receive delay, `incoming_jitter` is the
    /// maximum random millisecond offset applied around that delay, and
    /// `incoming_loss` is configuration for determining the probability that a
    /// given packet is lost.
    pub fn new(
        incoming_latency: Duration,
        incoming_jitter: Duration,
        incoming_loss: PacketLossConfig,
    ) -> Self {
        LinkConditionerConfig {
            incoming_latency,
            incoming_jitter,
            incoming_loss,
        }
    }

    /// Returns an approximate one-way half of this configuration.
    ///
    /// This divides latency, jitter, and loss probabilities by two.
    pub fn half(self) -> Self {
        LinkConditionerConfig {
            incoming_latency: self.incoming_latency / 2,
            incoming_jitter: self.incoming_jitter / 2,
            incoming_loss: PacketLossConfig {
                good_loss: self.incoming_loss.good_loss / 2.0,
                bad_loss: self.incoming_loss.bad_loss / 2.0,
                // Don't halve the state-transition probabilities. It would
                // only stretch the timescale of the bursts of packet loss.
                ..self.incoming_loss
            },
        }
    }

    /// Returns a preset for a low-latency, low-loss connection.
    pub fn good_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(40),
            incoming_jitter: Duration::from_millis(6),
            incoming_loss: PacketLossConfig::fixed_loss_probability(0.002),
        }
    }

    /// Returns a preset for a typical moderate-latency connection.
    pub fn average_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(100),
            incoming_jitter: Duration::from_millis(15),
            incoming_loss: PacketLossConfig::fixed_loss_probability(0.02),
        }
    }

    /// Returns a preset for a high-latency, lossy connection.
    pub fn poor_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(200),
            incoming_jitter: Duration::from_millis(30),
            incoming_loss: PacketLossConfig::fixed_loss_probability(0.10),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn fixed_loss_probability_is_state_independent() {
        let config = PacketLossConfig::fixed_loss_probability(0.3);
        // Both `PacketLossState` states lose packets at the same rate, so the mean loss
        // is exactly the rate regardless of which state the chain sits in.
        assert_relative_eq!(config.mean_loss(), 0.3, epsilon = 1e-4);
        assert_eq!(config.good_loss, config.bad_loss);
    }

    #[test]
    fn default_is_lossless() {
        assert_eq!(PacketLossConfig::default().mean_loss(), 0.0);
    }

    #[test]
    fn simple_gilbert_hits_target_mean_loss() {
        for &(mean_loss, mean_burst_len) in
            &[(0.002_f32, 3.0_f32), (0.02, 4.0), (0.10, 6.0), (0.5, 10.0)]
        {
            let config = PacketLossConfig::simple_gilbert(mean_loss, mean_burst_len);
            assert_relative_eq!(config.mean_loss(), mean_loss, epsilon = 1e-4);
            // The `PacketLossState::Bad` state must persist long enough to average
            // `mean_burst_len` drops.
            assert_relative_eq!(1.0 / config.bad_to_good, mean_burst_len, epsilon = 1e-4);
        }
    }

    #[test]
    fn gilbert_hits_target_mean_loss() {
        for &(mean_loss, mean_burst_len, burst_loss) in &[
            (0.002_f32, 3.0_f32, 0.5_f32),
            (0.02, 4.0, 0.8),
            (0.10, 6.0, 0.3),
        ] {
            let config = PacketLossConfig::gilbert(mean_loss, mean_burst_len, burst_loss);
            assert_relative_eq!(config.mean_loss(), mean_loss, epsilon = 1e-4);
            assert_relative_eq!(1.0 / config.bad_to_good, mean_burst_len, epsilon = 1e-4);
            assert_eq!(config.good_loss, 0.0);
            assert_eq!(config.bad_loss, burst_loss);
        }
    }

    #[test]
    fn gilbert_elliott_hits_target_mean_loss() {
        for &(mean_loss, mean_burst_len, burst_loss, min_loss) in &[
            (0.01_f32, 3.0_f32, 0.5_f32, 0.001_f32),
            (0.05, 8.0, 0.8, 0.01),
            (0.10, 6.0, 0.3, 0.02),
        ] {
            let config =
                PacketLossConfig::gilbert_elliott(mean_loss, mean_burst_len, burst_loss, min_loss);
            assert_relative_eq!(config.mean_loss(), mean_loss, epsilon = 1e-4);
            assert_relative_eq!(1.0 / config.bad_to_good, mean_burst_len, epsilon = 1e-4);
            assert_eq!(config.good_loss, min_loss);
            assert_eq!(config.bad_loss, burst_loss);
        }
    }

    #[test]
    fn constructor_ladder_reduces_downward() {
        // Each constructor with its extra knob pinned must produce exactly the
        // rung below it.
        let (mean_loss, mean_burst_len) = (0.02_f32, 4.0_f32);
        let simple = PacketLossConfig::simple_gilbert(mean_loss, mean_burst_len);
        assert_eq!(
            simple,
            PacketLossConfig::gilbert(mean_loss, mean_burst_len, 1.0)
        );
        assert_eq!(
            PacketLossConfig::gilbert(mean_loss, mean_burst_len, 0.5),
            PacketLossConfig::gilbert_elliott(mean_loss, mean_burst_len, 0.5, 0.0)
        );
    }
}
