use std::time::Duration;

use bevy::prelude::Timer;
use bevy::time::TimerMode;
use chrono::Duration as ChronoDuration;
use tracing::{info, trace};

use crate::{
    PingId, PingStore, TickManager, TimeManager, TimeSyncPingMessage, TimeSyncPongMessage,
};

/// In charge of syncing the client's tick/time with the server's tick/time
/// right after the connection is established
pub struct SyncManager {
    /// Number of pings to exchange with the server before finalizing the handshake
    handshake_pings: u8,
    current_handshake: u8,
    /// Time interval between every ping we send
    ping_interval: Duration,
    /// Timer to send regular pings to server
    ping_timer: Timer,
    pong_stats: Vec<SyncStats>,

    /// ping store to track which time sync pings we sent
    ping_store: PingStore,
    /// ping id corresponding to the most recent pong received
    most_recent_received_ping: PingId,
    /// whether the handshake is finalized
    synced: bool,
}

/// NTP algorithm stats
pub struct SyncStats {
    // clock offset: a positive value means that the client clock is faster than server clock
    pub(crate) offset_ms: f32,
    pub(crate) round_trip_delay_ms: f32,
}

impl SyncManager {
    pub fn new(handshake_pings: u8, ping_interval: Duration) -> Self {
        Self {
            handshake_pings,
            current_handshake: 0,
            ping_interval,
            ping_timer: Timer::new(ping_interval, TimerMode::Repeating),
            pong_stats: Vec::new(),
            ping_store: PingStore::new(),
            // start at -1 so that any first ping is more recent
            most_recent_received_ping: PingId(u16::MAX - 1),
            synced: false,
        }
    }

    pub(crate) fn update(&mut self, delta: Duration) {
        self.ping_timer.tick(delta);
    }

    pub(crate) fn is_synced(&self) -> bool {
        self.synced
    }

    // TODO: same as ping_manager
    #[cold]
    pub(crate) fn maybe_prepare_ping(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
    ) -> Option<TimeSyncPingMessage> {
        if !self.synced && (self.ping_timer.finished() || self.current_handshake == 0) {
            self.current_handshake += 1;
            self.ping_timer.reset();

            let ping_id = self
                .ping_store
                .push_new(time_manager.current_time().clone());

            // TODO: for rtt purposes, we could just send a ping that has no tick info
            // PingMessage::new(ping_id, time_manager.current_tick())
            return Some(TimeSyncPingMessage {
                id: ping_id,
                tick: tick_manager.current_tick(),
                ping_received_time: None,
            });

            // let message = ProtocolMessage::Sync(SyncMessage::Ping(ping));
            // let channel = ChannelKind::of::<DefaultUnreliableChannel>();
            // connection.message_manager.buffer_send(message, channel)
        }
        None
    }

    // TODO: USE KALMAN FILTERS?

    /// Received a pong: update
    /// Returns true if we have enough pongs to finalize the handshake
    #[cold]
    pub(crate) fn process_pong(
        &mut self,
        pong: &TimeSyncPongMessage,
        time_manager: &mut TimeManager,
        tick_manager: &mut TickManager,
    ) {
        trace!("Received time sync pong: {:?}", pong);
        let client_received_time = time_manager.current_time();

        let Some(ping_sent_time) = self.ping_store.remove(pong.ping_id) else {
            // received a ping that we were not supposed to get
            return;
        };

        // only update values for the most recent pongs received
        if pong.ping_id > self.most_recent_received_ping {
            // compute offset and round-trip delay via NTP algorithm: https://en.wikipedia.org/wiki/Network_Time_Protocol
            self.most_recent_received_ping = pong.ping_id;

            // offset
            // t1 - t0 (ping recv - ping sent)
            let ping_offset_ms =
                (pong.ping_received_time - ping_sent_time).num_milliseconds() as i32;
            // t2 - t3 (pong sent - pong receive)
            let pong_offset_ms =
                (pong.pong_sent_time - client_received_time).num_milliseconds() as i32;
            let offset_ms = (ping_offset_ms + pong_offset_ms) / 2;

            // round-trip-delay
            let rtt_ms = (client_received_time - ping_sent_time).num_milliseconds() as u32;
            let server_process_time_ms =
                (pong.pong_sent_time - pong.ping_received_time).num_milliseconds() as u32;
            let round_trip_delay_ms = rtt_ms - server_process_time_ms;

            // update stats buffer
            self.pong_stats.push(SyncStats {
                offset_ms: offset_ms as f32,
                round_trip_delay_ms: round_trip_delay_ms as f32,
            });

            // finalize if we have enough pongs
            if self.pong_stats.len() >= self.handshake_pings as usize {
                info!("received enough pongs to finalize handshake");
                self.synced = true;
                self.finalize(time_manager, tick_manager);
            }
        }
    }

    // This happens when a necessary # of handshake pongs have been recorded
    // Compute the final RTT/offset and set the client tick accordingly
    pub fn finalize(&mut self, time_manager: &mut TimeManager, tick_manager: &mut TickManager) {
        let sample_count = self.pong_stats.len() as f32;

        let stats = std::mem::take(&mut self.pong_stats);

        // Find the Mean
        let mut offset_mean = 0.0;
        let mut rtt_mean = 0.0;

        for stat in &stats {
            offset_mean += stat.offset_ms;
            rtt_mean += stat.round_trip_delay_ms;
        }

        offset_mean /= sample_count;
        rtt_mean /= sample_count;

        // Find the Variance
        let mut offset_diff_mean = 0.0;
        let mut rtt_diff_mean = 0.0;

        for stat in &stats {
            offset_diff_mean += (stat.offset_ms - offset_mean).powi(2);
            rtt_diff_mean += (stat.round_trip_delay_ms - rtt_mean).powi(2);
        }

        offset_diff_mean /= sample_count;
        rtt_diff_mean /= sample_count;

        // Find the Standard Deviation
        let offset_stdv = offset_diff_mean.sqrt();
        let rtt_stdv = rtt_diff_mean.sqrt();

        // Keep only the stat values inside the standard deviation (mitigation)
        let mut pruned_stats = Vec::new();
        for stat in &stats {
            let offset_diff = (stat.offset_ms - offset_mean).abs();
            let rtt_diff = (stat.round_trip_delay_ms - rtt_mean).abs();
            if offset_diff <= offset_stdv && rtt_diff <= rtt_stdv {
                pruned_stats.push(stat);
            }
        }

        // Find the mean of the pruned stats
        let pruned_sample_count = pruned_stats.len() as f32;
        let mut pruned_offset_mean = 0.0;
        let mut pruned_rtt_mean = 0.0;

        for stat in pruned_stats {
            pruned_offset_mean += stat.offset_ms;
            pruned_rtt_mean += stat.round_trip_delay_ms;
        }

        pruned_offset_mean /= pruned_sample_count;
        pruned_rtt_mean /= pruned_sample_count;

        // Update internal time using offset so that times are synced.
        // TODO: should we sync client/server time, or should we set client time to server_time + tick_delta?
        // TODO: does this algorithm work when client time is slowed/sped-up?

        // negative offset: client time (11am) is ahead of server time (10am)
        // positive offset: server time (11am) is ahead of client time (10am)
        // info!("Apply offset to client time: {}ms", pruned_offset_mean);
        time_manager.set_current_time(
            time_manager.current_time() + ChronoDuration::milliseconds(pruned_offset_mean as i64),
        );

        // Clear out outstanding pings
        self.ping_store.clear();

        // Compute how many ticks the client must be compared to server
        let latency_ms = (pruned_rtt_mean / 2.0) as u32;
        // TODO: recompute rtt_stdv from pruned ?
        let jitter_ms = (rtt_stdv / 2.0 * 3.0) as u32;
        let delta_ms =
            latency_ms + jitter_ms + tick_manager.config.tick_duration.as_millis() as u32;

        let delta_tick = delta_ms as u16 / tick_manager.config.tick_duration.as_millis() as u16;
        // Update client ticks
        info!(
            offset_ms = ?pruned_offset_mean,
            ?latency_ms,
            ?jitter_ms,
            ?delta_tick,
            "Finished syncing!"
        );
        tick_manager.increment_tick_by(delta_tick)
    }
}

#[cfg(test)]
mod tests {
    use crate::tick::Tick;
    use crate::{TickConfig, WrappedTime};

    use super::*;

    #[test]
    fn test_sync() {
        let mut sync_manager = SyncManager::new(3, Duration::from_millis(100));
        let mut time_manager = TimeManager::new();
        let mut tick_manager = TickManager::from_config(TickConfig {
            tick_duration: Duration::from_millis(50),
        });

        assert!(!sync_manager.is_synced());

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
        sync_manager.update(delta);
        time_manager.update(delta);

        // ping timer hasn't gone off yet, send nothing
        assert_eq!(
            sync_manager.maybe_prepare_ping(&time_manager, &tick_manager),
            None
        );
        sync_manager.update(delta);
        time_manager.update(delta);
        tick_manager.increment_tick_by(2);
        assert_eq!(
            sync_manager.maybe_prepare_ping(&time_manager, &tick_manager),
            Some(TimeSyncPingMessage {
                id: PingId(1),
                tick: Tick(2),
                ping_received_time: None,
            })
        );

        let delta = Duration::from_millis(100);
        sync_manager.update(delta);
        time_manager.update(delta);
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
            Some(WrappedTime::new(0))
        );
        assert_eq!(
            sync_manager.ping_store.remove(PingId(1)),
            Some(WrappedTime::new(120000))
        );
        assert_eq!(
            sync_manager.ping_store.remove(PingId(2)),
            Some(WrappedTime::new(220000))
        );

        // receive pongs
        // TODO
    }
}
