//! Receive-side packet conditioning for simulated network latency, jitter, and loss.

use alloc::vec::Vec;
use bevy_reflect::Reflect;
use core::time::Duration;
use lightyear_core::time::Instant;
use lightyear_utils::ready_buffer::ReadyBuffer;
use rand::{RngExt, SeedableRng, rngs::Xoshiro256PlusPlus};
use tracing::debug;

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
    /// The conditioner schedules packets with microsecond precision by using
    /// [`Duration::as_micros`]. For symmetric client/server simulations this is usually configured
    /// as half of the desired round-trip time.
    pub incoming_latency: Duration,

    /// Maximum random delay variation applied around [`incoming_latency`](Self::incoming_latency).
    ///
    /// For each payload, a random microsecond offset in `[-incoming_jitter, incoming_jitter)` is
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

/// The resolved fate of one sent packet, as observed by the sender.
///
/// If you're trying to capture network data to feed into
/// [`LinkConditionerConfig::fit`], construct one `ResolvedPacket` every time a
/// packet gets acknowledged or presumed lost.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResolvedPacket {
    /// Transport-level sequence number. Consecutive per link.
    pub packet_id: u32,

    /// Measured round-trip time if the packet was acknowledged. Set to
    /// `None` if it was presumed lost.
    pub rtt: Option<Duration>,
}

/// Minimum number of observed loss runs (groups of one or more consecutive
/// lost packets) before [`fit`](LinkConditionerConfig::fit) will treat
/// clumped losses as real bursts rather than independent losses that
/// happened to land next to each other. With fewer runs,
/// [`fit`](LinkConditionerConfig::fit) falls back to
/// [`with_fixed_loss`](LinkConditionerConfig::with_fixed_loss).
const MIN_LOSS_RUNS: usize = 20;

/// Minimum number of pairs whose packets were both lost before
/// [`fit`](LinkConditionerConfig::fit) trusts its estimate of how often the
/// packet right after a lost packet is also lost (Gilbert's `P(1|1)`).
///
/// A "pair" is two packets with consecutive ids `n` and `n + 1`.
const MIN_GILBERT_PAIRS: usize = 50;

/// Minimum number of triples whose first and third packets were lost (ignoring
/// the second packet's fate) before [`fit`](LinkConditionerConfig::fit) trusts
/// its estimate of how often that middle packet is also lost (Gilbert's triple
/// ratio).
///
/// A "triple" is three packets with consecutive ids `n`, `n + 1`, and `n + 2`.
const MIN_GILBERT_TRIPLES: usize = 20;

/// Generic receive-side packet conditioner.
///
/// `LinkConditioner` delays and drops payloads according to a
/// [`LinkConditionerConfig`].
#[derive(Debug, Clone)]
pub struct LinkConditioner<P: Eq> {
    config: LinkConditionerConfig,

    /// Source of the loss, transition, and jitter draws. Owned by the
    /// conditioner so a seeded one (see [`with_seed`](Self::with_seed))
    /// produces reproducible packet outcomes.
    ///
    /// Xoshiro256++ chosen for its high performance and implements `Clone`.
    rng: Xoshiro256PlusPlus,

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
    /// Creates an empty conditioner using `config`, with entropy-seeded
    /// randomness.
    pub fn new(config: LinkConditionerConfig) -> Self {
        Self::with_seed_internal(config, Xoshiro256PlusPlus::from_rng(&mut rand::rng()))
    }

    /// Creates an empty conditioner using `config` whose randomness is derived
    /// from `seed`, so the packet outcomes are reproducible.
    pub fn with_seed(config: LinkConditionerConfig, seed: u64) -> Self {
        Self::with_seed_internal(config, Xoshiro256PlusPlus::seed_from_u64(seed))
    }

    fn with_seed_internal(config: LinkConditionerConfig, rng: Xoshiro256PlusPlus) -> Self {
        LinkConditioner {
            config,
            rng,
            state: LinkConditionerState::default(),
            time_queue: ReadyBuffer::new(),
        }
    }

    /// Applies latency, jitter, and loss to `packet` relative to `instant`.
    ///
    /// Dropped packets are discarded immediately and reported as `None`.
    /// Delivered packets are queued by their simulated delivery instant, which
    /// is returned.
    pub(crate) fn condition_packet(&mut self, packet: P, instant: Instant) -> Option<Instant> {
        // Execute the Gilbert–Elliott model to decide packet loss. See
        // `LinkConditionerConfig` for details.
        let config = &self.config;
        let (packet_loss_probability, state_transition_probability) = match self.state {
            LinkConditionerState::Good => (config.good_loss, config.good_to_bad),
            LinkConditionerState::Bad => (config.bad_loss, config.bad_to_good),
        };
        if self.rng.random_range(0.0..1.0) < state_transition_probability {
            self.state = match self.state {
                LinkConditionerState::Good => LinkConditionerState::Bad,
                LinkConditionerState::Bad => LinkConditionerState::Good,
            };
        }
        if self.rng.random_range(0.0..1.0) < packet_loss_probability {
            // Packet lost.
            return None;
        }

        let mut delay_us: i64 = self.config.incoming_latency.as_micros() as i64;
        let mut packet_timestamp = instant;
        let jitter_us: i64 = self.config.incoming_jitter.as_micros() as i64;
        // Check against the integer and not the `Duration`. A sub-microsecond jitter
        // would otherwise reach random_range as the empty range 0..0,
        // which panics.
        if jitter_us > 0 {
            delay_us += self.rng.random_range(-jitter_us..jitter_us);
        }
        if delay_us > 0 {
            packet_timestamp += Duration::from_micros(delay_us as u64);
        }
        self.time_queue.push(packet_timestamp, packet);
        Some(packet_timestamp)
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

    /// Fits a **one-way** configuration to `sent_packets`, the recorded
    /// outcomes of packets sent over a single link. In other words, the
    /// returned [`LinkConditionerConfig`] will try to simulate the network
    /// behavior that resulted in the outcomes of `sent_packets`.
    ///
    /// `sent_packets` does not need to be ordered. Gaps in the packet ID
    /// sequence are packets still unresolved at capture end and are
    /// skipped. Lost packets at the start and end of `sent_packets` are
    /// ignored because they are lost due to link connection/disconnection
    /// reasons and not from the network behavior that this function is
    /// trying to fit to.
    ///
    /// `rtt_overhead` is the delay introduced by lightyear's own logic
    /// that is part of every packet's RTT:
    ///
    /// * The delay between lightyear receiving a packet and lightyear sending
    ///   the ack for it (the ack is not sent until the next scheduled outgoing
    ///   packet).
    /// * The delay between lightyear receiving an ack and lightyear actually
    ///   processing it.
    ///
    /// To simulate the captured link, install the returned configuration as
    /// the simulated *receiver*'s [`RecvLinkConditioner`](crate::RecvLinkConditioner):
    /// the capture observes packets sent from A to B, and a conditioner
    /// shapes *received* traffic, so the fitted configuration belongs on B's
    /// link.
    ///
    /// Returns `None` when `sent_packets` contains no acknowledged packets.
    pub fn fit(mut sent_packets: Vec<ResolvedPacket>, rtt_overhead: Duration) -> Option<Self> {
        sent_packets.sort_unstable_by_key(|outcome| outcome.packet_id);

        Some(
            Self::default()
                .fit_delay(&sent_packets, rtt_overhead)?
                .fit_loss(&sent_packets),
        )
    }

    /// Fits the one-way latency and jitter to the acknowledged packets of
    /// `sent_packets`, after deducting `rtt_overhead` (see [`fit`](Self::fit))
    /// from each RTT.
    ///
    /// Returns `None` if none of the packets in `sent_packets` were
    /// acknowledged. `sent_packets` does not need to be sorted.
    fn fit_delay(self, sent_packets: &[ResolvedPacket], rtt_overhead: Duration) -> Option<Self> {
        let mut rtts: Vec<Duration> = sent_packets
            .iter()
            .filter_map(|outcome| Some(outcome.rtt?.saturating_sub(rtt_overhead)))
            .collect();
        if rtts.is_empty() {
            // No packets were acknowledged.
            return None;
        }
        rtts.sort_unstable();

        // Returns the nth percentile RTT of `rtts`.
        //
        // `percentile(1, 2)` returns the median RTT.
        // `percentile(19, 20)` returns the 95th percentile RTT.
        //
        // Percentile defined as a ratio so the `rtts` index math is performed in
        // integers.
        let percentile =
            |numerator: usize, denominator: usize| rtts[(rtts.len() - 1) * numerator / denominator];

        // Half the median round trip approximates the one-way base delay.
        //
        // We use median rather than mean because real packet captures contain a few
        // extreme RTTs (acks that arrived hundreds of milliseconds late), and a
        // mean folds every one of them into the estimate, increasing the base delay
        // above what a typical packet experiences. The median only ranks values, so
        // a handful of stragglers cannot move it.
        let latency = percentile(1, 2) / 2;

        // The conditioner adds a random value from the range `[-jitter, jitter)`
        // to `latency` when deciding a packet's delay. To calculate jitter we:
        //
        // 1. Gather the 5th and 95th percentile RTTs to filter out extreme RTTs. That's
        //    the range of RTTs we want to simulate.
        // 2. Measure the width of that middle-90% band using `p95 − p5`.
        // 3. Halve it so the range reflects a one-way trip time.
        // 4. Divide by 1.8 to solve for `jitter`. The range `[-jitter, +jitter)` has a
        //    width of `2 * jitter`, and the conditioner takes values from it uniformly,
        //    so *its* middle-90% band has a width of `0.9 * 2 * jitter` or `1.8 *
        //    jitter`. Step 3 produced the width that `[-jitter, +jitter)` must match,
        //    so `1.8 * jitter = step 3`.
        //
        // `(1 / 2) * (1 / 1.8) = 5 / 18`
        let jitter = (percentile(19, 20) - percentile(1, 20)) * 5 / 18;

        Some(LinkConditionerConfig {
            incoming_latency: latency,
            incoming_jitter: jitter,
            ..self
        })
    }

    /// Fits the packet loss statistics onto the most realistic loss model the
    /// data (`sent_packets`) supports. The candidate models, from least to
    /// most demanding of data:
    ///
    /// 1. Lost: Used when `sent_packets` contains no acked packets.
    /// 2. Fixed loss probability: Used when `sample_packets` doesn't contain
    ///    enough runs (See [`MIN_LOSS_RUNS`]).
    /// 3. Simple Gilbert loss model: Used when `sent_packets` does not meet the
    ///    criterias defined in [`MIN_GILBERT_PAIRS`] and
    ///    [`MIN_GILBERT_TRIPLES`] *or* the bad-state isn't leaky.
    /// 4. Gilbert loss model: Used when `sent_packets` meets the criterias
    ///    defined in [`MIN_GILBERT_PAIRS`] and [`MIN_GILBERT_TRIPLES`] and the
    ///    bad-state is leaky.
    ///
    /// We don't try to fit the full Gilbert–Elliott model. Estimating its
    /// extra parameter, the packet-loss probability floor that gets applied
    /// even in the good state, requires an unrealistic amount of captured
    /// data, because a small floor is nearly indistinguishable from
    /// frequent, short bad-state visits. Callers who know the floor from an
    /// independent source can apply
    /// [`with_gilbert_elliott_loss`](Self::with_gilbert_elliott_loss) to the
    /// fitted configuration themselves.
    fn fit_loss(self, sent_packets: &[ResolvedPacket]) -> Self {
        // Trim the initial and last lost packets because their loss isn't the result of
        // network traffic and so they shouldn't be considered. The link is not in a
        // steady state while connecting and a disconnection triggers nacks for
        // every pending packet.
        let Some(first) = sent_packets
            .iter()
            .position(|outcome| outcome.rtt.is_some())
        else {
            // All packets were lost.
            debug!("All packets were lost; fitting total fixed loss");
            return self.with_fixed_loss(1.0);
        };
        let last = sent_packets
            .iter()
            .rposition(|outcome| outcome.rtt.is_some())
            .expect("Failed to find last packet despite finding a first packet");
        let sent_packets = &sent_packets[first..=last];

        // A "run" is a maximal stretch of lost packets with consecutive
        // packet ids, bounded on each side by a delivered packet or an id
        // gap (an unresolved packet).
        let mut run_count = 0usize;
        let mut in_run = false;
        let mut lost_count = 0usize;
        let mut prev_id: Option<u32> = None;
        for outcome in sent_packets {
            if prev_id.is_none_or(|id| id.wrapping_add(1) != outcome.packet_id) {
                in_run = false;
            }
            if outcome.rtt.is_none() {
                lost_count += 1;
                if !in_run {
                    run_count += 1;
                    in_run = true;
                }
            } else {
                in_run = false;
            }
            prev_id = Some(outcome.packet_id);
        }

        if lost_count == 0 {
            // No packets were lost so there's no probability of losing a packet.
            return self.with_fixed_loss(0.0);
        }
        let mean_loss = lost_count as f64 / sent_packets.len() as f64;
        if run_count < MIN_LOSS_RUNS {
            // With this few runs, clumped losses can't be told apart from independent
            // losses that landed next to each other by chance, so stick to a
            // fixed loss probability.
            debug!(
                run_count,
                MIN_LOSS_RUNS,
                "Too few loss runs to trust burst statistics; fitting a fixed loss probability"
            );
            return self.with_fixed_loss(mean_loss as f32);
        }

        // Number of pairs whose first packet was lost. The second packet may or may not
        // be lost.
        let mut pairs_1x_count = 0usize;

        // Number of pairs whose both packets were lost.
        let mut pairs_11_count = 0usize;

        // Collect statistics on "pairs". A "pair" is two packets with ids `n, n + 1`.
        for window in sent_packets.windows(2) {
            if window[0].packet_id.wrapping_add(1) != window[1].packet_id {
                continue;
            }
            if window[0].rtt.is_none() {
                pairs_1x_count += 1;
                if window[1].rtt.is_none() {
                    pairs_11_count += 1;
                }
            }
        }

        // Number of triples whose first and third packets were lost. The second packet
        // may or may not be lost.
        let mut triples_1x1_count = 0usize;

        // Number of triples whose packets were all lost.
        let mut triples_111_count = 0usize;

        // Collect statistics on "triples". A "triple" is three packets with ids `n, n +
        // 1, n + 2`.
        for window in sent_packets.windows(3) {
            if window[0].packet_id.wrapping_add(1) != window[1].packet_id
                || window[1].packet_id.wrapping_add(1) != window[2].packet_id
            {
                continue;
            }
            if window[0].rtt.is_none() && window[2].rtt.is_none() {
                triples_1x1_count += 1;
                if window[1].rtt.is_none() {
                    triples_111_count += 1;
                }
            }
        }

        if pairs_11_count >= MIN_GILBERT_PAIRS && triples_1x1_count >= MIN_GILBERT_TRIPLES {
            // There is enough data to fit the Gilbert model, which allows a leaky bad state
            // (a bad state that may deliver some packets instead of dropping them all). If
            // the bad state is leaky, the size of a run may be less than the stretch of
            // packets the chain spent in the bad state (a bad-state visit). If
            // the bad state weren't leaky (an assumption that the simple Gilbert model
            // makes), a run would be exactly a visit, so run sizes alone would determine
            // the visit length. Since leakiness can't be ruled out just yet, the visit
            // length and the bad-state loss probability are recovered from conditional
            // statistics rather than from the runs.

            // Gilbert's measured statistics (Hasslinger & Hohlfeld, MMB 2008,
            // Eq. 3).
            let a = mean_loss;
            let b = pairs_11_count as f64 / pairs_1x_count as f64;
            let c = triples_111_count as f64 / triples_1x1_count as f64;

            // Gilbert's closed-form solve (Hasslinger & Hohlfeld, MMB 2008,
            // Eq. 4). The paper solves for `h` (the bad state's *delivery* probability).
            // `bad_loss` is the complement, `1 - h = b / (1 - r)`.
            let one_minus_r = (a * c - b * b) / (2.0 * a * c - b * (a + c));
            let bad_loss = (b / one_minus_r) as f32;

            // Bad-state visits are geometrically distributed, so the mean
            // visit length is `1 / r` (Hasslinger & Hohlfeld, MMB 2008, Sec. 3:
            // `r = 1/ABEL`). The loss constructor turns it back into the chain
            // probabilities.
            let mean_bad_len = (1.0 / (1.0 - one_minus_r)) as f32;

            // Short or irregular traces are known to solve outside the valid
            // ranges (Gilbert notes this; the estimators are ratios of small
            // counts). Fall through to the burst-only rung when they do.
            if one_minus_r.is_finite()
                && mean_bad_len.is_finite()
                && mean_bad_len >= 1.0
                && bad_loss > mean_loss as f32
                && bad_loss <= 1.0
            {
                debug!(
                    mean_loss,
                    mean_bad_len, bad_loss, "Fitting a Gilbert loss model"
                );
                return self.with_gilbert_loss(mean_loss as f32, mean_bad_len, bad_loss);
            }
            debug!(
                one_minus_r,
                bad_loss,
                mean_bad_len,
                "The Gilbert solve fell outside valid probabilities; falling back to simple Gilbert"
            );
        } else {
            debug!(
                pairs_11_count,
                triples_1x1_count,
                "Too few samples for the Gilbert estimators; falling back to simple Gilbert"
            );
        }

        // At this point, we assume that the bad state is not leaky (all packets get
        // lost when in a bad state). In that case, an observed run *is* a bad-state
        // visit and its mean length is the parameter directly. Every lost packet
        // belongs to exactly one run, so the mean run length is the lost total spread
        // across the runs.
        let mean_run_len = lost_count as f64 / run_count as f64;
        debug!(
            mean_loss,
            mean_run_len, "Fitting a simple Gilbert loss model"
        );
        self.with_simple_gilbert_loss(mean_loss as f32, mean_run_len as f32)
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

    /// Returns a conditioner that simulates the link conditions from an
    /// ethernet connection in New York, New York USA to a WiFi connection
    /// in Beaverton, Oregon USA.
    ///
    /// This was created using [`fit`](Self::fit) provided with 57,129
    /// packets (~9 minutes) of real network data and an `rtt_overhead` of
    /// 33.5 ms.
    ///
    /// This preset is one direction. To simulate the full pair, install
    /// this on the peer receiving the New York to Beaverton direction and
    /// [`beaverton_to_ny`](Self::beaverton_to_ny) on the other peer.
    pub fn ny_to_beaverton() -> Self {
        Self {
            incoming_latency: Duration::from_micros(50_177),
            incoming_jitter: Duration::from_micros(9_333),
            good_loss: 0.0,
            bad_loss: 0.991_008_9,
            good_to_bad: 0.002_257_159_2,
            bad_to_good: 0.128_858_07,
        }
    }

    /// Returns a conditioner that simulates the link conditions from a
    /// WiFi connection in Beaverton, Oregon USA to an ethernet connection
    /// in New York, New York USA.
    ///
    /// This was created using [`fit`](Self::fit) provided with 34,836
    /// packets (~9 minutes) of real network data and an `rtt_overhead` of
    /// 33.5 ms.
    ///
    /// This preset is one direction. To simulate the full pair, install
    /// this on the peer receiving the Beaverton to New York direction and
    /// [`ny_to_beaverton`](Self::ny_to_beaverton) on the other peer.
    pub fn beaverton_to_ny() -> Self {
        Self {
            incoming_latency: Duration::from_micros(55_847),
            incoming_jitter: Duration::from_micros(17_000),
            good_loss: 0.0,
            bad_loss: 1.0,
            good_to_bad: 0.040_261_284,
            bad_to_good: 0.636_890_35,
        }
    }

    /// Returns a conditioner that simulates the link conditions from an
    /// ethernet connection in New York, New York USA to a WiFi connection
    /// in Reno, Nevada USA.
    ///
    /// This was created using [`fit`](Self::fit) provided with 62,608
    /// packets (~10 minutes) of real network data and an `rtt_overhead` of
    /// 33.5 ms.
    ///
    /// This preset is one direction. To simulate the full pair, install
    /// this on the peer receiving the New York to Reno direction and
    /// [`reno_to_ny`](Self::reno_to_ny) on the other peer.
    pub fn ny_to_reno() -> Self {
        Self {
            incoming_latency: Duration::from_micros(49_695),
            incoming_jitter: Duration::from_micros(5_141),
            good_loss: 0.0,
            bad_loss: 1.0,
            good_to_bad: 0.000_483_582_4,
            bad_to_good: 0.131_578_95,
        }
    }

    /// Returns a conditioner that simulates the link conditions from a
    /// WiFi connection in Reno, Nevada USA to an ethernet connection in
    /// New York, New York USA.
    ///
    /// This was created using [`fit`](Self::fit) provided with 34,829
    /// packets (~10 minutes) of real network data and an `rtt_overhead` of
    /// 33.5 ms.
    ///
    /// This preset is one direction. To simulate the full pair, install
    /// this on the peer receiving the Reno to New York direction and
    /// [`ny_to_reno`](Self::ny_to_reno) on the other peer.
    pub fn reno_to_ny() -> Self {
        Self {
            incoming_latency: Duration::from_micros(51_411),
            incoming_jitter: Duration::from_micros(14_481),
            good_loss: 0.0,
            bad_loss: 1.0,
            good_to_bad: 0.055_427_25,
            bad_to_good: 0.764_980_85,
        }
    }

    /// Returns a conditioner that simulates the link conditions from an
    /// ethernet connection in New York, New York USA to a WiFi connection
    /// in Pahrump, Nevada USA.
    ///
    /// This was created using [`fit`](Self::fit) provided with 53,101
    /// packets (~8.5 minutes) of real network data and an `rtt_overhead`
    /// of 33.5 ms.
    ///
    /// This preset is one direction. To simulate the full pair, install
    /// this on the peer receiving the New York to Pahrump direction and
    /// [`pahrump_to_ny`](Self::pahrump_to_ny) on the other peer.
    pub fn ny_to_pahrump() -> Self {
        Self {
            incoming_latency: Duration::from_micros(33_663),
            incoming_jitter: Duration::from_micros(8_985),
            good_loss: 0.0,
            bad_loss: 0.984_078_6,
            good_to_bad: 0.000_684_733_44,
            bad_to_good: 0.061_043_583,
        }
    }

    /// Returns a conditioner that simulates the link conditions from a
    /// WiFi connection in Pahrump, Nevada USA to an ethernet connection in
    /// New York, New York USA.
    ///
    /// This was created using [`fit`](Self::fit) provided with 39,043
    /// packets (~8.5 minutes) of real network data and an `rtt_overhead`
    /// of 33.5 ms.
    ///
    /// This preset is one direction. To simulate the full pair, install
    /// this on the peer receiving the Pahrump to New York direction and
    /// [`ny_to_pahrump`](Self::ny_to_pahrump) on the other peer.
    pub fn pahrump_to_ny() -> Self {
        Self {
            incoming_latency: Duration::from_micros(39_821),
            incoming_jitter: Duration::from_micros(14_321),
            good_loss: 0.0,
            bad_loss: 1.0,
            good_to_bad: 0.068_795_934,
            bad_to_good: 0.591_445_45,
        }
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

    /// Runs `count` packets through a real seeded [`LinkConditioner`] and
    /// records each packet's outcome, so a fit of the output is a true round
    /// trip through the runtime model. A delivered packet's RTT is twice its
    /// simulated one-way delay.
    fn simulate_packet_outcomes(
        config: &LinkConditionerConfig,
        count: u32,
        seed: u64,
    ) -> Vec<ResolvedPacket> {
        let mut conditioner = LinkConditioner::with_seed(config.clone(), seed);
        let sent_at = Instant::now();
        (0..count)
            .map(|packet_id| ResolvedPacket {
                packet_id,
                rtt: conditioner
                    .condition_packet(packet_id, sent_at)
                    .map(|delivered_at| delivered_at.duration_since(sent_at) * 2),
            })
            .collect()
    }

    #[test]
    fn fit_empty_returns_none() {
        assert!(LinkConditionerConfig::fit(Vec::new(), Duration::ZERO).is_none());
    }

    #[test]
    fn fit_loss_fits_all_lost_capture_as_total_loss() {
        let lost_packets: Vec<ResolvedPacket> = (0..100)
            .map(|packet_id| ResolvedPacket {
                packet_id,
                rtt: None,
            })
            .collect();

        // `fit` cannot work with only lost packets.
        assert!(LinkConditionerConfig::fit(lost_packets.clone(), Duration::ZERO).is_none());

        // `fit_loss` should return a 100% packet loss probability because every packet
        // was lost.
        let config = LinkConditionerConfig::default().fit_loss(&lost_packets);
        assert_eq!(config.mean_loss(), 1.0);
    }

    #[test]
    fn fit_recovers_delay_and_reports_lossless() {
        // Generate network traffic with a latency of 80ms ± 20ms.
        let config = LinkConditionerConfig::default()
            .with_incoming_latency(Duration::from_millis(80))
            .with_incoming_jitter(Duration::from_millis(20));
        let sent_packets = simulate_packet_outcomes(&config, 20_000, 7);
        let fitted = LinkConditionerConfig::fit(sent_packets, Duration::ZERO).unwrap();

        // Verify `fit` determined the correct latency and jitter.
        assert_relative_eq!(fitted.incoming_latency.as_secs_f64(), 0.080, epsilon = 3e-3);
        assert_relative_eq!(fitted.incoming_jitter.as_secs_f64(), 0.020, epsilon = 3e-3);
        assert_eq!(fitted.mean_loss(), 0.0);
    }

    #[test]
    fn fit_deducts_rtt_overhead_from_latency_only() {
        // Generate network traffic with a latency of 80ms ± 20ms.
        let config = LinkConditionerConfig::default()
            .with_incoming_latency(Duration::from_millis(80))
            .with_incoming_jitter(Duration::from_millis(20));
        let sent_packets = simulate_packet_outcomes(&config, 20_000, 7);

        // Pretend that 30ms of the RTT was due to RTT overhead then verify `fit`
        // determined the correct latency and jitter.
        let fitted = LinkConditionerConfig::fit(sent_packets, Duration::from_millis(30)).unwrap();

        // `fit` should remove the RTT overhead from its `incoming_latency` estimate.
        assert_relative_eq!(fitted.incoming_latency.as_secs_f64(), 0.065, epsilon = 3e-3);
        assert_relative_eq!(fitted.incoming_jitter.as_secs_f64(), 0.020, epsilon = 3e-3);
    }

    #[test]
    fn fit_sparse_loss_falls_back_to_fixed() {
        // 10 isolated losses is too few runs to claim burst structure.
        let sent_packets = (0..5_000u32).map(|packet_id| ResolvedPacket {
            packet_id,
            rtt: (packet_id % 500 != 250).then_some(Duration::from_millis(100)),
        });
        let loss = LinkConditionerConfig::fit(sent_packets.collect(), Duration::ZERO).unwrap();
        assert_eq!(loss.good_to_bad, 0.0);
        assert_relative_eq!(loss.mean_loss(), 10.0 / 5_000.0, epsilon = 1e-4);
    }

    #[test]
    fn fit_round_trips_simple_gilbert() {
        let truth = LinkConditionerConfig::default().with_simple_gilbert_loss(0.02, 8.0);
        let sent_packets = simulate_packet_outcomes(&truth, 400_000, 11);
        let fitted = LinkConditionerConfig::fit(sent_packets, Duration::ZERO).unwrap();
        assert_relative_eq!(fitted.mean_loss(), 0.02, max_relative = 0.1);
        assert_relative_eq!(1.0 / fitted.bad_to_good, 8.0, max_relative = 0.2);
        // Total loss inside bursts must be recovered as near-total.
        assert!(fitted.bad_loss > 0.85, "bad_loss = {}", fitted.bad_loss);
    }

    #[test]
    fn fit_round_trips_gilbert() {
        let truth = LinkConditionerConfig::default().with_gilbert_loss(0.05, 6.0, 0.5);
        let sent_packets = simulate_packet_outcomes(&truth, 400_000, 13);
        let fitted = LinkConditionerConfig::fit(sent_packets, Duration::ZERO).unwrap();
        assert_relative_eq!(fitted.mean_loss(), 0.05, max_relative = 0.1);
        assert_relative_eq!(1.0 / fitted.bad_to_good, 6.0, max_relative = 0.3);
        assert_relative_eq!(fitted.bad_loss, 0.5, epsilon = 0.15);
    }

    #[test]
    fn fit_trims_boundary_loss_runs() {
        // A leading connect burst and a trailing teardown burst around an
        // otherwise clean body must fit as lossless.
        let sent_packets = (0..10_000u32).map(|packet_id| ResolvedPacket {
            packet_id,
            rtt: (packet_id >= 5 && packet_id < 9_700).then_some(Duration::from_millis(100)),
        });
        let loss = LinkConditionerConfig::fit(sent_packets.collect(), Duration::ZERO).unwrap();
        assert_eq!(loss.mean_loss(), 0.0);
    }

    #[test]
    fn fit_does_not_merge_runs_across_id_gaps() {
        // Generate network traffic comprised of repeated blocks of packets where [20
        // acked, 2 lost, gap, 2 lost, 20 acked].
        let mut sent_packets = Vec::new();
        let mut packet_id = 0u32;
        for _ in 0..40 {
            for _ in 0..20 {
                sent_packets.push(ResolvedPacket {
                    packet_id,
                    rtt: Some(Duration::from_millis(100)),
                });
                packet_id += 1;
            }
            for _ in 0..2 {
                sent_packets.push(ResolvedPacket {
                    packet_id,
                    rtt: None,
                });
                packet_id += 1;
            }
            // The gap (i.e. unresolved packet IDs).
            packet_id += 3;
            for _ in 0..2 {
                sent_packets.push(ResolvedPacket {
                    packet_id,
                    rtt: None,
                });
                packet_id += 1;
            }
            for _ in 0..20 {
                sent_packets.push(ResolvedPacket {
                    packet_id,
                    rtt: Some(Duration::from_millis(100)),
                });
                packet_id += 1;
            }
        }
        let loss = LinkConditionerConfig::fit(sent_packets, Duration::ZERO).unwrap();
        // 80 observed runs of exactly 2 → Simple Gilbert with burst length 2.
        assert_relative_eq!(1.0 / loss.bad_to_good, 2.0, epsilon = 1e-4);
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
