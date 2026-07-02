//! Manages sending/receiving pings and computing network statistics
use crate::ping::estimator::RttEstimatorEwma;
use crate::ping::message::{Ping, Pong};
use crate::ping::store::{PingId, PingStore};
use alloc::{vec, vec::Vec};
use bevy_ecs::component::Component;
use bevy_reflect::Reflect;
use bevy_time::{Real, Stopwatch, Time};
use core::time::Duration;
use lightyear_core::time::Instant;
use lightyear_core::time::TickDelta;
use lightyear_messages::prelude::{MessageReceiver, MessageSender};
use tracing::{error, trace};

/// Config for the ping manager, which sends regular pings to the remote machine in order
/// to compute network statistics (RTT, jitter)
#[derive(Clone, Copy, Debug, Reflect)]
pub struct PingConfig {
    /// The duration to wait before sending a ping message to the remote host,
    /// in order to estimate RTT time
    pub ping_interval: Duration,
}

impl Default for PingConfig {
    fn default() -> Self {
        PingConfig {
            ping_interval: Duration::from_millis(100),
        }
    }
}

/// The [`PingManager`] is responsible for sending regular pings to the remote machine,
/// and monitoring pongs in order to estimate statistics (RTT, jitter) about the connection.
#[derive(Debug, Component)]
#[require(MessageSender<Ping>, MessageReceiver<Ping>, MessageSender<Pong>, MessageReceiver<Pong>)]
pub struct PingManager {
    config: PingConfig,
    /// Timer to send regular pings to the remote
    ping_timer: Stopwatch,
    /// ping store to track which pings we sent
    ping_store: PingStore,
    /// ping id corresponding to the most recent pong received
    most_recent_received_ping: Option<PingId>,
    /// We received time-sync pongs; we keep track that we will have to send pongs back when we can
    /// (when the connection's send_timer is ready)
    pongs_to_send: Vec<(Pong, Instant)>,

    /// Estimator used to compute RTT/Jitter from explicit Ping/Pong messages.
    pub rtt_estimator_ewma: RttEstimatorEwma,
    /// The number of pings we have sent
    pub(crate) pings_sent: u32,
    /// The number of pongs we have received
    pub pongs_recv: u32,
    /// The number of Ping/Pong latency samples used by the estimator.
    latency_samples_recv: u32,
}

impl Default for PingManager {
    fn default() -> Self {
        Self {
            config: PingConfig::default(),
            ping_timer: Stopwatch::new(),
            ping_store: PingStore::new(),
            most_recent_received_ping: None,
            pongs_to_send: vec![],
            rtt_estimator_ewma: RttEstimatorEwma::default(),
            pings_sent: 0,
            pongs_recv: 0,
            latency_samples_recv: 0,
        }
    }
}

impl PingManager {
    pub(crate) fn reset(&mut self) {
        self.ping_timer.reset();
        self.ping_store.reset();
        self.most_recent_received_ping = None;
        self.pongs_to_send.clear();
        self.rtt_estimator_ewma.reset();
        self.pings_sent = 0;
        self.pongs_recv = 0;
        self.latency_samples_recv = 0;
    }
}

impl PingManager {
    pub fn new(config: PingConfig) -> Self {
        Self {
            config,
            // pings
            ping_timer: Stopwatch::new(),
            ping_store: PingStore::new(),
            most_recent_received_ping: None,
            pongs_to_send: vec![],
            // sync
            rtt_estimator_ewma: RttEstimatorEwma::default(),
            pings_sent: 0,
            pongs_recv: 0,
            latency_samples_recv: 0,
        }
    }

    /// Return the latest estimate of rtt
    pub fn rtt(&self) -> Duration {
        self.rtt_estimator_ewma.final_stats.rtt
    }

    /// Return the latest estimate of jitter
    pub fn jitter(&self) -> Duration {
        self.rtt_estimator_ewma.final_stats.jitter
    }

    /// Return the number of RTT samples used to estimate latency.
    pub fn latency_samples_recv(&self) -> u32 {
        self.latency_samples_recv
    }

    /// Update the ping manager after a delta update
    pub(crate) fn update(&mut self, time: &Time<Real>) {
        self.ping_timer.tick(time.delta());
    }

    fn record_rtt_sample(&mut self, rtt_sample: Duration) {
        self.latency_samples_recv += 1;
        self.rtt_estimator_ewma.update_with_new_sample(rtt_sample);
    }

    /// Check if we are ready to send a ping to the remote
    pub(crate) fn maybe_prepare_ping(&mut self, now: Instant) -> Option<Ping> {
        // TODO: should we have something to start sending a sync ping right away? (so we don't wait for initial timer)
        if self.ping_timer.elapsed() >= self.config.ping_interval {
            self.ping_timer.reset();

            let ping_id = self.ping_store.push_new(now);
            self.pings_sent += 1;
            trace!(
                target: "lightyear_debug::sync",
                kind = "ping_send",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                ping_id = ping_id.0,
                pings_sent = self.pings_sent,
                "prepared ping"
            );
            return Some(Ping { id: ping_id });
        }
        None
    }

    /// Received a pong: update
    /// Returns true if we have enough pongs to finalize the handshake
    pub(crate) fn process_pong(&mut self, pong: &Pong, now: Instant, tick_duration: Duration) {
        self.pongs_recv += 1;
        let received_time = now;

        let Some(ping_sent_time) = self.ping_store.remove(pong.ping_id) else {
            error!("Received a ping that is not present in the ping-store anymore");
            return;
        };

        // only update values for the most recent pongs received
        if self
            .most_recent_received_ping
            .is_none_or(|latest| pong.ping_id > latest)
        {
            // compute round-trip delay via NTP algorithm: https://en.wikipedia.org/wiki/Network_Time_Protocol
            self.most_recent_received_ping = Some(pong.ping_id);

            // round-trip-delay
            let rtt = received_time.saturating_duration_since(ping_sent_time);
            let server_process_time = TickDelta::from(pong.frame_time).to_duration(tick_duration);
            trace!(?rtt, ?received_time, ?ping_sent_time, ?pong.frame_time,  "process received pong");
            let round_trip_delay = rtt.saturating_sub(server_process_time);

            // recompute stats whenever we get a new pong
            self.record_rtt_sample(round_trip_delay);
            trace!(
                target: "lightyear_debug::sync",
                kind = "pong_recv",
                schedule = "PreUpdate",
                sample_point = "PreUpdate",
                ping_id = pong.ping_id.0,
                pongs_recv = self.pongs_recv,
                latency_samples_recv = self.latency_samples_recv,
                rtt_ms = rtt.as_secs_f64() * 1000.0,
                server_process_ms = server_process_time.as_secs_f64() * 1000.0,
                round_trip_delay_ms = round_trip_delay.as_secs_f64() * 1000.0,
                estimated_rtt_ms = self.rtt().as_secs_f64() * 1000.0,
                jitter_ms = self.jitter().as_secs_f64() * 1000.0,
                "processed pong"
            );
        }
    }

    /// When we receive a Ping, we prepare a Pong in response.
    /// However we cannot send it immediately because we send packets at a regular interval
    /// Keep track of the pongs we need to send
    pub(crate) fn buffer_pending_pong(&mut self, ping: &Ping, now: Instant) {
        trace!(
            target: "lightyear_debug::sync",
            kind = "ping_recv",
            schedule = "PreUpdate",
            sample_point = "PreUpdate",
            ping_id = ping.id.0,
            pending_pongs = self.pongs_to_send.len() + 1,
            "buffered pong response"
        );
        self.pongs_to_send.push((
            Pong {
                ping_id: ping.id,
                frame_time: Default::default(),
            },
            now,
        ))
    }

    pub(crate) fn take_pending_pongs(&mut self) -> Vec<(Pong, Instant)> {
        core::mem::take(&mut self.pongs_to_send)
    }
}
