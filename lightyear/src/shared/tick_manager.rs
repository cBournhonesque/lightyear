//! Module to handle the [`Tick`], a sequence number incremented at each [`bevy::prelude::FixedUpdate`] schedule run
use bevy::prelude::*;
use bevy::utils::Duration;
use tracing::trace;

use crate::client::prediction::plugin::is_in_rollback;
use crate::client::prediction::rollback::Rollback;
use crate::prelude::FixedUpdateSet;
use crate::utils::wrapping_id::wrapping_id;

// Internal id that tracks the Tick value for the server and the client
wrapping_id!(Tick);

pub struct TickManagerPlugin {
    pub(crate) config: TickConfig,
}

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

impl Plugin for TickManagerPlugin {
    fn build(&self, app: &mut App) {
        app
            // RESOURCES
            .insert_resource(TickManager::from_config(self.config))
            // SYSTEM SETS
            .configure_sets(FixedFirst, FixedUpdateSet::TickUpdate)
            // SYSTEMS
            .add_systems(
                FixedFirst,
                (increment_tick
                    .in_set(FixedUpdateSet::TickUpdate)
                    // run if there is no rollback resource, or if we are not in rollback
                    .run_if(not(resource_exists::<Rollback>).or(not(is_in_rollback))),),
            );
    }
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
    pub(crate) fn from_config(config: TickConfig) -> Self {
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
