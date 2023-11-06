use lightyear_shared::tick::Tick;
use lightyear_shared::{TickConfig, TickManager, WrappedTime};
use std::time::Duration;

pub struct TimeManager {
    tick_manager: TickManager,
    wrapped_time: WrappedTime,
}

impl TimeManager {
    pub fn new(config: TickConfig) -> Self {
        Self {
            tick_manager: TickManager::from_config(config),
            wrapped_time: WrappedTime::new(0),
        }
    }

    /// Update the time by matching the virtual time from bevy
    /// (time from server start, wrapped around the hour)
    pub fn update(&mut self, delta: Duration) {
        self.wrapped_time += delta;
        self.tick_manager.update(delta);
    }

    /// Current time since server start, wrapped around 1 hour
    pub fn current_time(&self) -> WrappedTime {
        self.wrapped_time
    }

    pub fn current_tick(&self) -> Tick {
        self.tick_manager.current_tick()
    }
}
