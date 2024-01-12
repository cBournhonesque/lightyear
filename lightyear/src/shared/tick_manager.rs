//! Module to handle the [`Tick`], a sequence number incremented at each [`bevy::prelude::FixedUpdate`] schedule run
use bevy::utils::Duration;

use bevy::prelude::Resource;
use tracing::trace;

use crate::utils::wrapping_id::wrapping_id;

// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);

pub trait TickManaged: Resource {
    fn tick(&self) -> Tick;
    fn increment_tick(&mut self);
}

#[derive(Clone)]
pub struct TickConfig {
    pub tick_duration: Duration,
}

impl TickConfig {
    pub fn new(tick_duration: Duration) -> Self {
        Self { tick_duration }
    }
}

/// Manages the tick for the host system. Ticks are incremented by one every time
/// the [`bevy::prelude::FixedUpdate`] schedule runs
#[derive(Resource)]
pub struct TickManager {
    /// Tick configuration
    pub config: TickConfig,
    /// Current tick (sequence number of the FixedUpdate schedule)
    tick: Tick,
}

impl TickManager {
    pub fn from_config(config: TickConfig) -> Self {
        Self {
            config,
            tick: Tick(0),
        }
    }

    // NOTE: this is public just for integration testing purposes
    #[doc(hidden)]
    pub fn increment_tick(&mut self) {
        self.tick += 1;
        trace!(new_tick = ?self.tick, "incremented client tick")
    }
    pub(crate) fn set_tick_to(&mut self, tick: Tick) {
        self.tick = tick;
    }

    pub fn current_tick(&self) -> Tick {
        self.tick
    }
}
