use std::collections::VecDeque;

use lightyear_serde::{BitReader, BitWriter, Serde};

use naia_socket_shared::Instant;

use crate::shared::{sequence_greater_than, Timer};

use super::ping_config::PingConfig;

/// Is responsible for sending regular ping messages between client/servers
/// and to estimate rtt/jitter
pub struct PingManager {
    ping_timer: Timer,
    sent_pings: SentPings,
    pub rtt: f32,
    pub jitter: f32,
    rtt_smoothing_factor: f32,
    rtt_smoothing_factor_inv: f32,
}

impl PingManager {
    pub fn new(ping_config: &PingConfig) -> Self {
        let rtt_average = ping_config.rtt_initial_estimate.as_secs_f32() * 1000.0;
        let jitter_average = ping_config.jitter_initial_estimate.as_secs_f32() * 1000.0;

        PingManager {
            ping_timer: Timer::new(ping_config.ping_interval),
            sent_pings: SentPings::new(),
            rtt: rtt_average,
            jitter: jitter_average,
            rtt_smoothing_factor: ping_config.rtt_smoothing_factor,
            rtt_smoothing_factor_inv: 1.0 - ping_config.rtt_smoothing_factor,
        }
    }

    /// Returns whether a ping message should be sent
    pub fn should_send_ping(&self) -> bool {
        self.ping_timer.ringing()
    }

    /// Get an outgoing ping payload
    pub fn write_ping(&mut self, writer: &mut BitWriter) {
        self.ping_timer.reset();

        let ping_index = self.sent_pings.push_new();

        // write index
        ping_index.ser(writer);
    }

    /// Process an incoming pong payload
    pub fn process_pong(&mut self, reader: &mut BitReader) {
        if let Ok(ping_index) = PingIndex::de(reader) {
            match self.sent_pings.remove(ping_index) {
                None => {}
                Some(ping_instant) => {
                    let rtt_millis = &ping_instant.elapsed().as_secs_f32() * 1000.0;
                    self.process_new_rtt(rtt_millis);
                }
            }
        }
    }

    /// Recompute rtt/jitter estimations
    fn process_new_rtt(&mut self, rtt_millis: f32) {
        let new_jitter = ((rtt_millis - self.rtt) / 2.0).abs();
        self.jitter = (self.rtt_smoothing_factor_inv * self.jitter)
            + (self.rtt_smoothing_factor * new_jitter);

        self.rtt =
            (self.rtt_smoothing_factor_inv * self.rtt) + (self.rtt_smoothing_factor * rtt_millis);
    }
}

pub type PingIndex = u16;
const SENT_PINGS_HISTORY_SIZE: u16 = 32;

struct SentPings {
    ping_index: PingIndex,
    // front big, back small
    // front recent, back past
    buffer: VecDeque<(PingIndex, Instant)>,
}

impl SentPings {
    pub fn new() -> Self {
        SentPings {
            ping_index: 0,
            buffer: VecDeque::new(),
        }
    }

    pub fn push_new(&mut self) -> PingIndex {
        // save current ping index and add a new ping instant associated with it
        let ping_index = self.ping_index;
        self.ping_index = self.ping_index.wrapping_add(1);
        self.buffer.push_front((ping_index, Instant::now()));

        // a good time to prune down the size of this buffer
        while self.buffer.len() > SENT_PINGS_HISTORY_SIZE.into() {
            self.buffer.pop_back();
            //info!("pruning sent_pings buffer cause it got too big");
        }

        ping_index
    }

    pub fn remove(&mut self, ping_index: PingIndex) -> Option<Instant> {
        let mut vec_index = self.buffer.len();
        let mut found = false;

        loop {
            vec_index -= 1;

            if let Some((old_index, _)) = self.buffer.get(vec_index) {
                if *old_index == ping_index {
                    //found it!
                    found = true;
                } else {
                    // if old_index is bigger than ping_index, give up, it's only getting
                    // bigger
                    if sequence_greater_than(*old_index, ping_index) {
                        return None;
                    }
                }
            }

            if found {
                let (_, ping_instant) = self.buffer.remove(vec_index).unwrap();
                //info!("found and removed ping: {}", index);
                return Some(ping_instant);
            }

            // made it to the front
            if vec_index == 0 {
                return None;
            }
        }
    }
}
