use std::time::Duration;

use bevy::prelude::Resource;
use tracing::trace;

use crate::tick::Tick;

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

// TODO: maybe put this outside of server/client? as a separate resource in SharedPlugin instead?
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
    pub fn set_tick_to(&mut self, tick: Tick) {
        self.tick = tick;
    }

    pub fn current_tick(&self) -> Tick {
        self.tick
    }
}
