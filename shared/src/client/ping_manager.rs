use super::sync::SyncStats;
use crate::connection::ProtocolMessage;
use crate::tick::Tick;
use crate::{
    ChannelKind, Connection, DefaultSequencedUnreliableChannel, MessageManager, PingId,
    PingMessage, PingStore, PongMessage, Protocol, SyncMessage, TimeManager, TimeSyncPingMessage,
    TimeSyncPongMessage, WrappedTime,
};
use bevy::prelude::{Timer, TimerMode};
use chrono::Duration as ChronoDuration;
use std::time::Duration;

// TODO: this could be used by client or by server?
/// Contains Config properties which will be used by a Server or Client
#[derive(Clone, Debug)]
pub struct PingConfig {
    pub sync_num_pings: u8,
    pub sync_ping_interval_ms: Duration,
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
            sync_num_pings: 10,
            sync_ping_interval_ms: Duration::from_millis(100),
            ping_interval_ms: Duration::from_millis(100),
            rtt_ms_initial_estimate: Duration::from_millis(10),
            jitter_ms_initial_estimate: Default::default(),
            rtt_smoothing_factor: 0.0,
        }
    }
}

//
// pub struct Stats {
//     pruned_offset_avg: f32,
//     raw_offset_avg: f32,
//     offset_stdv: f32,
//     pruned_rtt_avg: f32,
//     raw_rtt_avg: f32,
//     rtt_stdv: f32,
// }
//
// impl Stats {
//     /// Receive new offset/rtt from a pong message, update stats
//     fn process_stats(&mut self, offset_millis: i32, rtt_millis: u32) {
//         let offset_sample = offset_millis as f32;
//         let rtt_sample = rtt_millis as f32;
//
//         self.raw_offset_avg = (0.9 * self.raw_offset_avg) + (0.1 * offset_sample);
//         self.raw_rtt_avg = (0.9 * self.raw_rtt_avg) + (0.1 * rtt_sample);
//
//         let offset_diff = offset_sample - self.raw_offset_avg;
//         let rtt_diff = rtt_sample - self.raw_rtt_avg;
//
//         self.offset_stdv = ((0.9 * self.offset_stdv.powi(2)) + (0.1 * offset_diff.powi(2))).sqrt();
//         self.rtt_stdv = ((0.9 * self.rtt_stdv.powi(2)) + (0.1 * rtt_diff.powi(2))).sqrt();
//
//         if offset_diff.abs() < self.offset_stdv && rtt_diff.abs() < self.rtt_stdv {
//             self.pruned_offset_avg = (0.9 * self.pruned_offset_avg) + (0.1 * offset_sample);
//             self.pruned_rtt_avg = (0.9 * self.pruned_rtt_avg) + (0.1 * rtt_sample);
//         } else {
//             // Pruned out sample
//         }
//     }
// }
//

//
// // TODO: right now this does 2 things
// // - PingManager: send ping/pongs and compute offset/rtt/jitter (and possibly ask TimeManager to speed/slow time)
// // - TickManager: compute ticks based on data
//
// // TODO: this should be associated with a connection? maybe we need Server/Client connection and BaseConnection
// // 2 objectives:
// // - handle sending regular pings to clients to estimate rtt/jitter/loss
// // - handle receiving pings from clients and sending back pongs
// pub struct PingManager {
//     /// Timer to send regular pings to server
//     ping_timer: Timer,
//     /// Store to keep track of sent pings
//     sent_pings: PingStore,
//     /// ping id corresponding to the most recent pong received
//     most_recent_received_ping: PingId,
//
//     // stats to sync time
//     pub stats: Stats,
//
//     // current client tick, so that messages send from the client arrive to the server at the correct tick
//     pub client_tick: Tick,
//     client_tick_instant: WrappedTime,
//
//     // server
//     server_tick: Tick,
//     server_tick_instant: WrappedTime,
//     server_tick_duration_avg: Duration,
//     server_speedup_potential: f32,
//     // if server misses a packet
//     // that might mean the buffer is too small!
//     // we can speed up the client so that we are sure that messages for tick T are received in server before tick T arrives
//     // (the tick_delta doesn't change based on time speed so we will reach tick T + D faster on client)
//
//     // difference between server tick and client tick
//     // if the difference should change (because latency/jitter changes, we speed-up/slow-down client?
//     // pub tick_delta: u32,
// }
//
// /// Ping/Tick/Sync manager
// /// - Is responsible for sending regular pings to client and get pongs back
// /// - Makes sure that the local time from time_manager is synced with the server_time
// /// - Computes a client time, which is how much ahead the client needs to be so that its messages arrive at the server at the correct tick
// /// - Computes local ticks by re-using the same values as server (server-tick, server-instant, server-duration)
// impl PingManager {
//     pub fn new(
//         ping_config: &PingConfig,
//         time_manager: &TimeManager,
//         round_trip_delay_ms: i32,
//         server_tick: Tick,
//         server_tick_instant: WrappedTime,
//         server_tick_duration_ms_avg: f32,
//         rtt_stdv: i32,
//     ) -> Self {
//         // because we went through syncing; now should be equivalent to server time
//         let now = time_manager.current_time();
//         let latency_ms = (round_trip_delay_ms / 2.0) as u32;
//         let major_jitter_ms = (rtt_stdv / 2.0 * 3.0) as u32;
//         // let tick_duration_ms = server_tick_duration_avg.round() as u32;
//
//         // let rtt_ms_average = ping_config.rtt_ms_initial_estimate.as_secs_f32() * 1000.0;
//         // let jitter_ms_average = ping_config.jitter_ms_initial_estimate.as_secs_f32() * 1000.0;
//
//         // NOTE: We want the client tick to be ahead of the server tick by tick_delta
//         // (so that commands sent from the client will be received right one time by the server)
//         // TODO: use tick_duration or server_tick_duration_avg?
//         // let tick_duration = ping_config.ping_interval_ms;
//
//         // how much ahead of the server should the client time be.
//         let time_delta_ms =
//             latency_ms + major_jitter_ms + server_tick_duration_ms_avg.round() as u32;
//         let client_time = now + Duration::from_millis(time_delta_ms as u64);
//         let client_tick = instant_to_tick(
//             &server_tick,
//             &server_tick_instant,
//             server_tick_duration_ms_avg,
//             &client_time,
//         );
//
//         PingManager {
//             ping_timer: Timer::new(ping_config.ping_interval_ms, TimerMode::Repeating),
//             sent_pings: PingStore::new(),
//             most_recent_received_ping: PingId(0) - 1,
//         }
//     }
//
//     /// Update our tracking of the server tick / server tick instant from the values
//     /// read from server packets
//     pub fn update_server_tick(&mut self, server_tick: Tick, server_tick_instant: WrappedTime) {
//         // only update values if the server tick got incremented
//         if server_tick <= self.server_tick {
//             return;
//         }
//
//         // TODO: understand this
//         // maybe this is to compute changes in the server's tick duration
//         // for example, because of fixed-timestamp, the next server tick might have started a bit later/earlier than expected
//         let prev_server_tick_instant = self.tick_to_instant(server_tick);
//         let offset = server_tick_instant - prev_server_tick_instant;
//
//         self.server_tick = server_tick;
//         self.server_tick_instant = server_tick_instant;
//
//         // TODO: understand this
//         self.client_tick_instant += offset;
//     }
//
//     /// Update the server tick duration
//     pub fn update_server_tick_duration_avg(
//         &mut self,
//         server_tick_duration_ms_avg: f32,
//         server_speedup_potential: f32,
//     ) {
//         // compute the interp of the client tick instant using the existing server_tick_duration
//         let client_interp = self.get_interp(self.client_tick, &self.client_tick_instant);
//
//         // update the server_tick_duration
//         self.server_tick_duration_avg =
//             Duration::from_secs_f32(server_tick_duration_ms_avg / 1000.0);
//         self.server_speedup_potential = server_speedup_potential;
//
//         // recomputes the client_tick_instant using the new server_tick_duration
//         self.client_tick_instant = self.instant_from_interp(self.client_tick, client_interp);
//     }
//
//     // TODO: is this correct? maybe we always want client_ticks to progress monotonically?
//
//     // When we receive a pong, time at server is last_tick + last_tick_instant
//     // set client_tick to last_tick + last_tick_instant - tick_delta
//     //
//     // when we don't receive pongs, just increment client_ticks by 1 once we cross tick duration
//     pub fn update(&mut self, time_manager: &TimeManager) -> bool {
//         let time_offset = time_manager.current_time() - self.last_tick_wrapped_time;
//         if time_offset > self.config.tick_duration {
//             // TODO: compute the actual tick duration
//             self.client_tick += 1;
//             self.last_tick_wrapped_time = time_manager.current_time();
//             return true;
//         }
//         return false;
//     }
//
//     /// Returns whether a ping message should be sent
//     pub fn should_send_ping(&self) -> bool {
//         self.ping_timer.finished()
//     }
//
//     /// Buffer a ping message (reset the timer)
//     // TimeSyncPingMessage = Ping from client to server.
//     // TODO: issues here: we would like to send the ping message immediately, otherwise the recorded current time is incorrect
//     //   - can give infinity priority to this channel?
//     //   - can write directly to io otherwise?
//     pub fn prepare_ping(&mut self, time_manager: &TimeManager) -> TimeSyncPingMessage {
//         self.ping_timer.reset();
//
//         let ping_id = self
//             .sent_pings
//             .push_new(time_manager.current_time().clone());
//
//         // TODO: for rtt purposes, we could just send a ping that has no tick info
//         // PingMessage::new(ping_id, time_manager.current_tick())
//         TimeSyncPingMessage::new(ping_id, Tick(0))
//
//         // let message = ProtocolMessage::Sync(SyncMessage::Ping(ping));
//         // let channel = ChannelKind::of::<DefaultUnreliableChannel>();
//         // connection.message_manager.buffer_send(message, channel)
//     }
//
//     /// Buffer a pong message (in response to a ping)
//     // TODO: issues here: we would like to send the ping message immediately, otherwise the recorded current time is incorrect
//     //   - can give infinity priority to this channel?
//     //   - can write directly to io otherwise?
//     pub fn prepare_pong(&mut self, time_manager: &TimeManager, ping: PingMessage) -> PongMessage {
//         // TODO: for rtt purposes, we could just send a ping that has no tick info
//         PongMessage {
//             ping_id: ping.id,
//             tick: Default::default(),
//             offset_sec: 0.0,
//         }
//         // let message = ProtocolMessage::Sync(SyncMessage::Ping(ping));
//         // let channel = ChannelKind::of::<DefaultUnreliableChannel>();
//         // connection.message_manager.buffer_send(message, channel)
//     }
//
//     /// Process an incoming pong payload
//     pub(crate) fn process_pong(
//         &mut self,
//         pong: &TimeSyncPongMessage,
//         time_manager: &TimeManager,
//     ) -> Option<ServerPongData> {
//         let client_received_time = time_manager.current_time();
//
//         let Some(ping_sent_time) = self.sent_pings.remove(pong.ping_id) else {
//             panic!("unknown ping id");
//         };
//
//         // only update values for the most recent pongs received
//         if pong.ping_id > self.most_recent_received_ping {
//             // compute offset and round-trip delay via NTP algorithm: https://en.wikipedia.org/wiki/Network_Time_Protocol
//             self.most_recent_received_ping = pong.ping_id;
//
//             // offset
//             // t1 - t0 (ping recv - ping sent)
//             let ping_offset_ms = (pong.ping_received_time - ping_sent_time).as_millis() as i32;
//             // t2 - t3 (pong sent - pong receive)
//             let pong_offset_ms = -((client_received_time - pong.pong_sent_time).as_millis() as i32);
//             let offset_ms = (ping_offset_ms + pong_offset_ms) / 2;
//
//             // round-trip-delay
//             let rtt_ms = (client_received_time - ping_sent_time).as_millis() as u32;
//             let server_process_time_ms =
//                 (pong.pong_sent_time - pong.ping_received_time).as_millis() as u32;
//             let round_trip_delay_ms = rtt_ms - server_process_time_ms;
//
//             // update stats
//             self.stats.process_stats(offset_ms, round_trip_delay_ms);
//             // TODO: no need to do it here, because we send server-tick, server-time in every package!
//             //  so we will call update_server_tick every time
//             // update server tick
//             self.update_server_tick(pong.server_tick, pong.server_tick_instant);
//             // update server_duration_avg
//             self.update_server_tick_duration_avg(
//                 pong.tick_duration_ms_avg,
//                 pong.tick_speedup_potential,
//             );
//
//             return Some(ServerPongData {
//                 tick_duration_ms_avg: pong.tick_duration_ms_avg,
//                 tick_speedup_potential: pong.tick_speedup_potential,
//                 offset_ms,
//                 round_trip_delay_ms,
//             });
//         }
//         None
//     }
//
//     /// Convert a tick to instant (using server_tick/tick_instant/tick_duration as reference)
//     pub(crate) fn tick_to_instant(&self, tick: Tick) -> WrappedTime {
//         let tick_diff = tick - self.server_tick;
//         let tick_diff_duration =
//             ChronoDuration::from_std(self.server_tick_duration_avg).unwrap() * (tick_diff as i32);
//         self.server_tick_instant + tick_diff_duration
//     }
//
//     /// Float between 0 and 1 representing the interpolation of `instant` between `tick` and `tick`+1
//     /// (uses server_tick_duration_avg as tick time)
//     pub(crate) fn get_interp(&self, tick: Tick, instant: &WrappedTime) -> f32 {
//         let delta_ms: Duration = (instant - self.tick_to_instant(tick)).to_std().unwrap();
//         // TODO: requires "div_duration" unstable feature
//         // delta_ms.div_duration_f32(self.server_tick_duration_avg)
//         delta_ms.as_secs_f32() / self.server_tick_duration_avg.as_secs_f32()
//     }
//
//     /// Find the instant corresponding to interpolating between `tick` and `tick`+1
//     /// (uses server_tick_duration_avg as tick time)
//     pub(crate) fn instant_from_interp(&self, tick: Tick, interp: f32) -> WrappedTime {
//         let offset_ms = self.server_tick_duration_avg.mul_f32(interp);
//         self.tick_to_instant(tick) + ChronoDuration::from_std(offset_ms).unwrap()
//     }
// }
//
// /// Earliest instant at which the server should be able to receive a message sent now
// /// - add latency
// /// - remove jitter
// /// TODO: understand adding tick duration?
// /// - add 1 tick (account for client receive -> client send delay)
// /// - add 1 tick (account for server receiving the message but having to wait until next frame's receive)
// /// - add 1 tick as a buffer?
// fn get_server_receivable_target(
//     now: &WrappedTime,
//     latency: u32,
//     jitter: u32,
//     tick_duration: u32,
// ) -> WrappedTime {
//     ///
//     let millis = (((latency + (tick_duration * 2)) as i32) - (jitter as i32)).max(0) as u32;
//     now + ChronoDuration::milliseconds(millis as i64)
// }
//
// /// Client tick should be in the 'future' compared to server
// /// This is the time at which the server should send a message so that the client receives it now
// /// - remove latency
// /// - remove jitter
// /// - remove one tick as a buffer?
// fn get_client_receiving_target(
//     now: &WrappedTime,
//     latency: u32,
//     jitter: u32,
//     tick_duration: u32,
// ) -> WrappedTime {
//     let millis = latency + jitter + tick_duration;
//     now - ChronoDuration::milliseconds(millis as i64)
// }
//
// /// Convert an instant to what the equivalent tick would be (using server_tick and server_tick_duration)
// fn instant_to_tick(
//     server_tick: &Tick,
//     // time when the server switch to tick `server_tick`
//     server_tick_instant: &WrappedTime,
//     // use server tick duration avg so that the client tick rate does not matter?
//     server_tick_duration_avg: f32,
//     instant: &WrappedTime,
// ) -> Tick {
//     let offset_ms = (*instant - *server_tick_instant).num_milliseconds();
//     let offset_ticks_f32 = (offset_ms as f32) / server_tick_duration_avg;
//     return server_tick + (offset_ticks_f32 as i16);
// }
//
// pub struct ServerPongData {
//     pub tick_duration_ms_avg: f32,
//     pub tick_speedup_potential: f32,
//     pub offset_ms: i32,
//     pub round_trip_delay_ms: u32,
// }
//
// #[cfg(test)]
// mod tests {
//     use super::*;
//     use bevy::prelude::Time;
//     use crate::{PingId, TickConfig};
//     #[test]
//     fn test_ping_manager() {
//         let ping_config = PingConfig {
//             ping_interval_ms: Duration::from_millis(100),
//             rtt_ms_initial_estimate: Duration::from_millis(10),
//             jitter_ms_initial_estimate: Default::default(),
//             rtt_smoothing_factor: 0.0,
//         };
//         let mut ping_manager = PingManager::new(&ping_config);
//         let tick_config = TickConfig::new(Duration::from_millis(16));
//         let mut time_manager = TimeManager::new();
//
//         assert!(!ping_manager.should_send_ping());
//         let delta = Duration::from_millis(100);
//         ping_manager.update(delta);
//         time_manager.update(delta);
//         assert!(ping_manager.should_send_ping());
//
//         let ping_message = ping_manager.prepare_ping(&time_manager);
//         assert!(!ping_manager.should_send_ping());
//         assert_eq!(ping_message.id, PingId(0));
//
//         let delta = Duration::from_millis(20);
//         ping_manager.update(delta);
//         time_manager.update(delta);
//         let pong_message = PongMessage {
//             ping_id: PingId(0),
//             tick: Default::default(),
//             offset_sec: 0.0,
//         };
//         ping_manager.process_pong(&pong_message, &time_manager);
//
//         assert_eq!(ping_manager.rtt_ms_average, 0.9 * 10.0 + 0.1 * 20.0);
//         assert_eq!(ping_manager.jitter_ms_average, 0.9 * 0.0 + 0.1 * 5.0);
//     }
// }
