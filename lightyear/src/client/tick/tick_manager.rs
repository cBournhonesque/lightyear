use std::time::Duration;

use crate::shared::{serde::Serde, wrapping_diff, Instant, Tick};

use lightyear_serde::{BitReader, BitWriter};

/// Manages the current tick for the host
pub struct TickManager {
    /// How much time in milliseconds does a tick last
    tick_interval_millis: f32,
    /// How much time in seconds does a tick last
    tick_interval_seconds: f32,
    /// Used to modify the tick interval. A value >1.0 means that the tick interval will be bigger
    tick_speed_factor: f32,
    /// Smoothed measure how fast the tick offset is varying
    tick_offset_speed_avg: f32,
    /// Smoothed measure of how much ahead the client tick is compared to the server tick
    tick_offset_avg: f32,
    /// current client tick
    internal_tick: Tick,
    client_sending_tick_adjust: f32,
    server_receivable_tick_adjust: f32,
    client_receiving_tick_adjust: f32,
    last_tick_instant: Instant,
    interpolation: f32,
    accumulator: f32,
    minimum_latency: f32,
    /// Last tick offset recorded
    last_tick_offset: i16,
    ticks_recorded: u8,
}

impl TickManager {
    /// Create a new TickManager with a given tick interval duration
    pub fn new(tick_interval: Duration, minimum_latency_duration: Option<Duration>) -> Self {
        let minimum_latency = {
            if let Some(min_latency) = minimum_latency_duration {
                min_latency.as_millis() as f32
            } else {
                0.0
            }
        };

        let tick_interval_millis = tick_interval.as_millis() as f32;

        TickManager {
            tick_interval_millis,
            tick_interval_seconds: tick_interval.as_nanos() as f32 / 1000000000.0,
            tick_speed_factor: 1.0,
            tick_offset_avg: 0.0,
            tick_offset_speed_avg: 0.0,
            internal_tick: 0,
            client_sending_tick_adjust: 0.0,
            client_receiving_tick_adjust: 0.0,
            server_receivable_tick_adjust: 0.0,
            last_tick_instant: Instant::now(),
            accumulator: 0.0,
            interpolation: 0.0,
            minimum_latency,
            last_tick_offset: 0,
            ticks_recorded: 0,
        }
    }

    pub fn write_client_tick(&self, writer: &mut BitWriter) -> Tick {
        let client_tick = self.client_sending_tick();
        client_tick.ser(writer);
        client_tick
    }

    /// Read server tick from any packet that includes it and updates
    ///
    /// # Panics
    ///
    /// If the incoming packet from the server doesn't contain the server tick
    pub fn read_server_tick(&mut self, reader: &mut BitReader, rtt: f32, jitter: f32) -> Tick {
        let server_tick = Tick::de(reader).expect("unable to read server tick from packet");

        self.record_server_tick(server_tick, rtt, jitter);

        server_tick
    }

    /// This should run every frame. Check if enough time has passed so that we move to the next tick
    /// Also keeps track of internal state such as the interpolation percentage
    pub fn recv_client_tick(&mut self) -> bool {
        let mut ticked = false;
        let tick_interval_seconds = self.tick_interval_seconds * self.tick_speed_factor;

        let frame_seconds = self.last_tick_instant.elapsed().as_nanos() as f32 / 1000000000.0;
        // if frame_seconds > tick_interval_seconds {
        //     info!("big frame delta: {} ms", frame_seconds*1000.0);
        // }
        self.accumulator += frame_seconds.min(0.25);
        self.last_tick_instant = Instant::now();

        if self.accumulator >= tick_interval_seconds {
            while self.accumulator >= tick_interval_seconds {
                self.accumulator -= tick_interval_seconds;
                self.internal_tick = self.internal_tick.wrapping_add(1);
            }
            // tick has occurred
            ticked = true;
        }
        self.interpolation = self.accumulator / tick_interval_seconds;
        ticked
    }

    /// Return the current interpolation of the frame between the two surrounding ticks
    /// 0.20 means that we are 20% of the way to the next tick
    pub fn interpolation(&self) -> f32 {
        self.interpolation
    }

    /// Using information from the Server and RTT/Jitter measurements, determine
    /// the appropriate future intended tick
    fn record_server_tick(&mut self, server_tick: Tick, rtt_average: f32, jitter_average: f32) {
        // make sure we only record server_ticks going FORWARD

        // tick diff
        let tick_offset = wrapping_diff(self.internal_tick, server_tick);

        if self.ticks_recorded <= 1 {
            if self.ticks_recorded == 1 {
                self.tick_offset_avg = tick_offset as f32;
            }

            self.ticks_recorded += 1;
        } else {
            self.tick_offset_avg = (0.9 * self.tick_offset_avg) + (0.1 * (tick_offset as f32));
            let tick_offset_speed = (tick_offset - self.last_tick_offset) as f32;
            self.tick_offset_speed_avg =
                (0.9 * self.tick_offset_speed_avg) + (0.1 * tick_offset_speed);
        }

        self.last_tick_offset = tick_offset;

        if self.tick_offset_speed_avg > 1.0 {
            self.tick_speed_factor -= 0.1;
            self.tick_offset_speed_avg = 0.0;
        }
        if self.tick_offset_speed_avg < -1.0 {
            self.tick_speed_factor += 0.1;
            self.tick_offset_speed_avg = 0.0;
        }

        // Calculate incoming & outgoing jitter buffer tick offsets

        let jitter_limit = jitter_average * 4.0;
        self.client_receiving_tick_adjust = jitter_limit / self.tick_interval_millis;

        // NOTE: I've struggled multiple times with why rtt_average instead of
        // ping_average exists in this calculation, figured it out, then
        // returned to struggle later. This is not a bug!
        // Keep in mind that self.server_tick here is the tick we have RECEIVED from the
        // Server which means that the real current server_tick is likely
        // self.server_tick + ping_average / tick_interval.
        // By using rtt_average here, we are correcting for our late (and
        // lesser) self.server_tick value
        let client_sending_adjust_millis = self.minimum_latency.max(rtt_average + jitter_limit);
        self.client_sending_tick_adjust = client_sending_adjust_millis / self.tick_interval_millis;

        // Calculate estimate of earliest tick Server could receive now
        let server_receivable_adjust_millis = rtt_average - jitter_limit;
        self.server_receivable_tick_adjust =
            server_receivable_adjust_millis / self.tick_interval_millis;
    }

    /// Gets the tick at which the Client is sending updates
    pub fn client_sending_tick(&self) -> Tick {
        let mut output = self.server_tick_estimate() + self.client_sending_tick_adjust;
        wrap_f32(&mut output);
        output.round() as Tick
    }

    /// Gets the tick at which to receive messages from the Server (after jitter
    /// buffer offset is applied)
    pub fn client_receiving_tick(&self) -> Tick {
        let mut output = self.server_tick_estimate() - self.client_receiving_tick_adjust;
        wrap_f32(&mut output);
        output.round() as Tick
    }

    /// Gets the earliest tick the Server may be able to receive Client messages
    pub fn server_receivable_tick(&self) -> Tick {
        let mut output = self.server_tick_estimate() + self.server_receivable_tick_adjust;
        wrap_f32(&mut output);
        output.round() as Tick
    }

    fn server_tick_estimate(&self) -> f32 {
        (self.internal_tick as f32) + self.tick_offset_avg
    }
}

fn wrap_f32(val: &mut f32) {
    const U16_MAX_AS_F32: f32 = u16::MAX as f32;
    while *val < 0.0 {
        *val += U16_MAX_AS_F32;
    }
    while *val > U16_MAX_AS_F32 {
        *val -= U16_MAX_AS_F32;
    }
}
