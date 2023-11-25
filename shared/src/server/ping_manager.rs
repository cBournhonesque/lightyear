use std::time::Duration;

use bevy::prelude::{Timer, TimerMode};

use crate::tick::Tick;
use crate::{
    PingMessage, PingStore, PongMessage, Protocol, TickManager, TimeManager, TimeSyncPingMessage,
    TimeSyncPongMessage,
};

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

// TODO: this should be associated with a connection? maybe we need Server/Client connection and BaseConnection
// 2 objectives:
// - handle sending regular pings to clients to estimate rtt/jitter/loss
// - handle receiving pings from clients and sending back pongs
pub struct PingManager {
    pub rtt_ms_average: f32,
    pub jitter_ms_average: f32,
    /// Timer to send regular pings to clients
    ping_timer: Timer,
    sent_pings: PingStore,
    /// We received time-sync pongs; we keep track that we will have to send pongs back when we can
    /// (when the connection's send_timer is ready)
    pongs_to_send: Vec<TimeSyncPingMessage>,
}

impl PingManager {
    pub fn new(ping_config: &PingConfig) -> Self {
        let rtt_ms_average = ping_config.rtt_ms_initial_estimate.as_secs_f32() * 1000.0;
        let jitter_ms_average = ping_config.jitter_ms_initial_estimate.as_secs_f32() * 1000.0;

        PingManager {
            rtt_ms_average,
            jitter_ms_average,
            ping_timer: Timer::new(ping_config.ping_interval_ms, TimerMode::Repeating),
            sent_pings: PingStore::new(),
            pongs_to_send: Vec::new(),
        }
    }

    pub fn update(&mut self, delta: Duration) {
        self.ping_timer.tick(delta);
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

    /// Prepare a time sync pong message (in response to a ping)
    // TODO: issues here: we would like to send the ping message immediately, otherwise the recorded current time is incorrect
    //   - can give infinity priority to this channel?
    //   - can write directly to io otherwise?
    pub fn prepare_sync_pong(
        &mut self,
        time_manager: &TimeManager,
        tick_manager: &TickManager,
        ping: TimeSyncPingMessage,
    ) -> TimeSyncPongMessage {
        // TODO: for rtt purposes, we could just send a ping that has no tick info
        TimeSyncPongMessage {
            ping_id: ping.id,
            // server_tick_instant: WrappedTime::new(0),
            // server_tick: tick_manager.current_tick(),
            ping_received_time: ping.ping_received_time.unwrap(),
            // TODO: can we get a more precise time? (based on real)?
            // TODO: otherwise we can consider that there's an entire tick duration between receive and sent
            pong_sent_time: time_manager.current_time(),
        }
        // let message = ProtocolMessage::Sync(SyncMessage::Ping(ping));
        // let channel = ChannelKind::of::<DefaultUnreliableChannel>();
        // connection.message_manager.buffer_send(message, channel)
    }

    /// Process an incoming pong payload
    pub fn process_pong(&mut self, pong: PongMessage, time_manager: &TimeManager) {
        if let Some(ping_sent_time) = self.sent_pings.remove(pong.ping_id) {
            assert!(time_manager.current_time() > ping_sent_time);
            let rtt_millis =
                (time_manager.current_time() - ping_sent_time).num_milliseconds() as f32;
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

    pub(crate) fn buffer_sync_ping(&mut self, ping: TimeSyncPingMessage) {
        self.pongs_to_send.push(ping)
    }

    pub(crate) fn client_pings_pending_pong(&mut self) -> Vec<TimeSyncPingMessage> {
        std::mem::take(&mut self.pongs_to_send)
    }
}

#[cfg(test)]
mod tests {
    use crate::PingId;

    use super::*;

    #[test]
    fn test_ping_manager() {
        let ping_config = PingConfig {
            ping_interval_ms: Duration::from_millis(100),
            rtt_ms_initial_estimate: Duration::from_millis(10),
            jitter_ms_initial_estimate: Default::default(),
            rtt_smoothing_factor: 0.0,
        };
        let mut ping_manager = PingManager::new(&ping_config);
        // let tick_config = TickConfig::new(Duration::from_millis(16));
        let mut time_manager = TimeManager::new(Duration::default());

        assert!(!ping_manager.should_send_ping());
        let delta = Duration::from_millis(100);
        ping_manager.update(delta);
        time_manager.update(delta, Duration::default());
        assert!(ping_manager.should_send_ping());

        let ping_message = ping_manager.prepare_ping(&time_manager);
        assert!(!ping_manager.should_send_ping());
        assert_eq!(ping_message.id, PingId(0));

        let delta = Duration::from_millis(20);
        ping_manager.update(delta);
        time_manager.update(delta, Duration::default());
        let pong_message = PongMessage {
            ping_id: PingId(0),
            tick: Default::default(),
            offset_sec: 0.0,
        };
        ping_manager.process_pong(pong_message, &time_manager);

        assert_eq!(ping_manager.rtt_ms_average, 0.9 * 10.0 + 0.1 * 20.0);
        assert_eq!(ping_manager.jitter_ms_average, 0.9 * 0.0 + 0.1 * 5.0);
    }
}
