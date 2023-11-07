use lightyear_shared::{PingId, PingStore, PongMessage, TimeManager, TimeSyncPongMessage};
use std::time::Duration;

/// In charge of syncing the client's tick/time with the server's tick/time
/// right after the connection is established
pub struct HandshakeTimeManager {
    /// Number of pings to exchange with the server before finalizing the handshake
    handshake_pings: u8,
    ping_interval: Duration,
    pong_stats: Vec<SyncStats>,

    /// ping store to track which time sync pings we sent
    ping_store: PingStore,
    /// ping id corresponding to the most recent pong received
    most_recent_received_ping: PingId,
}

/// NTP algorithm stats
pub struct SyncStats {
    // clock offset: a positive value means that the client clock is faster than server clock
    offset_ms: f32,
    round_trip_delay_ms: f32,
}

impl HandshakeTimeManager {
    pub fn new(handshake_pings: u8, ping_interval: Duration) -> Self {
        Self {
            handshake_pings,
            ping_interval,
            pong_stats: Vec::new(),
            ping_store: PingStore::new(),
            // start at -1 so that any first ping is more recent
            most_recent_received_ping: PingId(u16::MAX - 1),
        }
    }

    // TODO: same as ping_manager
    pub(crate) fn prepare_ping()

    // TODO: USE KALMAN FILTERS?

    /// Received a pong: update
    /// Returns true if we have enough pongs to finalize the handshake
    pub(crate) fn process_pong(
        &mut self,
        pong: &TimeSyncPongMessage,
        time_manager: &TimeManager,
    ) -> bool {
        let client_received_time = time_manager.current_time();

        let Some(ping_sent_time) = self.ping_store.remove(pong.ping_id) else {
            panic!("unknown ping id");
        };

        // only update values for the most recent pongs received
        if pong.ping_id > self.most_recent_received_ping {
            // compute offset and round-trip delay via NTP algorithm: https://en.wikipedia.org/wiki/Network_Time_Protocol
            self.most_recent_received_ping = pong.ping_id;

            // offset
            // t1 - t0 (ping recv - ping sent)
            let ping_offset_ms = (pong.ping_received_time - ping_sent_time).as_millis() as i32;
            // t2 - t3 (pong sent - pong receive)
            let pong_offset_ms = -((client_received_time - pong.pong_sent_time).as_millis() as i32);
            let offset_ms = (ping_offset_ms + pong_offset_ms) / 2;

            // round-trip-delay
            let rtt_ms = (client_received_time - ping_sent_time).as_millis() as u32;
            let server_process_time_ms =
                (pong.pong_sent_time - pong.ping_received_time).as_millis() as u32;
            let round_trip_delay_ms = rtt_ms - server_process_time_ms;

            // update stats buffer
            self.pong_stats.push(SyncStats {
                offset_ms: offset_ms as f32,
                round_trip_delay_ms: round_trip_delay_ms as f32,
            });

            // finalize if we have enough pongs
            if self.pong_stats.len() >= self.handshake_pings as usize {
                return true;
            }
        }
        return false;
    }

    // This happens when a necessary # of handshake pongs have been recorded
    pub fn finalize(mut self, time_manager: &mut TimeManager) {
        let sample_count = self.pong_stats.len() as f32;

        let stats = std::mem::take(&mut self.pong_stats);

        // Find the Mean
        let mut offset_mean = 0.0;
        let mut rtt_mean = 0.0;

        for stat in &stats {
            offset_mean += *stat.offset_ms;
            rtt_mean += *stat.round_trip_delay_ms;
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
            if offset_diff < offset_stdv && rtt_diff < rtt_stdv {
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

        // Update internal time using offset
        // TODO: RECHECK THIS!
        // client time is slower than server time
        if pruned_offset_mean < 0.0 {
            let offset_ms = (pruned_offset_mean * -1.0) as u32;
            time_manager.subtract_millis(offset_ms)
        } else {
            // client time is faster than server time,
            let offset_ms = pruned_offset_mean as u32;
            time_manager.update(Duration::from_millis(offset_ms as u64));
        }

        // Clear out outstanding pings
        self.ping_store.clear();

        // Update client ticks
    }
}
