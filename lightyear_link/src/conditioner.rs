//! Contains the `LinkConditioner` struct which can be used to simulate network conditions
use bevy::platform::time::Instant;
use bevy::reflect::Reflect;
use core::time::Duration;
use lightyear_utils::ready_buffer::ReadyBuffer;
use rand::Rng;

/// Contains configuration required to initialize a LinkConditioner
#[derive(Clone, Debug, Reflect)]
pub struct LinkConditionerConfig {
    /// Delay to receive incoming messages in milliseconds (half the RTT)
    pub incoming_latency: Duration,
    /// The maximum additional random latency to delay received incoming
    /// messages in milliseconds. This may be added OR subtracted from the
    /// latency determined in the `incoming_latency` property above
    pub incoming_jitter: Duration,
    /// The % chance that an incoming packet will be dropped.
    /// Represented as a value between 0 and 1
    pub incoming_loss: f32,
}

#[derive(Debug, Clone)]
pub struct LinkConditioner<P: Eq> {
    config: LinkConditionerConfig,
    pub time_queue: ReadyBuffer<Instant, P>,
}

impl<P: Eq> LinkConditioner<P> {
    pub fn new(config: LinkConditionerConfig) -> Self {
        LinkConditioner {
            config,
            time_queue: ReadyBuffer::new(),
        }
    }

    /// Add latency/jitter/loss to a packet
    ///
    /// `elapsed`: Duration since app start
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

    /// Check if a packet is ready to be returned
    pub(crate) fn pop_packet(&mut self, instant: Instant) -> Option<P> {
        self.time_queue.pop_item(&instant).map(|(_, packet)| packet)
    }
}

impl LinkConditionerConfig {
    /// Creates a new LinkConditionerConfig
    pub fn new(incoming_latency: Duration, incoming_jitter: Duration, incoming_loss: f32) -> Self {
        LinkConditionerConfig {
            incoming_latency,
            incoming_jitter,
            incoming_loss,
        }
    }

    /// Creates a new LinkConditioner that simulates a connection which is in a
    /// good condition
    pub fn good_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(40),
            incoming_jitter: Duration::from_millis(6),
            incoming_loss: 0.002,
        }
    }

    /// Creates a new `LinkConditioner` that simulates a connection which is in an
    /// average condition
    pub fn average_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(100),
            incoming_jitter: Duration::from_millis(15),
            incoming_loss: 0.02,
        }
    }

    /// Creates a new `LinkConditioner` that simulates a connection which is in an
    /// poor condition
    pub fn poor_condition() -> Self {
        LinkConditionerConfig {
            incoming_latency: Duration::from_millis(200),
            incoming_jitter: Duration::from_millis(30),
            incoming_loss: 0.04,
        }
    }
}
