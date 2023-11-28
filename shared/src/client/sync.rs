use std::time::Duration;

use bevy::prelude::Res;
use bevy::time::Stopwatch;
use bitvec::macros::internal::funty::Fundamental;
use tracing::{debug, info, trace};

use crate::client::interpolation::plugin::InterpolationDelay;
use crate::client::resource::Client;
use crate::packet::packet::PacketId;
use crate::protocol::Protocol;
use crate::tick::manager::TickManager;
use crate::tick::message::{TimeSyncPingMessage, TimeSyncPongMessage};
use crate::tick::ping_store::{PingId, PingStore};
use crate::tick::time::{TimeManager, WrappedTime};
use crate::tick::Tick;
use crate::utils::ReadyBuffer;

/// Run condition to run systems only if the client is synced
pub fn client_is_synced<P: Protocol>(client: Res<Client<P>>) -> bool {
    client.is_synced()
}

#[derive(Clone, Debug)]
pub struct SyncConfig {
    /// How much multiple of jitter do we apply as margin when computing the time
    /// a packet will get received by the server
    /// (worst case will be RTT / 2 + jitter * multiple_margin)
    /// % of packets that will be received within k * jitter
    /// 1: 65%, 2: 95%, 3: 99.7%
    pub jitter_multiple_margin: u8,
    /// How many ticks to we apply as margin when computing the time
    ///  a packet will get received by the server
    pub tick_margin: u8,
    /// How often do we send sync pings
    pub sync_ping_interval: Duration,
    /// Number of pings to exchange with the server before finalizing the handshake
    pub handshake_pings: u8,
    /// Duration of the rolling buffer of stats to compute RTT/jitter
    pub stats_buffer_duration: Duration,
    /// Error margin for upstream throttle (in multiple of ticks)
    pub error_margin: f32,
    // TODO: instead of constant speedup_factor, the speedup should be linear w.r.t the offset
    /// By how much should we speed up the simulation to make ticks stay in sync with server?
    pub speedup_factor: f32,

    // Integration
    current_server_time_smoothing: f32,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            jitter_multiple_margin: 3,
            tick_margin: 1,
            sync_ping_interval: Duration::from_millis(100),
            handshake_pings: 7,
            stats_buffer_duration: Duration::from_secs(2),
            error_margin: 1.0,
            speedup_factor: 1.03,
            current_server_time_smoothing: 0.1,
        }
    }
}

impl SyncConfig {
    pub fn speedup_factor(mut self, speedup_factor: f32) -> Self {
        self.speedup_factor = speedup_factor;
        self
    }
}

#[derive(Default)]
pub struct SentPacketStore {
    buffer: ReadyBuffer<WrappedTime, PacketId>,
}

impl SentPacketStore {
    pub fn new() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }
}

/// In charge of syncing the client's tick/time with the server's tick/time
/// right after the connection is established
pub struct SyncManager {
    config: SyncConfig,

    // pings
    /// Timer to send regular pings to server
    ping_timer: Stopwatch,
    /// ping store to track which time sync pings we sent
    ping_store: PingStore,
    /// ping id corresponding to the most recent pong received
    most_recent_received_ping: PingId,
    /// We hold the pongs we received (we receive them at PreUpdate but we want to handle them at PostUpdate)
    pong_buffer: Vec<TimeSyncPongMessage>,

    // stats
    /// Buffer to store the stats of the last
    sync_stats: SyncStatsBuffer,
    /// Current best estimates of various networking statistics
    final_stats: FinalStats,
    /// whether the handshake is finalized
    pub(crate) synced: bool,

    // time
    current_server_time: WrappedTime,
    pub(crate) interpolation_time: WrappedTime,
    interpolation_speed_ratio: f32,

    // ticks
    // TODO: see if this is correct; should we instead attach the tick on every update message?
    /// Tick of the server that we last received in any packet from the server.
    /// This is not updated every tick, but only when we receive a packet from the server.
    /// (usually every frame)
    pub(crate) latest_received_server_tick: Tick,
    pub(crate) estimated_interpolation_tick: Tick,
    pub(crate) duration_since_latest_received_server_tick: Duration,
    pub(crate) new_latest_received_server_tick: bool,
}

/// The final stats that we care about
#[derive(Default)]
pub struct FinalStats {
    pub rtt: Duration,
    pub jitter: Duration,
}

/// NTP algorithm stats
// TODO: maybe use Duration?
#[derive(Debug, PartialEq)]
pub struct SyncStats {
    // clock offset: a positive value means that the client clock is faster than server clock
    pub(crate) offset_us: f64,
    pub(crate) round_trip_delay_us: f64,
}

// TODO: maybe use type alias instead?
pub struct SyncStatsBuffer {
    buffer: ReadyBuffer<WrappedTime, SyncStats>,
}

impl SyncStatsBuffer {
    fn new() -> Self {
        Self {
            buffer: ReadyBuffer::new(),
        }
    }
}

// TODO:
// split into Ping/pong manager, PredictionTime Manager, InterpolationTime Manager, StatsManager
impl SyncManager {
    pub fn new(config: SyncConfig) -> Self {
        Self {
            config: config.clone(),
            // pings
            ping_timer: Stopwatch::new(),
            ping_store: PingStore::new(),
            most_recent_received_ping: PingId(u16::MAX - 1),
            pong_buffer: vec![],
            // sync
            sync_stats: SyncStatsBuffer::new(),
            final_stats: FinalStats::default(),
            synced: false,
            // time
            current_server_time: WrappedTime::default(),
            interpolation_time: WrappedTime::default(),
            interpolation_speed_ratio: 1.0,
            // server tick
            latest_received_server_tick: Tick(0),
            estimated_interpolation_tick: Tick(0),
            duration_since_latest_received_server_tick: Duration::default(),
            new_latest_received_server_tick: false,
        }
    }

    pub fn rtt(&self) -> Duration {
        self.final_stats.rtt
    }

    pub fn jitter(&self) -> Duration {
        self.final_stats.jitter
    }

    pub(crate) fn update(
        &mut self,
        time_manager: &mut TimeManager,
        tick_manager: &mut TickManager,
        interpolation_delay: &InterpolationDelay,
        server_update_rate: Duration,
    ) {
        self.ping_timer.tick(time_manager.delta());
        self.duration_since_latest_received_server_tick += time_manager.delta();
        self.current_server_time += time_manager.delta();
        self.interpolation_time += time_manager.delta().mul_f32(self.interpolation_speed_ratio);

        for pong in std::mem::take(&mut self.pong_buffer) {
            if self.process_pong(&pong, time_manager, tick_manager) {
                info!("Received enough pongs to finalize handshake");
                self.synced = true;
                self.finalize(time_manager, tick_manager);
                self.interpolation_time = self.interpolation_objective(
                    interpolation_delay,
                    server_update_rate,
                    tick_manager,
                )
            }
        }

        if self.synced {
            self.update_interpolation_time(interpolation_delay, server_update_rate, tick_manager);

            // TODO: the buffer duration should depend on loss rate!
            // clear stats that are older than a threshold, such as 2 seconds
            let oldest_time = time_manager.current_time() - self.config.stats_buffer_duration;
            let old_len = self.sync_stats.buffer.len();
            self.sync_stats.buffer.pop_until(&oldest_time);
            let new_len = self.sync_stats.buffer.len();

            // recompute RTT jitter from the last 2-seconds of stats if we popped anything
            if old_len != new_len {
                self.final_stats = self.compute_stats();
            }
        }
    }

    pub(crate) fn buffer_pong(&mut self, pong: TimeSyncPongMessage) {
        self.pong_buffer.push(pong);
    }

    pub(crate) fn is_synced(&self) -> bool {
        self.synced
    }

    // TODO: same as ping_manager
    pub(crate) fn maybe_prepare_ping(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Option<TimeSyncPingMessage> {
        // TODO: should we have something to start sending a sync ping right away? (so we don't wait for initial timer)
        if self.ping_timer.elapsed() >= self.config.sync_ping_interval {
            self.ping_timer.reset();

            let ping_id = self
                .ping_store
                .push_new(time_manager.current_time().clone());

            // TODO: for rtt purposes, we could just send a ping that has no tick info
            return Some(TimeSyncPingMessage {
                id: ping_id,
                tick: tick_manager.current_tick(),
                ping_received_time: None,
            });
        }
        None
    }

    // TODO:
    // - for efficiency, we want to use a rolling mean/std algorithm
    // - every N seconds (for example 2 seconds), we clear the buffer for stats older than 2 seconds and recompute mean/std from the remaining elements
    /// Compute the stats (offset, rtt, jitter) from the stats present in the buffer
    pub fn compute_stats(&mut self) -> FinalStats {
        let sample_count = self.sync_stats.buffer.len() as f64;

        // Find the Mean
        let (offset_mean, rtt_mean) =
            self.sync_stats
                .buffer
                .heap
                .iter()
                .fold((0.0, 0.0), |acc, stat| {
                    let item = &stat.item;
                    (
                        acc.0 + item.offset_us / sample_count,
                        acc.1 + item.round_trip_delay_us / sample_count,
                    )
                });

        // TODO: should I use biased or unbiased estimator?
        // Find the Variance
        let (offset_diff_mean, rtt_diff_mean): (f64, f64) = self
            .sync_stats
            .buffer
            .heap
            .iter()
            .fold((0.0, 0.0), |acc, stat| {
                let item = &stat.item;
                (
                    acc.0 + (item.offset_us - offset_mean).powi(2) / (sample_count),
                    acc.1 + (item.round_trip_delay_us - rtt_mean).powi(2) / (sample_count),
                )
            });

        // Find the Standard Deviation
        let (offset_stdv, rtt_stdv) = (offset_diff_mean.sqrt(), rtt_diff_mean.sqrt());

        // // Get the pruned mean: keep only the stat values inside the standard deviation (mitigation)
        // let pruned_samples = self.sync_stats.buffer.heap.iter().filter(|stat| {
        //     let item = &stat.item;
        //     let offset_diff = (item.offset_us - offset_mean).abs();
        //     let rtt_diff = (item.round_trip_delay_us - rtt_mean).abs();
        //     offset_diff <= offset_stdv + 100.0 * f64::EPSILON
        //         && rtt_diff <= rtt_stdv + 100.0 * f64::EPSILON
        // });
        // let (mut pruned_offset_mean, mut pruned_rtt_mean, pruned_sample_count) = pruned_samples
        //     .fold((0.0, 0.0, 0.0), |acc, stat| {
        //         let item = &stat.item;
        //         (
        //             acc.0 + item.offset_us,
        //             acc.1 + item.round_trip_delay_us,
        //             acc.2 + 1.0,
        //         )
        //     });
        // pruned_offset_mean /= pruned_sample_count;
        // pruned_rtt_mean /= pruned_sample_count;
        // TODO: recompute rtt_stdv from pruned ?

        FinalStats {
            rtt: Duration::from_secs_f64(rtt_mean / 1000000.0),
            // jitter is based on one-way delay, so we divide by 2
            jitter: Duration::from_secs_f64((rtt_stdv / 2.0) / 1000000.0),
        }
    }

    /// Compute the current client time; we will make sure that the client tick is ahead of the server tick
    /// Even if it is wrapped around.
    /// (i.e. if client tick is 1, and server tick is 65535, we act as if the client tick was 65537)
    /// This is because we have 2 distinct entities with wrapping: Ticks and WrappedTime
    pub(crate) fn current_prediction_time(
        &self,
        tick_manager: &TickManager,
        time_manager: &TimeManager,
    ) -> WrappedTime {
        // NOTE: careful! We know that client tick should always be ahead of server tick.
        //  let's assume that this is the case after we did tick syncing
        //  so if we are behind, that means that the client tick wrapped around.
        //  for the purposes of the sync computations, the client tick should be ahead
        let mut client_tick_raw = tick_manager.current_tick().0 as i32;
        // TODO: fix this
        // client can only be this behind server if it wrapped around...
        if (self.latest_received_server_tick.0 as i32 - client_tick_raw) > i16::MAX as i32 - 1000 {
            client_tick_raw = client_tick_raw + u16::MAX as i32;
        }
        WrappedTime::from_duration(
            tick_manager.config.tick_duration * client_tick_raw as u32 + time_manager.overstep(),
        )
    }

    /// current server time from server's point of view (using server tick)
    pub(crate) fn current_server_time(&self) -> WrappedTime {
        // TODO: instead of just using the latest_received_server_tick, there should be some sort
        //  of integration/smoothing
        self.current_server_time
    }

    /// Everytime we receive a new server update:
    /// Update the estimated current server time, computed from the time elapsed since the
    /// latest received server tick, and our estimate of the RTT
    pub(crate) fn update_current_server_time(&mut self, tick_manager: &TickManager) {
        let new_current_server_time_estimate = WrappedTime::from_duration(
            self.latest_received_server_tick.0 as u32 * tick_manager.config.tick_duration
                + self.duration_since_latest_received_server_tick
                + self.rtt() / 2,
        );
        // instead of just using the latest_received_server_tick, there should be some sort
        // of integration/smoothing
        // (in case the latest server tick is wildly off-base)
        if self.current_server_time == WrappedTime::default() {
            self.current_server_time = new_current_server_time_estimate;
        } else {
            self.current_server_time = self.current_server_time
                * self.config.current_server_time_smoothing
                + new_current_server_time_estimate
                    * (1.0 - self.config.current_server_time_smoothing);
        }
    }

    /// time at which the server would receive a packet we send now
    fn predicted_server_receive_time(&self) -> WrappedTime {
        self.current_server_time() + self.rtt() / 2
    }

    /// how far ahead of the server should I be?
    fn client_ahead_minimum(&self, tick_manager: &TickManager) -> Duration {
        self.config.jitter_multiple_margin as u32 * self.jitter()
            + self.config.tick_margin as u32 * tick_manager.config.tick_duration
    }

    pub(crate) fn estimated_interpolated_tick(&self) -> Tick {
        self.estimated_interpolation_tick
    }

    pub(crate) fn interpolation_objective(
        &self,
        // TODO: make interpolation delay part of SyncConfig?
        interpolation_delay: &InterpolationDelay,
        // TODO: should we get this via an estimate?
        server_update_rate: Duration,
        tick_manager: &TickManager,
    ) -> WrappedTime {
        // We want the interpolation time to be just a little bit behind the latest server time
        // We add `duration_since_latest_received_server_tick` because we receive them intermittently
        // TODO: maybe integrate?
        let objective_time = WrappedTime::from_duration(
            self.latest_received_server_tick.0 as u32 * tick_manager.config.tick_duration
                + self.duration_since_latest_received_server_tick,
        );
        // how much we want interpolation time to be behind the latest received server tick?
        // TODO: use a specified config margin + add std of time_between_server_updates?
        let objective_delta =
            chrono::Duration::from_std(interpolation_delay.to_duration(server_update_rate))
                .unwrap();
        return objective_time - objective_delta;
    }

    pub(crate) fn interpolation_tick(&self, tick_manager: &TickManager) -> Tick {
        Tick(
            (self.interpolation_time.elapsed_us_wrapped
                / tick_manager.config.tick_duration.as_micros() as u32) as u16,
        )
    }

    // TODO: only run when there's a change? (new server tick received or new ping received)

    // TODO: change name to make it clear that we might modify speed
    pub(crate) fn update_interpolation_time(
        &mut self,
        // TODO: make interpolation delay part of SyncConfig?
        interpolation_delay: &InterpolationDelay,
        // TODO: should we get this via an estimate?
        server_update_rate: Duration,
        tick_manager: &TickManager,
    ) {
        // for interpolation time, we don't need to use ticks (because we only need interpolation at the end
        // of the frame, not during the FixedUpdate schedule)
        let objective_time =
            self.interpolation_objective(interpolation_delay, server_update_rate, tick_manager);
        let delta = objective_time - self.interpolation_time;

        let error_margin = chrono::Duration::milliseconds(10);
        if delta > error_margin {
            // interpolation time is too far behind, speed-up!
            self.interpolation_speed_ratio = 1.0 * self.config.speedup_factor;
        } else if delta < -error_margin {
            self.interpolation_speed_ratio = 1.0 / self.config.speedup_factor;
        } else {
            self.interpolation_speed_ratio = 1.0;
        }
    }

    /// Update the client time ("upstream-throttle"): speed-up or down depending on the
    /// The objective of update-client-time is to make sure the client packets for tick T arrive on server before server reaches tick T
    /// but not too far ahead
    pub(crate) fn update_prediction_time(
        &mut self,
        time_manager: &mut TimeManager,
        tick_manager: &TickManager,
    ) {
        let rtt = self.rtt();
        let jitter = self.jitter();
        // current client time
        let current_prediction_time = self.current_prediction_time(tick_manager, time_manager);
        // time at which the server would receive a packet we send now
        // (or time at which the server's packet would arrive on the client, computed using server tick)
        let predicted_server_receive_time = self.predicted_server_receive_time();

        // how far ahead of the server am I?
        let client_ahead_delta = current_prediction_time - predicted_server_receive_time;
        // how far ahead of the server should I be?
        let client_ahead_minimum = self.client_ahead_minimum(tick_manager);

        // we want client_ahead_delta > 3 * RTT_stddev + N / tick_rate to be safe
        let error = client_ahead_delta - chrono::Duration::from_std(client_ahead_minimum).unwrap();
        let error_margin_time = chrono::Duration::from_std(
            tick_manager
                .config
                .tick_duration
                .mul_f32(self.config.error_margin),
        )
        .unwrap();

        time_manager.sync_relative_speed = if error > error_margin_time {
            debug!(
                ?rtt,
                ?jitter,
                ?current_prediction_time,
                latest_received_server_tick = ?self.latest_received_server_tick,
                client_tick = ?tick_manager.current_tick(),
                client_ahead_delta_ms = ?client_ahead_delta.num_milliseconds(),
                ?client_ahead_minimum,
                error_ms = ?error.num_milliseconds(),
                error_margin_time_ms = ?error_margin_time.num_milliseconds(),
                "Too far ahead of server! Slow down!",
            );
            // we are too far ahead of the server, slow down
            1.0 / self.config.speedup_factor
        } else if error < -error_margin_time {
            debug!(
                ?rtt,
                ?jitter,
                ?current_prediction_time,
                latest_received_server_tick = ?self.latest_received_server_tick,
                client_tick = ?tick_manager.current_tick(),
                client_ahead_delta_ms = ?client_ahead_delta.num_milliseconds(),
                ?client_ahead_minimum,
                error_ms = ?error.num_milliseconds(),
                error_margin_time_ms = ?error_margin_time.num_milliseconds(),
                "Too far behind of server! Speed up!",
            );
            // we are too far behind the server, speed up
            1.0 * self.config.speedup_factor
        } else {
            // we are within margins
            trace!("good speed");
            1.0
        };
    }

    /// Received a pong: update
    /// Returns true if we have enough pongs to finalize the handshake
    pub(crate) fn process_pong(
        &mut self,
        pong: &TimeSyncPongMessage,
        time_manager: &mut TimeManager,
        tick_manager: &mut TickManager,
    ) -> bool {
        trace!("Received time sync pong: {:?}", pong);
        let client_received_time = time_manager.current_time();

        let Some(ping_sent_time) = self.ping_store.remove(pong.ping_id) else {
            // received a ping that we were not supposed to get
            panic!("Received a ping that we were not supposed to get!");
            // return false;
        };

        // only update values for the most recent pongs received
        if pong.ping_id > self.most_recent_received_ping {
            // compute offset and round-trip delay via NTP algorithm: https://en.wikipedia.org/wiki/Network_Time_Protocol
            self.most_recent_received_ping = pong.ping_id;

            // offset
            // t1 - t0 (ping recv - ping sent)
            let ping_offset_us = (pong.ping_received_time - ping_sent_time)
                .num_microseconds()
                .unwrap();
            // t2 - t3 (pong sent - pong receive)
            let pong_offset_us = (pong.pong_sent_time - client_received_time)
                .num_microseconds()
                .unwrap();
            let offset_us = (ping_offset_us + pong_offset_us) / 2;

            // round-trip-delay
            let rtt_us = (client_received_time - ping_sent_time)
                .num_microseconds()
                .unwrap();
            let server_process_time_us = (pong.pong_sent_time - pong.ping_received_time)
                .num_microseconds()
                .unwrap();
            let round_trip_delay_us = rtt_us - server_process_time_us;

            // update stats buffer
            self.sync_stats.buffer.add_item(
                client_received_time,
                SyncStats {
                    offset_us: offset_us as f64,
                    round_trip_delay_us: round_trip_delay_us as f64,
                },
            );

            // finalize if we have enough pongs
            return !self.synced
                && self.sync_stats.buffer.len() >= self.config.handshake_pings as usize;
            // if !self.synced && self.sync_stats.buffer.len() >= self.config.handshake_pings as usize
            // {
            //     info!("received enough pongs to finalize handshake");
            //     self.synced = true;
            //     self.finalize(time_manager, tick_manager);
            // }
        }
        return false;
    }

    // Update internal time using offset so that times are synced.
    // This happens when a necessary # of handshake pongs have been recorded
    // Compute the final RTT/offset and set the client tick accordingly
    pub fn finalize(&mut self, time_manager: &mut TimeManager, tick_manager: &mut TickManager) {
        self.final_stats = self.compute_stats();
        // recompute the current server time (using the rtt we just computed)
        self.update_current_server_time(tick_manager);

        // Compute how many ticks the client must be compared to server
        let client_ideal_time =
            self.predicted_server_receive_time() + self.client_ahead_minimum(tick_manager);
        // we add 1 to get the div_ceil
        let client_ideal_tick = Tick(
            (client_ideal_time.elapsed_us_wrapped
                / tick_manager.config.tick_duration.as_micros() as u32) as u16
                + 1,
        );

        let delta_tick = client_ideal_tick - tick_manager.current_tick();
        // Update client ticks
        let latency = self.final_stats.rtt / 2;
        info!(
            ?latency,
            ?self.final_stats.jitter,
            ?delta_tick,
            ?client_ideal_tick,
            server_tick = ?self.latest_received_server_tick,
            client_current_tick = ?tick_manager.current_tick(),
            "Finished syncing!"
        );
        tick_manager.set_tick_to(client_ideal_tick)
    }
}

#[cfg(test)]
mod tests {
    use crate::tick::manager::TickConfig;
    use crate::tick::Tick;

    use super::*;

    #[test]
    fn test_send_pings() {
        let config = SyncConfig::default();
        let mut sync_manager = SyncManager::new(config);
        let mut time_manager = TimeManager::new(Duration::default());
        let mut tick_manager = TickManager::from_config(TickConfig {
            tick_duration: Duration::from_millis(50),
        });
        let interpolation_delay = InterpolationDelay::default();
        let server_update_rate = Duration::default();

        assert!(!sync_manager.is_synced());
        assert_eq!(
            sync_manager.maybe_prepare_ping(&time_manager, &tick_manager),
            None
        );

        let delta = Duration::from_millis(100);
        time_manager.update(delta, Duration::default());
        sync_manager.update(
            &mut time_manager,
            &mut tick_manager,
            &interpolation_delay,
            server_update_rate,
        );

        // send pings
        assert_eq!(
            sync_manager.maybe_prepare_ping(&time_manager, &tick_manager),
            Some(TimeSyncPingMessage {
                id: PingId(0),
                tick: Tick(0),
                ping_received_time: None,
            })
        );
        let delta = Duration::from_millis(60);
        time_manager.update(delta, Duration::default());
        sync_manager.update(
            &mut time_manager,
            &mut tick_manager,
            &interpolation_delay,
            server_update_rate,
        );

        // ping timer hasn't gone off yet, send nothing
        assert_eq!(
            sync_manager.maybe_prepare_ping(&time_manager, &tick_manager),
            None
        );
        time_manager.update(delta, Duration::default());
        sync_manager.update(
            &mut time_manager,
            &mut tick_manager,
            &interpolation_delay,
            server_update_rate,
        );
        tick_manager.set_tick_to(Tick(2));
        assert_eq!(
            sync_manager.maybe_prepare_ping(&time_manager, &tick_manager),
            Some(TimeSyncPingMessage {
                id: PingId(1),
                tick: Tick(2),
                ping_received_time: None,
            })
        );

        let delta = Duration::from_millis(100);
        time_manager.update(delta, Duration::default());
        sync_manager.update(
            &mut time_manager,
            &mut tick_manager,
            &interpolation_delay,
            server_update_rate,
        );
        assert_eq!(
            sync_manager.maybe_prepare_ping(&time_manager, &tick_manager),
            Some(TimeSyncPingMessage {
                id: PingId(2),
                tick: Tick(2),
                ping_received_time: None,
            })
        );

        // we sent all the pings we need
        assert_eq!(
            sync_manager.maybe_prepare_ping(&time_manager, &tick_manager),
            None
        );

        // check ping store
        assert_eq!(
            sync_manager.ping_store.remove(PingId(0)),
            Some(WrappedTime::new(100000))
        );
        assert_eq!(
            sync_manager.ping_store.remove(PingId(1)),
            Some(WrappedTime::new(220000))
        );
        assert_eq!(
            sync_manager.ping_store.remove(PingId(2)),
            Some(WrappedTime::new(320000))
        );

        // receive pongs
        // TODO
    }
}
