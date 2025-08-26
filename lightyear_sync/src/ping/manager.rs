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
use tracing::{error, info, trace};

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
/// and monitor pongs in order to estimate statistics (rtt, jitter) about the connection.
#[derive(Debug, Component)]
#[require(MessageSender<Ping>, MessageReceiver<Ping>, MessageSender<Pong>, MessageReceiver<Pong>)]
pub struct PingManager {
    config: PingConfig,
    /// Timer to send regular pings to the remote
    ping_timer: Stopwatch,
    /// ping store to track which pings we sent
    ping_store: PingStore,
    /// ping id corresponding to the most recent pong received
    most_recent_received_ping: PingId,
    /// We received time-sync pongs; we keep track that we will have to send pongs back when we can
    /// (when the connection's send_timer is ready)
    pongs_to_send: Vec<(Pong, Instant)>,

    // TODO: we could actually compute stats from every single packet, not just pings/pongs
    /// Estimator used to compute RTT/Jitter from the pongs received
    pub rtt_estimator_ewma: RttEstimatorEwma,
    /// The number of pings we have sent
    pub(crate) pings_sent: u32,
    /// The number of pongs we have received
    pub pongs_recv: u32,
}

impl Default for PingManager {
    fn default() -> Self {
        Self {
            config: PingConfig::default(),
            ping_timer: Stopwatch::new(),
            ping_store: PingStore::new(),
            most_recent_received_ping: PingId(u16::MAX - 1),
            pongs_to_send: vec![],
            rtt_estimator_ewma: RttEstimatorEwma::default(),
            pings_sent: 0,
            pongs_recv: 0,
        }
    }
}

impl PingManager {
    pub(crate) fn reset(&mut self) {
        self.ping_timer.reset();
        self.ping_store.reset();
        self.most_recent_received_ping = PingId(u16::MAX - 1);
        self.pongs_to_send.clear();
        self.rtt_estimator_ewma.reset();
        self.pings_sent = 0;
        self.pongs_recv = 0;
    }
}

impl PingManager {
    pub fn new(config: PingConfig) -> Self {
        Self {
            config,
            // pings
            ping_timer: Stopwatch::new(),
            ping_store: PingStore::new(),
            most_recent_received_ping: PingId(u16::MAX - 1),
            pongs_to_send: vec![],
            // sync
            rtt_estimator_ewma: RttEstimatorEwma::default(),
            pings_sent: 0,
            pongs_recv: 0,
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

    /// Update the ping manager after a delta update
    pub(crate) fn update(&mut self, time: &Time<Real>) {
        self.ping_timer.tick(time.delta());
    }

    /// Check if we are ready to send a ping to the remote
    pub(crate) fn maybe_prepare_ping(&mut self, now: Instant) -> Option<Ping> {
        // TODO: should we have something to start sending a sync ping right away? (so we don't wait for initial timer)
        if self.ping_timer.elapsed() >= self.config.ping_interval {
            self.ping_timer.reset();

            let ping_id = self.ping_store.push_new(now);
            self.pings_sent += 1;
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
        if pong.ping_id > self.most_recent_received_ping {
            // compute round-trip delay via NTP algorithm: https://en.wikipedia.org/wiki/Network_Time_Protocol
            self.most_recent_received_ping = pong.ping_id;

            // round-trip-delay
            let rtt = received_time.saturating_duration_since(ping_sent_time);
            let server_process_time = TickDelta::from(pong.frame_time).to_duration(tick_duration);
            trace!(?rtt, ?received_time, ?ping_sent_time, ?pong.frame_time,  "process received pong");
            let round_trip_delay = rtt.saturating_sub(server_process_time);

            // recompute stats whenever we get a new pong
            self.rtt_estimator_ewma
                .update_with_new_sample(round_trip_delay);
        }
    }

    /// When we receive a Ping, we prepare a Pong in response.
    /// However we cannot send it immediately because we send packets at a regular interval
    /// Keep track of the pongs we need to send
    pub(crate) fn buffer_pending_pong(&mut self, ping: &Ping, now: Instant) {
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

#[cfg(test)]
mod tests {

    // #[test]
    // fn test_send_pings() {
    //     let config = PingConfig {
    //         ping_interval: Duration::from_millis(100),
    //         stats_buffer_duration: Duration::from_secs(4),
    //     };
    //     let mut ping_manager = PingManager::new(config, Duration::default());
    //     let mut real = Time::<Real>::default();
    //     real.update();
    //
    //     assert_eq!(ping_manager.maybe_prepare_ping(real.last_update().unwrap()), None);
    //
    //     let delta = Duration::from_millis(100);
    //     real.update_with_duration(delta);
    //     ping_manager.update(&real);
    //
    //     // send pings
    //     assert_eq!(
    //         ping_manager.maybe_prepare_ping(real.last_update().unwrap()),
    //         Some(Ping { id: PingId(0) })
    //     );
    //     let delta = Duration::from_millis(60);
    //     real.update_with_duration(delta);
    //     ping_manager.update(&real);
    //
    //     // ping timer hasn't gone off yet, send nothing
    //     assert_eq!(ping_manager.maybe_prepare_ping(real.last_update().unwrap()), None);
    //     real.update_with_duration(delta);
    //     ping_manager.update(&real);
    //     assert_eq!(
    //         ping_manager.maybe_prepare_ping(real.last_update().unwrap()),
    //         Some(Ping { id: PingId(1) })
    //     );
    //
    //     let delta = Duration::from_millis(100);
    //     real.update_with_duration(delta);
    //     ping_manager.update(&real);
    //     assert_eq!(
    //         ping_manager.maybe_prepare_ping(real.last_update().unwrap()),
    //         Some(Ping { id: PingId(2) })
    //     );
    //
    //     // we sent all the pings we need
    //     assert_eq!(ping_manager.maybe_prepare_ping(real.last_update().unwrap()), None);
    //
    //     // check ping store
    //     assert_eq!(
    //         ping_manager.ping_store.remove(PingId(0)),
    //         Some(Duration::from_millis(100))
    //     );
    //     assert_eq!(
    //         ping_manager.ping_store.remove(PingId(1)),
    //         Some(Duration::from_millis(220))
    //     );
    //     assert_eq!(
    //         ping_manager.ping_store.remove(PingId(2)),
    //         Some(Duration::from_millis(320))
    //     );
    //
    //     // receive pongs
    //     // TODO
    // }

    // #[test]
    // fn test_ping_manager() {
    //     let ping_config = PingConfig {
    //         ping_interval_ms: Duration::from_millis(100),
    //         rtt_ms_initial_estimate: Duration::from_millis(10),
    //         jitter_ms_initial_estimate: Default::default(),
    //         rtt_smoothing_factor: 0.0,
    //     };
    //     let mut ping_manager = PingManager::new(&ping_config);
    //     // let tick_config = TickConfig::new(Duration::from_millis(16));
    //     let mut time_manager = TimeManager::new(Duration::default());
    //
    //     assert!(!ping_manager.should_send_ping());
    //     let delta = Duration::from_millis(100);
    //     ping_manager.update(delta);
    //     time_manager.update(delta, Duration::default());
    //     assert!(ping_manager.should_send_ping());
    //
    //     let ping_message = ping_manager.prepare_ping(&time_manager);
    //     assert!(!ping_manager.should_send_ping());
    //     assert_eq!(ping_message.id, PingId(0));
    //
    //     let delta = Duration::from_millis(20);
    //     ping_manager.update(delta);
    //     time_manager.update(delta, Duration::default());
    //     let pong_message = Pong {
    //         ping_id: PingId(0),
    //         tick: Default::default(),
    //         offset_sec: 0.0,
    //     };
    //     ping_manager.process_pong(pong_message, &time_manager);
    //
    //     assert_eq!(ping_manager.rtt_ms_average, 0.9 * 10.0 + 0.1 * 20.0);
    //     assert_eq!(ping_manager.jitter_ms_average, 0.9 * 0.0 + 0.1 * 5.0);
    // }
}
