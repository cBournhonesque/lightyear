use crate::ping::manager::PingManager;
use crate::timeline::sync::{
    SyncAdjustment, SyncConfig, SyncContext, SyncTargetTimeline, SyncedTimeline,
};

use bevy_derive::{Deref, DerefMut};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_utils::default;
use core::time::Duration;
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_core::time::{TickDelta, TickInstant};
use lightyear_core::timeline::{NetworkTimeline, SyncEvent, Timeline, TimelineConfig};
use lightyear_link::{Link, LinkStats};
use tracing::trace;

/// Timeline that is used to make sure that Inputs from this peer will arrive on time
/// on the remote peer
#[derive(Debug, Component, Reflect)]
#[require(InputTimeline)]
pub struct InputTimelineConfig {
    pub(crate) sync: SyncConfig,
    pub(crate) input_delay_config: InputDelayConfig,
}

impl InputTimelineConfig {
    pub fn new(sync_config: SyncConfig, input_delay: InputDelayConfig) -> Self {
        Self {
            sync: sync_config,
            input_delay_config: input_delay,
        }
    }

    pub fn with_input_delay(mut self, input_delay: InputDelayConfig) -> Self {
        self.input_delay_config = input_delay;
        self
    }

    pub fn with_sync_config(mut self, sync_config: SyncConfig) -> Self {
        self.sync = sync_config;
        self
    }

    /// Returns the true if the timeline is configured for deterministic lockstep mode,
    /// where all the latency is covered by input delay, and no prediction is done.
    #[inline]
    pub fn is_lockstep(&self) -> bool {
        self.input_delay_config.is_lockstep()
    }

    /// Update the input delay based on the current RTT and tick duration
    /// when there is a SyncEvent
    pub(crate) fn recompute_input_delay_on_sync(
        trigger: On<SyncEvent<InputTimelineConfig>>,
        tick_duration: Res<TickDuration>,
        mut query: Query<(&Link, &mut InputTimeline, &InputTimelineConfig)>,
    ) {
        if let Ok((link, mut timeline, config)) = query.get_mut(trigger.entity) {
            timeline.input_delay_ticks = config.input_delay_config.input_delay_ticks(
                link.stats,
                &config.sync,
                tick_duration.0,
            );
            trace!(
                "Recomputing input delay on sync event! Input delay ticks: {}",
                timeline.input_delay_ticks
            );
        }
    }

    // TODO: we want to limit this when only the config updates, not the timeline itself!
    //  disabling this for now
    /// Update the input delay based on the current RTT and tick duration
    /// when the InputDelayConfig is updated
    pub(crate) fn recompute_input_delay_on_config_update(
        trigger: On<Insert, InputTimelineConfig>,
        tick_duration: Res<TickDuration>,
        mut query: Query<(&Link, &mut InputTimeline, &InputTimelineConfig)>,
    ) {
        if let Ok((link, mut timeline, config)) = query.get_mut(trigger.entity) {
            timeline.input_delay_ticks = config.input_delay_config.input_delay_ticks(
                link.stats,
                &config.sync,
                tick_duration.0,
            );
            trace!(
                "Recomputing input delay on config update! Input delay ticks: {}. Config: {:?}",
                timeline.input_delay_ticks, config.input_delay_config
            );
        }
    }
}

impl Default for InputTimelineConfig {
    fn default() -> Self {
        Self {
            sync: SyncConfig::default(),
            input_delay_config: InputDelayConfig::no_input_delay(),
        }
    }
}

#[derive(Debug, Reflect)]
pub struct InputContext {
    sync: SyncContext,
    /// Current input_delay_ticks that are being applied
    input_delay_ticks: u16,
    relative_speed: f32,
    is_synced: bool,
}

impl InputContext {
    /// Return the input delay in number of ticks
    pub fn input_delay(&self) -> u16 {
        self.input_delay_ticks
    }
}

impl Default for InputContext {
    fn default() -> Self {
        Self {
            sync: SyncContext::default(),
            input_delay_ticks: 0,
            relative_speed: 1.0,
            is_synced: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Reflect)]
pub struct InputDelayConfig {
    /// Minimum number of input delay ticks that will be applied, regardless of latency.
    ///
    /// This should almost always be set to 0 to ensure that your game is as responsive as possible.
    /// Some games might prefer enforcing a minimum input delay to ensure a consistent game feel even
    /// when the latency conditions are changing.
    pub minimum_input_delay_ticks: u16,
    /// Maximum amount of input delay that will be applied in order to cover latency, before any prediction
    /// is done to cover additional latency.
    ///
    /// Input delay can be ideal in low-latency situations to avoid rollbacks and networking artifacts, but it
    /// must be balanced against the responsiveness of the game. Even at higher latencies, it's useful to add
    /// some input delay to reduce the amount of rollback ticks that are needed. (to reduce the rollback visual artifacts
    /// and CPU costs)
    ///
    /// The default value is 3 (or about 50ms at 60Hz): for clients that have less than 50ms ping, we will apply input delay
    /// to cover the latency, and there should no rollback.
    ///
    /// Set to 0ms if you won't want any input delay. (for example for shooters)
    pub maximum_input_delay_before_prediction: u16,
    /// This setting describes how far ahead the client simulation is allowed to predict to cover latency.
    /// This controls the maximum amount of rollback ticks. Any additional latency will be covered by adding more input delays.
    ///
    /// The default value is 7 ticks (or about 100ms of prediction at 60Hz)
    ///
    /// If you set `maximum_input_delay_before_prediction` to 50ms and `maximum_predicted_time` to 100ms, and the client has:
    /// - 30ms ping: there will be 30ms of input delay and no prediction
    /// - 120ms ping: there will be 50ms of input delay and 70ms of prediction/rollback
    /// - 200ms ping: there will be 100ms of input delay, and 100ms of prediction/rollback
    pub maximum_predicted_ticks: u16,
}

impl InputDelayConfig {
    /// Cover up to 50ms of latency with input delay, and after that use prediction for up to 100ms
    /// - `minimum_input_delay_ticks`: no minimum input delay
    /// - `minimum_input_delay_before_prediction`: 3 ticks (or about 50ms at 60Hz), cover 50ms of latency with input delay
    /// - `maximum_predicted_ticks`: 7 ticks (or about 100ms at 60Hz), cover the next 100ms of latency with prediction
    ///   (the rest will be covered by more input delay)
    pub fn balanced() -> Self {
        Self {
            minimum_input_delay_ticks: 0,
            maximum_input_delay_before_prediction: 3,
            maximum_predicted_ticks: 7,
        }
    }

    /// No input-delay, all the latency will be covered by prediction. We have unlimited prediction
    pub fn no_input_delay() -> Self {
        Self {
            minimum_input_delay_ticks: 0,
            maximum_input_delay_before_prediction: 0,
            maximum_predicted_ticks: 100,
        }
    }

    /// Returns true if we are running in deterministic lockstep mode,
    /// meaning that all the latency is covered by input delay, and no prediction is done
    #[inline]
    pub fn is_lockstep(&self) -> bool {
        self.maximum_predicted_ticks == 0
    }

    /// All the latency will be covered by adding input-delay
    pub fn no_prediction() -> Self {
        Self {
            minimum_input_delay_ticks: 0,
            maximum_input_delay_before_prediction: 0,
            maximum_predicted_ticks: 0,
        }
    }

    pub fn fixed_input_delay(delay_ticks: u16) -> Self {
        Self {
            minimum_input_delay_ticks: delay_ticks,
            maximum_input_delay_before_prediction: delay_ticks,
            maximum_predicted_ticks: 100,
        }
    }

    /// Compute the amount of input delay that should be applied, considering the current RTT
    fn input_delay_ticks(
        &self,
        link_stats: LinkStats,
        sync_config: &SyncConfig,
        tick_interval: Duration,
    ) -> u16 {
        let jitter_margin = sync_config.jitter_margin(link_stats.jitter);
        let effective_rtt = link_stats.rtt + jitter_margin;
        assert!(
            self.minimum_input_delay_ticks <= self.maximum_input_delay_before_prediction,
            "The minimum amount of input_delay should be less than or equal to the maximum_input_delay_before_prediction"
        );
        let mut rtt_ticks =
            (effective_rtt.as_nanos() as f32 / tick_interval.as_nanos() as f32).ceil() as u16;

        // if we're in lockstep mode, we will take extra margin
        if self.is_lockstep() {
            // TODO: make this configurable!
            rtt_ticks += 2;
        }
        // if the rtt is lower than the minimum input delay, we will apply the minimum input delay
        if rtt_ticks <= self.minimum_input_delay_ticks {
            return self.minimum_input_delay_ticks;
        }
        // else, apply input delay up to the maximum input delay
        if rtt_ticks <= self.maximum_input_delay_before_prediction {
            return rtt_ticks;
        }
        // else, apply input delay up to the maximum input delay, and cover the rest with prediction
        // if not possible, add even more input delay
        if rtt_ticks <= (self.maximum_predicted_ticks + self.maximum_input_delay_before_prediction)
        {
            self.maximum_input_delay_before_prediction
        } else {
            rtt_ticks - self.maximum_predicted_ticks
        }
    }
}

/// Timeline that is used to keep track of when the client should buffer inputs.
///
/// This timeline is synced with the server timeline, and is the main driving timeline:
/// any speed adjustments applied to this timeline will also be applied to the `Time<Virtual>` timeline.
/// (and will therefore affect how fast the FixedUpdate loop runs, and how ticks are incremented)
///
/// This timeline is updated in PostUpdate; it CANNOT be used to get accurate `tick` in PreUpdate or Update;
/// use `LocalTimeline` instead.
#[derive(Component, Deref, DerefMut, Default, Debug, Reflect)]
pub struct InputTimeline(pub Timeline<InputTimelineConfig>);

impl TimelineConfig for InputTimelineConfig {
    type Context = InputContext;
    type Timeline = InputTimeline;
}

impl SyncedTimeline for InputTimeline {
    /// We want the Predicted timeline to be:
    /// - RTT/2 ahead of the server timeline, so that inputs sent from the server arrive on time
    /// - On top of that, we will take a bit of margin based on the jitter
    /// - we can reduce the ahead-delay by the input_delay
    ///
    /// Because of the input-delay, the time we return might be in the past compared with the main timeline
    fn sync_objective<T: SyncTargetTimeline>(
        &self,
        remote: &T,
        config: &Self::Config,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> TickInstant {
        let remote = remote.current_estimate();
        let network_delay = TickDelta::from_duration(ping_manager.rtt() / 2, tick_duration);
        let jitter_margin = TickDelta::from_duration(
            config.sync.jitter_margin(ping_manager.jitter()),
            tick_duration,
        );
        let input_delay: TickDelta = Tick(self.context.input_delay_ticks).into();
        // NOTE: because of input delay, this could be in the past, which causes issues with Prediction
        // let's make sure that we're always ahead of the server
        let mut obj = remote + network_delay + jitter_margin - input_delay;
        if obj < remote {
            obj = remote + TickDelta::from_i16(1)
        }
        trace!(
            ?remote,
            ?network_delay,
            ?jitter_margin,
            ?input_delay,
            "InputTimeline objective: {:?}",
            obj
        );
        obj
    }

    fn resync(&mut self, sync_objective: TickInstant) -> i16 {
        let now = self.now();
        self.now = sync_objective;
        (sync_objective - now).to_i16()
    }

    /// Adjust the current timeline to stay in sync with the [`RemoteTimeline`].
    ///
    /// Most of the times this will just be slight nudges to modify the speed of the [`SyncedTimeline`].
    /// If there's a big discrepancy, we will snap the [`SyncedTimeline`] to the [`RemoteTimeline`] by sending a SyncEvent
    ///
    /// [`RemoteTimeline`]: super::remote::RemoteTimeline
    fn sync<T: SyncTargetTimeline>(
        &mut self,
        main: &T,
        config: &Self::Config,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> Option<i16> {
        // skip syncing if we haven't received enough information
        if ping_manager.pongs_recv < config.sync.handshake_pings as u32 {
            return None;
        }
        let now = self.now();
        let objective = self.sync_objective(main, config, ping_manager, tick_duration);
        let error = now - objective;
        let error_ticks = error.to_f32();
        let adjustment = if !self.is_synced {
            SyncAdjustment::Resync
        } else {
            self.sync.speed_adjustment(&config.sync, error_ticks)
        };
        trace!(
            ?now,
            ?objective,
            ?adjustment,
            ?error_ticks,
            error_margin = ?config.sync.error_margin,
            max_error_margin = ?config.sync.max_error_margin,
            "InputTimeline sync"
        );
        self.is_synced = true;
        match adjustment {
            SyncAdjustment::Resync => {
                return Some(self.resync(objective));
            }
            SyncAdjustment::SpeedAdjust(ratio) => {
                self.set_relative_speed(ratio);
            }
            SyncAdjustment::DoNothing => {
                // within acceptable margins, gradually return to normal speed (1.0)
                let current = self.relative_speed();
                if (current - 1.0).abs() > 0.001 {
                    let new_speed = current + (1.0 - current) * 0.1;
                    self.set_relative_speed(new_speed);
                }
            }
        }
        None
    }

    fn is_synced(&self) -> bool {
        self.is_synced
    }

    fn relative_speed(&self) -> f32 {
        self.relative_speed
    }

    fn set_relative_speed(&mut self, ratio: f32) {
        self.relative_speed = ratio;
    }

    fn reset(&mut self) {
        trace!("Resetting InputTimeline");
        self.is_synced = false;
        self.relative_speed = 1.0;
        self.now = Default::default();
        // TODO: also reset tick duration?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_input_delay_config() {
        let sync_config = SyncConfig::default();
        let config_1 = InputDelayConfig {
            minimum_input_delay_ticks: 2,
            maximum_input_delay_before_prediction: 3,
            maximum_predicted_ticks: 7,
        };
        // 1. Test the minimum input delay
        assert_eq!(
            config_1.input_delay_ticks(
                LinkStats {
                    rtt: Duration::from_millis(10),
                    ..default()
                },
                &sync_config,
                Duration::from_millis(16)
            ),
            2
        );

        // 2. Test the maximum input delay before prediction
        assert_eq!(
            config_1.input_delay_ticks(
                LinkStats {
                    rtt: Duration::from_millis(60),
                    ..default()
                },
                &sync_config,
                Duration::from_millis(16)
            ),
            3
        );

        // 3. Test the maximum predicted delay
        assert_eq!(
            config_1.input_delay_ticks(
                LinkStats {
                    rtt: Duration::from_millis(200),
                    ..default()
                },
                &sync_config,
                Duration::from_millis(16)
            ),
            6
        );
        assert_eq!(
            config_1.input_delay_ticks(
                LinkStats {
                    rtt: Duration::from_millis(300),
                    ..default()
                },
                &sync_config,
                Duration::from_millis(16)
            ),
            12
        );
    }
}
