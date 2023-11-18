use std::time::Duration;

use bevy::prelude::Timer;
use bevy::time::TimerMode;
use chrono::Duration as ChronoDuration;
use tracing::{info, trace};

use crate::packet::packet::PacketId;
use crate::tick::Tick;
use crate::{
    PingId, PingStore, ReadyBuffer, TickManager, TimeManager, TimeSyncPingMessage,
    TimeSyncPongMessage, WrappedTime,
};

pub struct SyncConfig {
    /// How much multiple of jitter do we apply as margin when computing the time
    /// a packet will get received by the server
    /// (worst case will be RTT / 2 + jitter * multiple_margin)
    pub jitter_multiple_margin: u8,
    pub tick_margin: u8,
    pub sync_ping_interval: Duration,
    /// Number of pings to exchange with the server before finalizing the handshake
    pub handshake_pings: u8,
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            jitter_multiple_margin: 3,
            sync_ping_interval: Duration::from_millis(100),
            handshake_pings: 10,
        }
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
    current_handshake: u8,

    /// Timer to send regular pings to server
    ping_timer: Timer,
    sync_stats: SyncStatsBuffer,
    /// Current best estimates of various networking statistics
    final_stats: FinalStats,

    /// sent packet store to track the time we sent each packet
    sent_packet_store: SentPacketStore,
    /// ping store to track which time sync pings we sent
    ping_store: PingStore,
    /// ping id corresponding to the most recent pong received
    most_recent_received_ping: PingId,
    // TODO: see if this is correct; should we instead attach the tick on every update message?
    /// Tick of the server that we last received in any packet from the server.
    /// This is not updated every tick, but only when we receive a packet from the server.
    /// (usually every frame)
    pub(crate) latest_received_server_tick: Tick,
    /// whether the handshake is finalized
    synced: bool,
}

/// The final stats that we care about
#[derive(Default)]
pub struct FinalStats {
    pub rtt_ms: f32,
    pub jitter_ms: f32,
}

/// NTP algorithm stats
pub struct SyncStats {
    // clock offset: a positive value means that the client clock is faster than server clock
    pub(crate) offset_ms: f32,
    pub(crate) round_trip_delay_ms: f32,
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

impl SyncManager {
    pub fn new(config: SyncConfig) -> Self {
        Self {
            config,
            current_handshake: 0,
            ping_timer: Timer::new(config.sync_ping_interval.clone(), TimerMode::Repeating),
            sync_stats: SyncStatsBuffer::new(),
            // sent_packet_store: SentPacketStore::new(),
            final_stats: FinalStats::default(),
            sent_packet_store: SentPacketStore::default(),
            ping_store: PingStore::new(),
            // start at -1 so that any first ping is more recent
            most_recent_received_ping: PingId(u16::MAX - 1),
            latest_received_server_tick: Tick(0),
            synced: false,
        }
    }

    pub fn rtt(&self) -> f32 {
        self.final_stats.rtt_ms
    }

    pub fn jitter(&self) -> f32 {
        self.final_stats.jitter_ms
    }

    pub(crate) fn update(&mut self, time_manager: &TimeManager) {
        self.ping_timer.tick(time_manager.delta());

        if self.synced {
            // clear stats that are older than a threshold, such as 2 seconds
            let oldest_time = time_manager.current_time() - ChronoDuration::seconds(2);
            self.sync_stats.buffer.pop_until(&oldest_time);

            // recompute RTT jitter from the last 2-seconds of stats
            self.final_stats = self.compute_stats();
        }
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
        if self.ping_timer.finished() || self.current_handshake == 0 {
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

    // TODO:
    // - for efficiency, we want to use a rolling mean/std algorithm
    // - every N seconds (for example 2 seconds), we clear the buffer for stats older than 2 seconds and recompute mean/std from the remaining elements
    /// Compute the stats (offset, rtt, jitter) from the stats present in the buffer
    pub fn compute_stats(&mut self) -> FinalStats {
        let sample_count = self.sync_stats.len() as f32;

        // Find the Mean
        let (offset_mean, rtt_mean) =
            self.sync_stats
                .buffer
                .heap
                .iter()
                .fold((0.0, 0.0), |acc, stat| {
                    let item = &stat.item;
                    (
                        acc.0 + item.offset_ms / sample_count,
                        acc.1 + item.round_trip_delay_ms / sample_count,
                    )
                });

        // TODO: should I use biased or not?
        // Find the Variance (use the unbiased estimator)
        let (offset_diff_mean, rtt_diff_mean) =
            self.sync_stats
                .buffer
                .heap
                .iter()
                .fold((0.0, 0.0), |acc, stat| {
                    let item = &stat.item;
                    (
                        acc.0 + (item.offset_ms - offset_mean).powi(2) / (sample_count - 1),
                        acc.1 + (item.round_trip_delay_ms - rtt_mean).powi(2) / (sample_count - 1),
                    )
                });

        // Find the Standard Deviation
        let (offset_stdv, rtt_stdv) = (offset_diff_mean.sqrt() as f32, rtt_diff_mean.sqrt() as f32);

        // Get the pruned mean: keep only the stat values inside the standard deviation (mitigation)
        let pruned_samples = self.sync_stats.buffer.heap.iter().filter(|stat| {
            let item = &stat.item;
            let offset_diff = (item.offset_ms - offset_mean).abs();
            let rtt_diff = (item.round_trip_delay_ms - rtt_mean).abs();
            offset_diff <= offset_stdv && rtt_diff <= rtt_stdv
        });
        let pruned_sample_count = pruned_samples.len() as f32;
        let (pruned_offset_mean, pruned_rtt_mean) = pruned_samples.fold((0.0, 0.0), |acc, stat| {
            let item = &stat.item;
            (
                acc.0 + item.offset_ms / pruned_sample_count,
                acc.1 + item.round_trip_delay_ms / pruned_sample_count,
            )
        });
        // TODO: recompute rtt_stdv from pruned ?

        FinalStats {
            rtt_ms: pruned_rtt_mean,
            // jitter is based on one-way delay, so we divide by 2
            jitter_ms: rtt_stdv / 2,
        }
    }

    // TODO:
    // - on client, when we send a packet, we record its instant
    //   when we receive a packet, we check its acks. if the ack is one of the packets we sent, we use that
    //   to update our RTT estimate
    // TODO: when we receive a packet on the client, we check the acks and we learn when the packet

    /// Update the client time ("upstream-throttle"): speed-up or down depending on the
    pub(crate) fn update_client_time(
        &mut self,
        time_manager: &mut TimeManager,
        tick_manager: &mut TickManager,
    ) {
        // The objective of update-client-time is to make sure the client packets for tick T arrive on server before server reaches tick T
        // but not too far ahead

        let overstep = time_manager.overstep();
        let current_client_time =
            (tick_manager.current_tick() / tick_manager.config.tick_duration) + overstep;

        let duration_since_last_received_server_tick = 0.0;
        let current_server_time = (self.latest_received_server_tick
            / tick_manager.config.tick_duration)
            + duration_since_last_received_server_tick
            + self.rtt();

        let current_rtt_pred = 0.0;
        let current_jitter_pred = 0.0;

        // time at which the server would receive a packet we send now
        let time_server_receive = current_server_time + current_rtt_pred;
        // how far ahead of the server am I?
        let client_ahead_delta = current_client_time - time_server_receive;
        // how far ahead of the server should I be?

        let client_ahead_minimum = self.config.jitter_multiple_margin * self.jitter()
            + N / tick_manager.config.tick_duration;
        // we want client_ahead_delta > 3 * RTT_stddev + N / tick_rate to be safe
        let error = client_head_delta - client_ahead_minimum;
        if error > epsilon {
            // we are too far ahead of the server, slow down
        } else {
            // we are too far behind the server, speed up
        }
    }

    /// Received a pong: update
    /// Returns true if we have enough pongs to finalize the handshake
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
            self.sync_stats.buffer.add_item(
                client_received_time,
                SyncStats {
                    offset_ms: offset_ms as f32,
                    round_trip_delay_ms: round_trip_delay_ms as f32,
                },
            );

            // finalize if we have enough pongs
            if self.sync_stats.len() >= self.config.handshake_pings as usize {
                info!("received enough pongs to finalize handshake");
                self.synced = true;
                self.finalize(time_manager, tick_manager);
            }
        }
    }

    // This happens when a necessary # of handshake pongs have been recorded
    // Compute the final RTT/offset and set the client tick accordingly
    pub fn finalize(&mut self, time_manager: &mut TimeManager, tick_manager: &mut TickManager) {
        self.final_stats = self.compute_stats();

        // Update internal time using offset so that times are synced.
        // TODO: should we sync client/server time, or should we set client time to server_time + tick_delta?
        // TODO: does this algorithm work when client time is slowed/sped-up?

        // negative offset: client time (11am) is ahead of server time (10am)
        // positive offset: server time (11am) is ahead of client time (10am)
        // info!("Apply offset to client time: {}ms", pruned_offset_mean);

        // time_manager.set_current_time(
        //     time_manager.current_time() + ChronoDuration::milliseconds(pruned_offset_mean as i64),
        // );

        // Clear out outstanding pings
        self.ping_store.clear();

        // Compute how many ticks the client must be compared to server
        let latency_ms = (self.final_stats.rtt_ms / 2.0) as u32;
        let delta_ms = latency_ms
            + self.final_stats.jitter_ms * self.config.jitter_multiple_margin
            + tick_manager.config.tick_duration.as_millis() as u32;

        let delta_tick = delta_ms as u16 / tick_manager.config.tick_duration.as_millis() as u16;
        // Update client ticks
        info!(
            offset_ms = ?pruned_offset_mean,
            ?latency_ms,
            ?jitter_ms,
            ?delta_tick,
            "Finished syncing!"
        );
        tick_manager.set_tick_to(self.latest_received_server_tick + Tick(delta_tick))
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
