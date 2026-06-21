//! Receive-side packet conditioning for simulated network latency, jitter, and loss.

use bevy_reflect::Reflect;
use core::time::Duration;
use lightyear_core::time::Instant;
use lightyear_utils::ready_buffer::ReadyBuffer;
use rand::RngExt;

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
    /// Probability that an incoming payload is dropped.
    ///
    /// This is expressed as a fraction in the inclusive range `0.0..=1.0`, where `0.0` keeps every
    /// payload and `1.0` drops every payload. The value is not clamped by the constructor, so callers
    /// should pass a normalized probability.
    pub incoming_loss: f32,
}

/// Generic receive-side packet conditioner.
///
/// `LinkConditioner` delays and drops payloads according to a [`LinkConditionerConfig`].
#[derive(Debug, Clone)]
pub struct LinkConditioner<P: Eq> {
    config: LinkConditionerConfig,
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
            time_queue: ReadyBuffer::new(),
        }
    }

    /// Applies latency, jitter, and loss to `packet` relative to `instant`.
    ///
    /// Dropped packets are discarded immediately. Delivered packets are queued by their simulated
    /// delivery instant.
    pub(crate) fn condition_packet(&mut self, packet: P, instant: Instant) {
        let mut rng = rand::rng();
        if rng.random_range(0.0..1.0) <= self.config.incoming_loss {
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
    /// `incoming_latency` is the base receive delay, `incoming_jitter` is the maximum random
    /// millisecond offset applied around that delay, and `incoming_loss` is the per-payload drop
    /// probability.
    pub fn new(incoming_latency: Duration, incoming_jitter: Duration, incoming_loss: f32) -> Self {
        LinkConditionerConfig {
            incoming_latency,
            incoming_jitter,
            incoming_loss,
        }
    }

    /// Returns an approximate one-way half of this configuration.
    ///
    /// This divides latency, jitter, and packet-loss probability by two.
    pub fn half(self) -> Self {
        LinkConditionerConfig {
            incoming_latency: self.incoming_latency / 2,
            incoming_jitter: self.incoming_jitter / 2,
            incoming_loss: self.incoming_loss / 2.0,
        }
    }

    /// Returns a preset for a low-latency, low-loss connection.
    pub fn good_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(40),
            incoming_jitter: Duration::from_millis(6),
            incoming_loss: 0.002,
        }
    }

    /// Returns a preset for a typical moderate-latency connection.
    pub fn average_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(100),
            incoming_jitter: Duration::from_millis(15),
            incoming_loss: 0.02,
        }
    }

    /// Returns a preset for a high-latency, lossy connection.
    pub fn poor_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(200),
            incoming_jitter: Duration::from_millis(30),
            incoming_loss: 0.10,
        }
    }
}
