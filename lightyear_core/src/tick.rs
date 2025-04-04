use crate::prediction::Rollback;
use bevy::app::{App, FixedFirst, Plugin};
use bevy::prelude::{not, resource_exists, Event, IntoScheduleConfigs, Reflect, ResMut, Resource};
use core::time::Duration;
use lightyear_utils::wrapping_id;
use tracing::trace;

// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);

// TODO: we actually don't need this on server-side..
#[derive(Event, Debug, Clone, Copy)]
pub enum TickEvent {
    TickSnap { old_tick: Tick, new_tick: Tick },
}

/// System that increments the tick at the start of FixedUpdate
pub(crate) fn increment_tick(mut tick_manager: ResMut<TickManager>) {
    tick_manager.increment_tick();
    trace!("increment_tick! new tick: {:?}", tick_manager.tick());
}

#[derive(Clone, Copy, Debug, Reflect)]
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
        trace!(new_tick = ?self.tick, "incremented tick")
    }
    pub(crate) fn set_tick_to(&mut self, tick: Tick) -> TickEvent {
        let old_tick = self.tick;
        self.tick = tick;
        // info!(?old_tick, new_tick =?tick, "tick snap event");
        TickEvent::TickSnap {
            old_tick,
            new_tick: tick,
        }
    }

    /// Get the current tick of the local app
    pub fn tick(&self) -> Tick {
        self.tick
    }

    /// Get the current tick of the app; works even if we are in rollback
    pub fn tick_or_rollback_tick(&self, rollback_state: &Rollback) -> Tick {
        rollback_state.get_rollback_tick().unwrap_or(self.tick)
    }
}


pub struct TickPlugin {
    pub tick_duration: Duration,
}

impl Plugin for TickPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(TickManager::from_config(TickConfig::new(self.tick_duration)));
    }
}

