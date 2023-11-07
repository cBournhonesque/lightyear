use bevy::prelude::{Timer, TimerMode};
use lightyear_shared::connection::ProtocolMessage;
use lightyear_shared::tick::Tick;
use lightyear_shared::{
    ChannelKind, Connection, DefaultUnreliableChannel, MessageManager, PingMessage, PingStore,
    PongMessage, Protocol, SyncMessage, TimeManager, WrappedTime,
};
use std::time::Duration;
use chrono::Duration as ChronoDuration;
use crate::sync::SyncStats;

// TODO: this could be used by client or by server?
/// Contains Config properties which will be used by a Server or Client
#[derive(Clone, Debug)]
pub struct PingConfig {
    /// The duration to wait before sending a ping message to the remote host,
    /// in order to estimate RTT time
    pub ping_interval_ms: Duration,
    /// The initial estimate for the RTT
    pub rtt_ms_initial_estimate: Duration,
    /// The initial estimate for Jitter
    pub jitter_ms_initial_estimate: Duration,
    /// Factor to smooth out estimate of RTT. A higher number will
    /// smooth out measurements, but at the cost of responsiveness
    pub rtt_smoothing_factor: f32,

}

pub struct Stats {
    pruned_offset_avg: f32,
    raw_offset_avg: f32,
    offset_stdv: f32,
    pruned_rtt_avg: f32,
    raw_rtt_avg: f32,
    rtt_stdv: f32,
}

impl Stats {
    /// Receive new offset/rtt from a pong message, update stats
    fn process_stats(&mut self, offset_millis: i32, rtt_millis: u32) {
        let offset_sample = offset_millis as f32;
        let rtt_sample = rtt_millis as f32;

        self.raw_offset_avg = (0.9 * self.raw_offset_avg) + (0.1 * offset_sample);
        self.raw_rtt_avg = (0.9 * self.raw_rtt_avg) + (0.1 * rtt_sample);

        let offset_diff = offset_sample - self.raw_offset_avg;
        let rtt_diff = rtt_sample - self.raw_rtt_avg;

        self.offset_stdv = ((0.9 * self.offset_stdv.powi(2)) + (0.1 * offset_diff.powi(2))).sqrt();
        self.rtt_stdv = ((0.9 * self.rtt_stdv.powi(2)) + (0.1 * rtt_diff.powi(2))).sqrt();

        if offset_diff.abs() < self.offset_stdv && rtt_diff.abs() < self.rtt_stdv {
            self.pruned_offset_avg = (0.9 * self.pruned_offset_avg) + (0.1 * offset_sample);
            self.pruned_rtt_avg = (0.9 * self.pruned_rtt_avg) + (0.1 * rtt_sample);
        } else {
            // Pruned out sample
        }
    }
}

impl Default for PingConfig {
    fn default() -> Self {
        Self {
            ping_interval_ms: Duration::from_millis(100),
            rtt_ms_initial_estimate: Duration::from_millis(10),
            jitter_ms_initial_estimate: Default::default(),
            rtt_smoothing_factor: 0.0,
        }
    }
}

// TODO: right now this does 2 things
// - PingManager: send ping/pongs and compute offset/rtt/jitter (and possibly ask TimeManager to speed/slow time)
// - TickManager: compute ticks based on data

// TODO: this should be associated with a connection? maybe we need Server/Client connection and BaseConnection
// 2 objectives:
// - handle sending regular pings to clients to estimate rtt/jitter/loss
// - handle receiving pings from clients and sending back pongs
pub struct PingManager {
    pub rtt_ms_average: f32,
    pub jitter_ms_average: f32,
    /// Timer to send regular pings to clients
    ///
    ping_timer: Timer,
    sent_pings: PingStore,

    pub stats: Stats,

    // current client tick
    pub client_tick: Tick,

    // if server misses a packet
    // that might mean the buffer is too small!
    // we can speed up the client so that we are sure that messages for tick T are received in server before tick T arrives
    // (the tick_delta doesn't change based on time speed so we will reach tick T + D faster on client)

    // difference between server tick and client tick
    // if the difference should change (because latency/jitter changes, we speed-up/slow-down client?
    pub tick_delta: u32,

    /// The tick that we think the client should be at when it runs "receive" (when running PreUpdate)
    pub client_receiving_tick: Tick,
    /// The tick that we think the client should be at when it runs "send" (when running PostUpdate)
    pub client_sending_tick: Tick,
    ///The tick that we think would be at when it receives a message sent now
    pub server_receivable_tick: Tick,
    client_receiving_instant: WrappedTime,
    client_sending_instant: WrappedTime,
    /// Soonest instant where server should be able to receive a message sent now
    server_receivable_instant: WrappedTime,
}

impl PingManager {
    pub fn new(ping_config: &PingConfig, time_manager: &TimeManager, round_trip_delay_ms: i32, rtt_stdv: i32) -> Self {
        let now = time_manager.current_time();
        let latency_ms = (round_trip_delay_ms / 2.0) as u32;
        let major_jitter_ms = (rtt_stdv / 2.0 * 3.0) as u32;
        // let tick_duration_ms = server_tick_duration_avg.round() as u32;

        // let rtt_ms_average = ping_config.rtt_ms_initial_estimate.as_secs_f32() * 1000.0;
        // let jitter_ms_average = ping_config.jitter_ms_initial_estimate.as_secs_f32() * 1000.0;

        // NOTE: We want the client tick to be ahead of the server tick by tick_delta
        // (so that commands sent from the client will be received right one time by the server)
        // TODO: use tick_duration or server_tick_duration_avg?
        let tick_delta_ms = latency_ms + major_jitter_ms + tick_duration;


        let server_receivable_instant = get_server_receivable_target()

        PingManager {
            rtt_ms_average,
            jitter_ms_average,
            ping_timer: Timer::new(ping_config.ping_interval_ms, TimerMode::Repeating),
            sent_pings: PingStore::new(),
        }
    }

    // TODO: is this correct? maybe we always want client_ticks to progress monotonically?

    // When we receive a pong, time at server is last_tick + last_tick_instant
    // set client_tick to last_tick + last_tick_instant - tick_delta
    //
    // when we don't receive pongs, just increment client_ticks by 1 once we cross tick duration
    pub fn update(&mut self, time_manager: &TimeManager) -> bool {
        let time_offset = time_manager.current_time() - self.last_tick_wrapped_time;
        if time_offset > self.config.tick_duration {
            // TODO: compute the actual tick duration
            self.client_tick += 1;
            self.last_tick_wrapped_time = time_manager.current_time();
            return true;
        }
        return false;
    }

    /// Returns whether a ping message should be sent
    pub fn should_send_ping(&self) -> bool {
        self.ping_timer.finished()
    }

    /// Buffer a ping message (reset the timer)
    // TODO: issues here: we would like to send the ping message immediately, otherwise the recorded current time is incorrect
    //   - can give infinity priority to this channel?
    //   - can write directly to io otherwise?
    pub fn prepare_ping(&mut self, time_manager: &TimeManager) -> PingMessage {
        self.ping_timer.reset();

        let ping_id = self.sent_pings.push_new(time_manager.current_time());

        // TODO: for rtt purposes, we could just send a ping that has no tick info
        // PingMessage::new(ping_id, time_manager.current_tick())
        PingMessage::new(ping_id, Tick(0))

        // let message = ProtocolMessage::Sync(SyncMessage::Ping(ping));
        // let channel = ChannelKind::of::<DefaultUnreliableChannel>();
        // connection.message_manager.buffer_send(message, channel)
    }

    /// Buffer a pong message (in response to a ping)
    // TODO: issues here: we would like to send the ping message immediately, otherwise the recorded current time is incorrect
    //   - can give infinity priority to this channel?
    //   - can write directly to io otherwise?
    pub fn prepare_pong(&mut self, time_manager: &TimeManager, ping: PingMessage) -> PongMessage {
        // TODO: for rtt purposes, we could just send a ping that has no tick info
        PongMessage {
            ping_id: ping.id,
            tick: Default::default(),
            offset_sec: 0.0,
        }
        // let message = ProtocolMessage::Sync(SyncMessage::Ping(ping));
        // let channel = ChannelKind::of::<DefaultUnreliableChannel>();
        // connection.message_manager.buffer_send(message, channel)
    }

    /// Process an incoming pong payload
    pub fn process_pong(&mut self, pong: &PongMessage, time_manager: &TimeManager) {
        // TODO: update offset/rtt
        // TODO: update tick delta

        if let Some(ping_sent_time) = self.sent_pings.remove(pong.ping_id) {
            assert!(time_manager.current_time() > ping_sent_time);
            let rtt_millis = (time_manager.current_time() - ping_sent_time).as_secs_f32() * 1000.0;
            let new_jitter = ((rtt_millis - self.rtt_ms_average) / 2.0).abs();
            // TODO: use rtt_smoothing_factor?
            self.rtt_ms_average = (0.9 * self.rtt_ms_average) + (0.1 * rtt_millis);
            self.jitter_ms_average = (0.9 * self.jitter_ms_average) + (0.1 * new_jitter);

            #[cfg(feature = "metrics")]
            {
                metrics::increment_gauge!("rtt_ms_average", self.rtt_ms_average as f64);
                metrics::increment_gauge!("jitter_ms_average", self.jitter_ms_average as f64);
            }
        }
    }
}

/// Earliest instant at which the server should be able to receive a message sent now
/// - add latency
/// - remove jitter
/// TODO: understand adding tick duration?
/// - add 1 tick (account for client receive -> client send delay)
/// - add 1 tick (account for server receiving the message but having to wait until next frame's receive)
/// - add 1 tick as a buffer?
fn get_server_receivable_target(
    now: &WrappedTime,
    latency: u32,
    jitter: u32,
    tick_duration: u32,
) -> WrappedTime {
    ///
    let millis = (((latency + (tick_duration * 2)) as i32) - (jitter as i32)).max(0) as u32;
    now + ChronoDuration::milliseconds(millis as i64)
}

/// Client tick should be in the 'future' compared to server
/// This is the time at which the server should send a message so that the client receives it now
/// - remove latency
/// - remove jitter
/// - remove one tick as a buffer?
fn get_client_receiving_target(
    now: &WrappedTime,
    latency: u32,
    jitter: u32,
    tick_duration: u32,
) -> WrappedTime {
    let millis = latency + jitter + tick_duration;
    now - ChronoDuration::milliseconds(millis as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Time;
    use lightyear_shared::{PingId, TickConfig};
    #[test]
    fn test_ping_manager() {
        let ping_config = PingConfig {
            ping_interval_ms: Duration::from_millis(100),
            rtt_ms_initial_estimate: Duration::from_millis(10),
            jitter_ms_initial_estimate: Default::default(),
            rtt_smoothing_factor: 0.0,
        };
        let mut ping_manager = PingManager::new(&ping_config);
        let tick_config = TickConfig::new(Duration::from_millis(16));
        let mut time_manager = TimeManager::new();

        assert!(!ping_manager.should_send_ping());
        let delta = Duration::from_millis(100);
        ping_manager.update(delta);
        time_manager.update(delta);
        assert!(ping_manager.should_send_ping());

        let ping_message = ping_manager.prepare_ping(&time_manager);
        assert!(!ping_manager.should_send_ping());
        assert_eq!(ping_message.id, PingId(0));

        let delta = Duration::from_millis(20);
        ping_manager.update(delta);
        time_manager.update(delta);
        let pong_message = PongMessage {
            ping_id: PingId(0),
            tick: Default::default(),
            offset_sec: 0.0,
        };
        ping_manager.process_pong(&pong_message, &time_manager);

        assert_eq!(ping_manager.rtt_ms_average, 0.9 * 10.0 + 0.1 * 20.0);
        assert_eq!(ping_manager.jitter_ms_average, 0.9 * 0.0 + 0.1 * 5.0);
    }
}
