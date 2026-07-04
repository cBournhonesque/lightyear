//! Receive-side packet conditioning for simulated network latency, jitter, and loss.

use bevy_reflect::Reflect;
use core::time::Duration;
use lightyear_core::time::Instant;
use lightyear_utils::ready_buffer::ReadyBuffer;
use rand::RngExt;

/// The simulated link's current condition. Used as the good/bad state in the
/// Gilbert–Elliott model for determining probability of packet loss (See
/// [`LinkConditionerConfig`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LinkConditionerState {
    /// The link is healthy. Usually means packets are not be getting lost or
    /// delayed.
    #[default]
    Good,

    /// The link is degraded. Usually entails packets getting lost or delayed.
    Bad,
}

/// Configuration for receive-side packet conditioning.
///
/// The values describe the local inbound path: a payload inserted into a
/// [`LinkConditioner`] can be delayed by [`incoming_latency`](Self::incoming_latency),
/// randomly shifted by [`incoming_jitter`](Self::incoming_jitter), or dropped
/// according to the loss probabilities.
///
/// Build a configuration from [`default`](Default::default) with the
/// `with_` methods:
///
/// ```
/// # use core::time::Duration;
/// # use lightyear_link::prelude::LinkConditionerConfig;
/// let config = LinkConditionerConfig::default()
///     .with_incoming_latency(Duration::from_millis(40))
///     .with_incoming_jitter(Duration::from_millis(8))
///     .with_simple_gilbert_loss(0.005, 8.0);
/// ```
///
/// When modeling a full round trip, use one conditioner on each peer or
/// call [`half`](Self::half) to derive an approximate one-way configuration
/// from an end-to-end configuration.
///
/// # How packet loss probability is modeled
///
/// The probability that a given packet is lost follows the [Gilbert–Elliott
/// model]. Consider the link being in either a
/// [`Good`](LinkConditionerState::Good) or [`Bad`](LinkConditionerState::Bad)
/// state. The probability that a packet is lost depends on which state the link
/// is in (see [`good_loss`](Self::good_loss) and [`bad_loss`](Self::bad_loss)).
/// After each packet is sent or lost, the link's state may change (see
/// [`good_to_bad`](Self::good_to_bad) and [`bad_to_good`](Self::bad_to_good)).
///
/// ## Choosing a loss model
///
/// The config can represent the full [Gilbert–Elliott model]. However, you
/// may not need the level of fidelity that it provides, so the config has
/// four different methods to set the packet-loss probability:
///
/// * [`with_fixed_loss`](Self::with_fixed_loss): The probability that a packet
///   is lost is the same for every packet. The conditioner does not consider if
///   the link is in a good or bad state. Use when only the overall packet-loss
///   rate matters.
/// * [`with_simple_gilbert_loss`](Self::with_simple_gilbert_loss): Packet loss
///   occurs in "bursts" which are periods of complete packet loss (the
///   [`Bad`](LinkConditionerState::Bad) state) separated by gaps where packet
///   loss does not occur (the [`Good`](LinkConditionerState::Good) state). This
///   is the default choice for simulating a real link as it is easier to create
///   and is similar enough to how real links behave.
/// * [`with_gilbert_loss`](Self::with_gilbert_loss): Bursts are now "leaky"
///   where only some of a burst's packets are lost instead of all of them. Pick
///   when bursts shouldn't be total blackouts (e.g. you want to simulate
///   network congestion that delays and thins traffic rather than severing it).
/// * [`with_gilbert_elliott_loss`](Self::with_gilbert_elliott_loss): Bursts are
///   leaky and now there is a minimum packet-loss probability to all packets
///   even in the non-burst periods (the [`Good`](LinkConditionerState::Good)
///   state). This minimum packet-loss probability is rarely distinguishable
///   from short bursts in a capture of real packet loss over any realistic
///   length, so pick it only when the minimum packet-loss probability is known.
///
/// [Gilbert–Elliott model]: https://en.wikipedia.org/wiki/Burst_error#Gilbert%E2%80%93Elliott_model
#[derive(Clone, Debug, Default, PartialEq, Reflect)]
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

    /// Probability that a packet is lost while in
    /// [`Good`](LinkConditionerState::Good) (`0.0..=1.0`).
    pub good_loss: f32,

    /// Probability that a packet is lost while in
    /// [`Bad`](LinkConditionerState::Bad) (`0.0..=1.0`).
    pub bad_loss: f32,

    /// Probability of going from [`Good`](LinkConditionerState::Good) to
    /// [`Bad`](LinkConditionerState::Bad) after each packet (`0.0..=1.0`).
    pub good_to_bad: f32,

    /// Probability of going from [`Bad`](LinkConditionerState::Bad) to
    /// [`Good`](LinkConditionerState::Good) after each packet (`0.0..=1.0`).
    pub bad_to_good: f32,
}

/// Generic receive-side packet conditioner.
///
/// `LinkConditioner` delays and drops payloads according to a [`LinkConditionerConfig`].
#[derive(Debug, Clone)]
pub struct LinkConditioner<P: Eq> {
    config: LinkConditionerConfig,

    /// The simulated link's current Gilbert–Elliott condition, stepped once
    /// per packet by the chain probabilities in [`LinkConditionerConfig`].
    state: LinkConditionerState,

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
            state: LinkConditionerState::default(),
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
        // `LinkConditionerConfig` for details.
        let config = &self.config;
        let (packet_loss_probability, state_transition_probability) = match self.state {
            LinkConditionerState::Good => (config.good_loss, config.good_to_bad),
            LinkConditionerState::Bad => (config.bad_loss, config.bad_to_good),
        };
        if rng.random_range(0.0..1.0) < state_transition_probability {
            self.state = match self.state {
                LinkConditionerState::Good => LinkConditionerState::Bad,
                LinkConditionerState::Bad => LinkConditionerState::Good,
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
    /// Replaces [`incoming_latency`](Self::incoming_latency).
    #[must_use]
    pub fn with_incoming_latency(mut self, incoming_latency: Duration) -> Self {
        self.incoming_latency = incoming_latency;
        self
    }

    /// Replaces [`incoming_jitter`](Self::incoming_jitter).
    #[must_use]
    pub fn with_incoming_jitter(mut self, incoming_jitter: Duration) -> Self {
        self.incoming_jitter = incoming_jitter;
        self
    }

    /// Replaces the packet-loss probability model with a fixed loss
    /// probability. Every packet is lost independently with the probability
    /// `loss_probability`.
    ///
    /// Pick this when only the overall loss rate matters, and a bursty form
    /// when consecutive-loss behavior does.
    #[must_use]
    pub fn with_fixed_loss(mut self, loss_probability: f32) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&loss_probability),
            "Loss probability must be in 0.0..=1.0, got {loss_probability}"
        );
        self.good_loss = loss_probability;
        self.bad_loss = loss_probability;
        self.good_to_bad = 0.0;
        self.bad_to_good = 0.0;
        self
    }

    /// Replaces the packet-loss probability model with the Simple Gilbert model
    /// (Hasslinger & Hohlfeld, MMB 2008, Table 2). Packet loss occurs in
    /// "bursts", periods of complete packet loss. No packets are lost
    /// during non-burst periods.
    ///
    /// Pick this when all you know about the link is how much it loses and
    /// how long the outages run. This is the usual case when hand-authoring a
    /// preset.
    ///
    /// `mean_loss` is the fraction of total packets lost and must be in `[0,
    /// 1)`. A loss rate of 1.0 means every packet is lost, i.e. an infinitely
    /// long burst, which this model cannot represent. Use
    /// [`with_fixed_loss`](Self::with_fixed_loss) for a
    /// fully-lossy link.
    ///
    /// `mean_bad_len` is the average number of consecutive packets spent in
    /// [`Bad`](LinkConditionerState::Bad) per visit, and must be `>= 1.0`.
    /// Because the bad state here loses every packet, a bad-state visit *is*
    /// an observed burst of consecutive losses.
    ///
    /// Equivalent to [`with_gilbert_loss`](Self::with_gilbert_loss) with
    /// `bad_loss = 1.0`.
    #[must_use]
    pub fn with_simple_gilbert_loss(self, mean_loss: f32, mean_bad_len: f32) -> Self {
        self.with_gilbert_elliott_loss(mean_loss, mean_bad_len, 1.0, 0.0)
    }

    /// Replaces the packet-loss probability model with a Gilbert model
    /// (Gilbert, BSTJ 39(5), 1960). It is similar to
    /// [`with_simple_gilbert_loss`](Self::with_simple_gilbert_loss) but
    /// bursts are now "leaky" where only a fraction of a burst's packets are
    /// lost instead of all of them.
    ///
    /// Pick this when bursts should thin traffic rather than sever it.
    ///
    /// `mean_loss` is the fraction of total packets lost and must be in
    /// `[0, bad_loss)`.
    ///
    /// `mean_bad_len` is the average number of consecutive packets spent in
    /// [`Bad`](LinkConditionerState::Bad) per visit (`>= 1.0`). Packets are
    /// not always lost in the bad state now, so an *observed* run of
    /// consecutive losses may be shorter than the bad-state visit that
    /// contains it.
    ///
    /// `bad_loss` is the probability that a packet is lost while the chain
    /// is in [`Bad`](LinkConditionerState::Bad).
    ///
    /// Equivalent to
    /// [`with_gilbert_elliott_loss`](Self::with_gilbert_elliott_loss) with
    /// `min_loss = 0.0`.
    #[must_use]
    pub fn with_gilbert_loss(self, mean_loss: f32, mean_bad_len: f32, bad_loss: f32) -> Self {
        self.with_gilbert_elliott_loss(mean_loss, mean_bad_len, bad_loss, 0.0)
    }

    /// Replaces the packet-loss probability model with a full Gilbert–Elliott
    /// model (Elliott, BSTJ 42(5), 1963). It is similar to
    /// [`with_gilbert_loss`](Self::with_gilbert_loss) but now there is a
    /// minimum packet-loss probability, `min_loss`, even while the chain is
    /// in [`Good`](LinkConditionerState::Good).
    ///
    /// Pick this only when the minimum packet loss probability is known
    /// independently of the bursts. A capture of real network packet loss
    /// over any realistic length rarely distinguishes a floor from frequent
    /// short bursts.
    ///
    /// `mean_loss` is the fraction of total packets lost and must be in
    /// `[min_loss, bad_loss)`. `mean_loss` is a mixture of the
    /// two state densities, so it can't be outside them.
    ///
    /// `mean_bad_len` is the average number of consecutive packets spent in
    /// [`Bad`](LinkConditionerState::Bad) per visit (`>= 1.0`).
    ///
    /// `bad_loss` is the probability that a packet is lost while the chain
    /// is in [`Bad`](LinkConditionerState::Bad).
    #[must_use]
    pub fn with_gilbert_elliott_loss(
        mut self,
        mean_loss: f32,
        mean_bad_len: f32,
        bad_loss: f32,
        min_loss: f32,
    ) -> Self {
        debug_assert!(
            (0.0..=1.0).contains(&min_loss) && (0.0..=1.0).contains(&bad_loss),
            "Loss probabilities must be in 0.0..=1.0, got min_loss {min_loss}, bad_loss {bad_loss}"
        );
        debug_assert!(
            min_loss <= mean_loss && mean_loss < bad_loss,
            "Mean loss must be in range [min_loss, bad_loss) = [{min_loss}, {bad_loss}), got {mean_loss}"
        );
        debug_assert!(
            mean_bad_len >= 1.0,
            "Mean bad-state visit length must be >= 1.0 (a visit is at least one packet), got {mean_bad_len}"
        );

        // A bad-state visit is a run of consecutive packets in the bad state, and
        // that run length is geometrically distributed with mean `1.0 / bad_to_good`
        // (Hasslinger & Hohlfeld, MMB 2008, Sec. 3: their `r = 1/ABEL`, where r is
        // bad_to_good and ABEL is the average burst length).
        let bad_to_good = 1.0 / mean_bad_len;

        // Total loss rate is each state's loss density weighted by its stationary
        // probability: `mean_loss = (1 - stationary_bad) * good_loss +
        // stationary_bad * bad_loss` (Hasslinger & Hohlfeld, MMB 2008, Eq. 2),
        // solved here for the stationary bad-state probability.
        let stationary_bad = (mean_loss - min_loss) / (bad_loss - min_loss);

        // `stationary_bad = good_to_bad / (good_to_bad + bad_to_good)`.
        let good_to_bad = stationary_bad * bad_to_good / (1.0 - stationary_bad);

        self.good_loss = min_loss;
        self.bad_loss = bad_loss;
        self.good_to_bad = good_to_bad;
        self.bad_to_good = bad_to_good;
        self
    }

    /// Returns the fraction of total packets this configuration drops.
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

    /// Returns an approximate one-way half of this configuration.
    ///
    /// This divides latency, jitter, and loss probabilities by two.
    pub fn half(self) -> Self {
        LinkConditionerConfig {
            incoming_latency: self.incoming_latency / 2,
            incoming_jitter: self.incoming_jitter / 2,
            good_loss: self.good_loss / 2.0,
            bad_loss: self.bad_loss / 2.0,
            // Don't halve the state-transition probabilities. It would
            // only stretch the timescale of the bursts of packet loss.
            ..self
        }
    }

    /// Returns a preset for a low-latency, low-loss connection.
    pub fn good_condition() -> Self {
        Self::default()
            .with_incoming_latency(Duration::from_millis(40))
            .with_incoming_jitter(Duration::from_millis(6))
            .with_fixed_loss(0.002)
    }

    /// Returns a preset for a typical moderate-latency connection.
    pub fn average_condition() -> Self {
        Self::default()
            .with_incoming_latency(Duration::from_millis(100))
            .with_incoming_jitter(Duration::from_millis(15))
            .with_fixed_loss(0.02)
    }

    /// Returns a preset for a high-latency, lossy connection.
    pub fn poor_condition() -> Self {
        Self::default()
            .with_incoming_latency(Duration::from_millis(200))
            .with_incoming_jitter(Duration::from_millis(30))
            .with_fixed_loss(0.10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn fixed_loss_probability_is_state_independent() {
        let config = LinkConditionerConfig::default().with_fixed_loss(0.3);
        // Both `LinkConditionerState` states lose packets at the same rate, so the mean loss
        // is exactly the rate regardless of which state the chain sits in.
        assert_relative_eq!(config.mean_loss(), 0.3, epsilon = 1e-4);
        assert_eq!(config.good_loss, config.bad_loss);
    }

    #[test]
    fn default_is_lossless() {
        assert_eq!(LinkConditionerConfig::default().mean_loss(), 0.0);
    }

    #[test]
    fn simple_gilbert_hits_target_mean_loss() {
        for &(mean_loss, mean_bad_len) in
            &[(0.002_f32, 3.0_f32), (0.02, 4.0), (0.10, 6.0), (0.5, 10.0)]
        {
            let config =
                LinkConditionerConfig::default().with_simple_gilbert_loss(mean_loss, mean_bad_len);
            assert_relative_eq!(config.mean_loss(), mean_loss, epsilon = 1e-4);
            // The `LinkConditionerState::Bad` state must persist long enough to average
            // `mean_bad_len` drops.
            assert_relative_eq!(1.0 / config.bad_to_good, mean_bad_len, epsilon = 1e-4);
        }
    }

    #[test]
    fn gilbert_hits_target_mean_loss() {
        for &(mean_loss, mean_bad_len, bad_loss) in &[
            (0.002_f32, 3.0_f32, 0.5_f32),
            (0.02, 4.0, 0.8),
            (0.10, 6.0, 0.3),
        ] {
            let config = LinkConditionerConfig::default().with_gilbert_loss(
                mean_loss,
                mean_bad_len,
                bad_loss,
            );
            assert_relative_eq!(config.mean_loss(), mean_loss, epsilon = 1e-4);
            assert_relative_eq!(1.0 / config.bad_to_good, mean_bad_len, epsilon = 1e-4);
            assert_eq!(config.good_loss, 0.0);
            assert_eq!(config.bad_loss, bad_loss);
        }
    }

    #[test]
    fn gilbert_elliott_hits_target_mean_loss() {
        for &(mean_loss, mean_bad_len, bad_loss, min_loss) in &[
            (0.01_f32, 3.0_f32, 0.5_f32, 0.001_f32),
            (0.05, 8.0, 0.8, 0.01),
            (0.10, 6.0, 0.3, 0.02),
        ] {
            let config = LinkConditionerConfig::default().with_gilbert_elliott_loss(
                mean_loss,
                mean_bad_len,
                bad_loss,
                min_loss,
            );
            assert_relative_eq!(config.mean_loss(), mean_loss, epsilon = 1e-4);
            assert_relative_eq!(1.0 / config.bad_to_good, mean_bad_len, epsilon = 1e-4);
            assert_eq!(config.good_loss, min_loss);
            assert_eq!(config.bad_loss, bad_loss);
        }
    }

    #[test]
    fn constructor_ladder_reduces_downward() {
        // Each constructor with its extra knob pinned must produce exactly the
        // rung below it.
        let (mean_loss, mean_bad_len) = (0.02_f32, 4.0_f32);
        let base = LinkConditionerConfig::default;
        let simple = base().with_simple_gilbert_loss(mean_loss, mean_bad_len);
        assert_eq!(
            simple,
            base().with_gilbert_loss(mean_loss, mean_bad_len, 1.0)
        );
        assert_eq!(
            base().with_gilbert_loss(mean_loss, mean_bad_len, 0.5),
            base().with_gilbert_elliott_loss(mean_loss, mean_bad_len, 0.5, 0.0)
        );
    }
}
