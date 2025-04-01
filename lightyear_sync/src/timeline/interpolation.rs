use crate::ping::manager::PingManager;
use crate::timeline::sync::{SyncConfig, SyncEvent, SyncedTimeline};
use crate::timeline::{NetworkTimeline, Timeline};
use bevy::prelude::{Component, Reflect};
use core::time::Duration;
use lightyear_core::tick::Tick;
use lightyear_core::time::{Overstep, TickDelta, TickInstant, TimeDelta};

/// Config to specify how the snapshot interpolation should behave
#[derive(Clone, Copy, Reflect)]
pub struct InterpolationConfig {
    /// The minimum delay that we will apply for interpolation
    /// This should be big enough so that the interpolated entity always has a server snapshot
    /// to interpolate towards.
    /// Set to 0.0 if you want to only use the Ratio
    pub min_delay: Duration,
    /// The interpolation delay is a ratio of the update-rate from the server
    /// The higher the server update_rate (i.e. smaller send_interval), the smaller the interpolation delay
    /// Set to 0.0 if you want to only use the Delay
    pub send_interval_ratio: f32,
    pub relative_speed: f32,
}

impl Default for InterpolationConfig {
    fn default() -> Self {
        Self {
            min_delay: Duration::from_millis(5),
            send_interval_ratio: 1.3,
            relative_speed: 1.0,
        }
    }
}

impl InterpolationConfig {
    pub fn with_min_delay(mut self, min_delay: Duration) -> Self {
        self.min_delay = min_delay;
        self
    }

    pub fn with_send_interval_ratio(mut self, send_interval_ratio: f32) -> Self {
        self.send_interval_ratio = send_interval_ratio;
        self
    }

    /// How much behind the latest server update we want the interpolation time to be
    pub(crate) fn to_duration(self, server_send_interval: Duration) -> Duration {
        // TODO: deal with server_send_interval = 0 (set to frame rate)
        let ratio_value = server_send_interval.mul_f32(self.send_interval_ratio);
        core::cmp::max(ratio_value, self.min_delay)
    }
}

#[derive(Component, Default)]
pub struct Interpolation {
    tick_duration: Duration,

    pub(crate) remote_send_interval: Duration,
    pub(crate) interpolation_config: InterpolationConfig,
    pub(crate) sync_config: SyncConfig,
    pub(crate) relative_speed: f32,
    pub(crate) now: TickInstant,
}

// TODO: should this be contained in a 'BaseTimeline'?

pub type InterpolationTimeline = Timeline<Interpolation>;

impl SyncedTimeline for Timeline<Interpolation> {
    // TODO: how can we make this configurable? or maybe just store the TICK_DURATION in the timeline itself?

    fn sync_objective<T: NetworkTimeline>(&self, main: &T, ping_manager: &PingManager) -> TickInstant {
        let delay = TickDelta::from_duration(self.context.interpolation_config.to_duration(self.context.remote_send_interval), self.tick_duration());
        let target = main.now();
        target - delay
    }

    fn resync(&mut self, sync_objective: TickInstant) -> SyncEvent<Self> {
        let now = self.now();
        let target = sync_objective;
        self.now = target;
        SyncEvent::<Self> {
            old: now,
            new: target,
            marker: core::marker::PhantomData,
        }
    }

    // TODO: this code is duplicated in the Predicted timeline
    /// Adjust the current timeline to stay in sync with the [`MainTimeline`].
    ///
    /// Most of the times this will just be slight nudges to modify the speed of the [`SyncedTimeline`].
    /// If there's a big discrepancy, we will snap the [`SyncedTimeline`] to the [`MainTimeline`] by sending a SyncEvent
    fn sync<T: NetworkTimeline>(&mut self, main: &T, ping_manager: &PingManager) -> Option<SyncEvent<Self>> {
        // skip syncing if we haven't received enough information
        if ping_manager.pongs_recv < self.context.sync_config.handshake_pings as u32 {
            return None
        }
        // TODO: should we call current_estimate()? now() should basically return the same thing
        let target = main.now();
        let objective = self.sync_objective(main, ping_manager);
        let error = objective - target;
        let is_ahead = error.is_positive();
        let error_duration = error.to_duration(self.tick_duration());
        let error_margin = self.tick_duration().mul_f32(self.context.sync_config.error_margin);
        let max_error_margin = self.tick_duration().mul_f32(self.context.sync_config.max_error_margin);
        if error_duration > max_error_margin {
            return Some(self.resync(objective));
        } else if error_duration > error_margin {
            let ratio = if is_ahead {
                1.0 / self.context.sync_config.speedup_factor
            } else {
                1.0 * self.context.sync_config.speedup_factor
            };
            self.set_relative_speed(ratio);
        }
        None
    }

    // TODO: do we want this or do we want a marker component to check if the timline is synced?
    fn is_synced(&self) -> bool {
        todo!()
    }

    fn relative_speed(&self) -> f32 {
        self.context.relative_speed
    }

    fn set_relative_speed(&mut self, ratio: f32) {
        self.context.relative_speed = ratio;
    }
}