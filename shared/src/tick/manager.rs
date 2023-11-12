use crate::tick::Tick;
use bevy::prelude::Resource;
use bevy::time::Time;
use std::time::Duration;
use tracing::trace;

// When a server sends a message with its tick Ts, t
// the client knows its current tick Tc and can compare Ts-Tc and know if it needs to slow down/up etc.
// and it knows the tick difference between them as well as the rtt?

#[derive(Clone)]
pub struct TickConfig {
    pub tick_duration: Duration,
}

impl TickConfig {
    pub fn new(tick_duration: Duration) -> Self {
        Self { tick_duration }
    }
}

// Manages the tick for the host system
#[derive(Resource)]
pub struct TickManager {
    pub config: TickConfig,
    /// Current tick (sequence number of the FixedUpdate schedule)
    /// Gets updated by the FixedUpdate schedule
    tick: Tick,
}

// TODO: THIS IS SERVER TIME MANAGER.
impl TickManager {
    pub fn from_config(config: TickConfig) -> Self {
        Self {
            config,
            tick: Tick(0),
        }
    }

    pub fn increment_tick(&mut self) {
        self.tick += 1;
        trace!(new_tick = ?self.tick, "incremented client tick")
    }

    pub fn increment_tick_by(&mut self, increment: u16) {
        self.tick += increment;
    }

    //     /// Update the status of the TickManager after time advances by `elapsed`
    // pub fn update(&mut self, delta: Duration) {
    //     // self.current_time += time;
    //
    //     // TODO: only servers fixes the tick based on time
    //     //  client set their ticks with a RTT offset based on server tick
    //     // Possibly update tick
    //     let time_offset_sec = delta.as_secs_f64();
    //     let tick_offset = time_offset_sec / self.config.tick_duration.as_secs_f64();
    //     self.tick = Tick(self.tick.wrapping_add(tick_offset as u16))
    // }

    pub fn current_tick(&self) -> Tick {
        self.tick
    }

    // Receive a client ping containing: client tick, client timestamp
    // Send back:
    // - server timestamp when receiving client message, server tick,
    // pub fn process_client_ping(&mut self) {}
}

// set tick rate
// update tick whenever time crosses threshold
