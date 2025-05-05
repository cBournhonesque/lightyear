//! Manages sending/receiving pings and computing network statistics
#[cfg(not(feature = "std"))]
use alloc::{vec, vec::Vec};
use bevy::reflect::Reflect;
use bevy::time::Stopwatch;
use core::time::Duration;
use tracing::{error, trace};

use crate::shared::ping::message::{Ping, Pong};
use crate::shared::ping::store::{PingId, PingStore};
use crate::shared::time_manager::{TimeManager, WrappedTime};
use crate::utils::ready_buffer::ReadyBuffer;

/// Config for the ping manager, which sends regular pings to the remote machine in order
/// to compute network statistics (RTT, jitter)
#[derive(Clone, Copy, Debug, Reflect)]
pub struct PingConfig {
    /// The duration to wait before sending a ping message to the remote host,
    /// in order to estimate RTT time
    pub ping_interval: Duration,
    /// Duration of the rolling buffer of stats to compute RTT/jitter
    /// NOTE: this must be high enough to have received enough pongs to sync
    pub stats_buffer_duration: Duration,
}

impl Default for PingConfig {
    fn default() -> Self {
        PingConfig {
            ping_interval: Duration::from_millis(100),
            stats_buffer_duration: Duration::from_secs(4),
        }
    }
}

/// The [`PingManager`] is responsible for sending regular pings to the remote machine,
/// and monitor pongs in order to estimate statistics (rtt, jitter) about the connection.
#[derive(Debug)]
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
    pongs_to_send: Vec<Pong>,

    // stats
    // TODO: we could actually compute stats from every single packet, not just pings/pongs
    /// Buffer to store the connection stats from the last few pongs received
    pub(crate) sync_stats: SyncStatsBuffer,
    /// Current best estimates of various networking statistics
    pub final_stats: FinalStats,
    /// The number of pings we have sent
    pub(crate) pings_sent: u32,
    /// The number of pongs we have received
    pub(crate) pongs_recv: u32,
}

/// Connection stats aggregated over several [`SyncStats`]
#[derive(Debug)]
pub struct FinalStats {
    pub rtt: Duration,
    pub jitter: Duration,
}

impl Default for FinalStats {
    fn default() -> Self {
        Self {
            // start with a conservative estimate
            rtt: Duration::from_millis(100),
            jitter: Duration::default(),
        }
    }
}

/// Stats computed from each pong
#[derive(Debug, PartialEq)]
pub struct SyncStats {
    pub(crate) round_trip_delay: Duration,
}

pub type SyncStatsBuffer = ReadyBuffer<WrappedTime, SyncStats>;

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
            sync_stats: SyncStatsBuffer::new(),
            final_stats: FinalStats::default(),
            pings_sent: 0,
            pongs_recv: 0,
        }
    }

    /// Return the latest estimate of rtt
    pub fn rtt(&self) -> Duration {
        self.final_stats.rtt
    }

    /// Return the latest estimate of jitter
    pub fn jitter(&self) -> Duration {
        self.final_stats.jitter
    }

    /// Update the ping manager after a delta update
    pub(crate) fn update(&mut self, time_manager: &TimeManager) {
        self.ping_timer.tick(time_manager.delta());

        // clear stats that are older than a threshold, such as 2 seconds
        let oldest_time = time_manager.current_time() - self.config.stats_buffer_duration;
        let old_len = self.sync_stats.len();
        self.sync_stats.pop_until(&oldest_time);
        let new_len = self.sync_stats.len();

        // recompute RTT jitter from the last 2-seconds of stats if we popped anything
        if old_len != new_len {
            self.compute_stats();
            #[cfg(feature = "metrics")]
            {
                metrics::gauge!("connection::rtt_ms").set(self.rtt().as_millis() as f64);
                metrics::gauge!("connection::jitter_ms").set(self.jitter().as_millis() as f64);
            }
        }

        // NOTE: no need to clear anything in the ping_store because new pings will overwrite older pings
    }

    /// Check if we are ready to send a ping to the remote
    pub(crate) fn maybe_prepare_ping(&mut self, time_manager: &TimeManager) -> Option<Ping> {
        // TODO: should we have something to start sending a sync ping right away? (so we don't wait for initial timer)
        if self.ping_timer.elapsed() >= self.config.ping_interval {
            self.ping_timer.reset();

            let ping_id = self.ping_store.push_new(time_manager.current_time());
            self.pings_sent += 1;
            return Some(Ping { id: ping_id });
        }
        None
    }

    // TODO: optimization
    //  - for efficiency, we want to use a rolling mean/std algorithm
    //  - every N seconds (for example 2 seconds), we clear the buffer for stats older than 2 seconds and recompute mean/std from the remaining elements
    /// Compute the stats (offset, rtt, jitter) from the stats present in the buffer
    pub fn compute_stats(&mut self) {
        let sample_count = self.sync_stats.len() as f64;

        // Find the Mean
        let rtt_mean = self.sync_stats.heap.iter().fold(0.0, |acc, stat| {
            let item = &stat.item;
            acc + item.round_trip_delay.as_secs_f64() / sample_count
        });

        // TODO: should I use biased or unbiased estimator?
        // Find the Variance
        let rtt_diff_mean: f64 = self.sync_stats.heap.iter().fold(0.0, |acc, stat| {
            let item = &stat.item;
            acc + (item.round_trip_delay.as_secs_f64() - rtt_mean).powi(2) / (sample_count)
        });

        // Find the Standard Deviation
        let rtt_stdv = rtt_diff_mean.sqrt();

        // Get the pruned mean: keep only the stat values inside the standard deviation (mitigation)
        let pruned_samples = self.sync_stats.heap.iter().filter(|stat| {
            let item = &stat.item;
            let rtt_diff = (item.round_trip_delay.as_secs_f64() - rtt_mean).abs();
            rtt_diff <= rtt_stdv + 1000.0 * f64::EPSILON
        });
        let (pruned_rtt_mean, pruned_sample_count) =
            pruned_samples.fold((0.0, 0.0), |acc, stat| {
                let item = &stat.item;
                (acc.0 + item.round_trip_delay.as_secs_f64(), acc.1 + 1.0)
            });

        let final_rtt_mean = if pruned_sample_count > 0.0 {
            pruned_rtt_mean / pruned_sample_count
        } else {
            rtt_mean
        };

        // TODO: recompute rtt_stdv from pruned ?
        // Find the Mean
        let rtt_mean = self.sync_stats.heap.iter().fold(0.0, |acc, stat| {
            let item = &stat.item;
            acc + item.round_trip_delay.as_secs_f64() / sample_count
        });

        // TODO: should I use biased or unbiased estimator?
        // Find the Variance
        let rtt_diff_mean: f64 = self.sync_stats.heap.iter().fold(0.0, |acc, stat| {
            let item = &stat.item;
            acc + (item.round_trip_delay.as_secs_f64() - rtt_mean).powi(2) / (sample_count)
        });
        let final_rtt_stdv = if pruned_sample_count > 0.0 {
            rtt_diff_mean.sqrt()
        } else {
            0.0
        };

        self.final_stats = FinalStats {
            // rtt: Duration::from_secs_f64(rtt_mean),
            rtt: Duration::from_secs_f64(final_rtt_mean),
            // jitter is based on one-way delay, so we divide by 2
            jitter: Duration::from_secs_f64(final_rtt_stdv / 2.0),
        };
        trace!(
            rtt = ?self.final_stats.rtt,
            jitter = ?self.final_stats.jitter,
            "Computed stats!"
        );
    }

    /// Received a pong: update
    /// Returns true if we have enough pongs to finalize the handshake
    pub(crate) fn process_pong(&mut self, pong: &Pong, current_time: WrappedTime) {
        trace!("Received pong: {:?}", pong);
        self.pongs_recv += 1;
        let received_time = current_time;

        let Some(ping_sent_time) = self.ping_store.remove(pong.ping_id) else {
            error!("Received a ping that is not present in the ping-store anymore");
            return;
        };

        // only update values for the most recent pongs received
        if pong.ping_id > self.most_recent_received_ping {
            // compute round-trip delay via NTP algorithm: https://en.wikipedia.org/wiki/Network_Time_Protocol
            self.most_recent_received_ping = pong.ping_id;

            // round-trip-delay
            // info!(?received_time, ?ping_sent_time, "rtt");
            let rtt = received_time - ping_sent_time;
            // info!(pong_sent_time = ?pong.pong_sent_time, ping_received_time = ?pong.ping_received_time, "server process time");
            let server_process_time = pong.pong_sent_time - pong.ping_received_time;
            trace!(?rtt, ?received_time, ?ping_sent_time, ?server_process_time, ?pong.pong_sent_time, ?pong.ping_received_time, "process pong");
            let round_trip_delay = (rtt - server_process_time).to_std().unwrap_or_default();

            // update stats buffer
            self.sync_stats
                .push(received_time, SyncStats { round_trip_delay });

            // recompute stats whenever we get a new pong
            self.compute_stats();
        }
    }

    /// When we receive a Ping, we prepare a Pong in response.
    /// However we cannot send it immediately because we send packets at a regular interval
    /// Keep track of the pongs we need to send
    pub(crate) fn buffer_pending_pong(&mut self, ping: &Ping, current_time: WrappedTime) {
        self.pongs_to_send.push(Pong {
            ping_id: ping.id,
            // TODO: we want to use real time instead of just time_manager.current_time() no?
            ping_received_time: current_time,
            // TODO: can we get a more precise time? (based on real)?
            // TODO: otherwise we can consider that there's an entire tick duration between receive and sent
            // we are using 0.0 as a placeholder for now, we will fill it when we actually
            // send the pong
            // TODO: use option?
            pong_sent_time: WrappedTime::default(),
        })
    }
    pub(crate) fn take_pending_pongs(&mut self) -> Vec<Pong> {
        core::mem::take(&mut self.pongs_to_send)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_pings() {
        let config = PingConfig {
            ping_interval: Duration::from_millis(100),
            stats_buffer_duration: Duration::from_secs(4),
        };
        let mut ping_manager = PingManager::new(config);
        let mut time_manager = TimeManager::default();

        assert_eq!(ping_manager.maybe_prepare_ping(&time_manager), None);

        let delta = Duration::from_millis(100);
        time_manager.update(delta);
        ping_manager.update(&time_manager);

        // send pings
        assert_eq!(
            ping_manager.maybe_prepare_ping(&time_manager),
            Some(Ping { id: PingId(0) })
        );
        let delta = Duration::from_millis(60);
        time_manager.update(delta);
        ping_manager.update(&time_manager);

        // ping timer hasn't gone off yet, send nothing
        assert_eq!(ping_manager.maybe_prepare_ping(&time_manager), None);
        time_manager.update(delta);
        ping_manager.update(&time_manager);
        assert_eq!(
            ping_manager.maybe_prepare_ping(&time_manager),
            Some(Ping { id: PingId(1) })
        );

        let delta = Duration::from_millis(100);
        time_manager.update(delta);
        ping_manager.update(&time_manager);
        assert_eq!(
            ping_manager.maybe_prepare_ping(&time_manager),
            Some(Ping { id: PingId(2) })
        );

        // we sent all the pings we need
        assert_eq!(ping_manager.maybe_prepare_ping(&time_manager), None);

        // check ping store
        assert_eq!(
            ping_manager.ping_store.remove(PingId(0)),
            Some(WrappedTime::new(100))
        );
        assert_eq!(
            ping_manager.ping_store.remove(PingId(1)),
            Some(WrappedTime::new(220))
        );
        assert_eq!(
            ping_manager.ping_store.remove(PingId(2)),
            Some(WrappedTime::new(320))
        );

        // receive pongs
        // TODO
    }

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
