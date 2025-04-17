use crate::ping::manager::PingManager;
use crate::timeline::sync::{SyncConfig, SyncEvent, SyncedTimeline};
use bevy::prelude::{Component, Query, Reflect, Trigger};
use core::time::Duration;
use lightyear_core::tick::Tick;
use lightyear_core::time::{TickDelta, TickInstant};
use lightyear_core::timeline::{NetworkTimeline, Timeline};
use lightyear_link::{Link, LinkStats};


/// Timeline that is used to make sure that Inputs from this peer will arrive on time
/// on the remote peer
#[derive(Debug, Clone, Copy, Reflect)]
pub struct Input {
    pub(crate) config: SyncConfig,
    /// Current input_delay_ticks that are being applied
    pub(crate) input_delay_ticks: u16,
    relative_speed: f32,

    pub(crate) input_delay_config: InputDelayConfig,

}

impl Input {
    // TODO: currently this is fixed, but the input delay should be updated everytime we have a
    //  SyncEvent. We want to make it configurable based on prediction settings.
    /// Return the input delay in number of ticks
    pub fn input_delay(&self) -> u16 {
        self.input_delay_ticks
    }

    /// Update the input delay based on the current RTT and tick duration
    /// when there is a SyncEvent
    pub(crate) fn recompute_input_delay(
        trigger: Trigger<SyncEvent<Input>>,
        mut query: Query<(&Link, &mut Timeline<Input>)>
    ) {
        if let Ok((link, mut timeline)) = query.get_mut(trigger.target()) {
            let rtt = link.stats.rtt;
            let tick_duration = timeline.tick_duration;
            timeline.input_delay_ticks =  timeline.input_delay_config.input_delay_ticks(rtt, tick_duration);
        }
    }
}

impl Default for Input {
    fn default() -> Self {
        Self {
            config: SyncConfig::default(),
            input_delay_ticks: 0,
            relative_speed: 1.0,
            input_delay_config: InputDelayConfig::no_input_delay(),
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

    /// No input-delay, all the latency will be covered by prediction
    pub fn no_input_delay() -> Self {
        Self {
            minimum_input_delay_ticks: 0,
            maximum_input_delay_before_prediction: 0,
            maximum_predicted_ticks: 100,
        }
    }

    /// All the latency will be covered by adding input-delay
    pub fn no_prediction() -> Self {
        Self {
            minimum_input_delay_ticks: 0,
            maximum_input_delay_before_prediction: 0,
            maximum_predicted_ticks: 0,
        }
    }

    /// Compute the amount of input delay that should be applied, considering the current RTT
    fn input_delay_ticks(&self, rtt: Duration, tick_interval: Duration) -> u16 {
        assert!(self.minimum_input_delay_ticks <= self.maximum_input_delay_before_prediction,
                "The minimum amount of input_delay should be lower than the maximum_input_delay_before_prediction");
        let rtt_ticks = (rtt.as_nanos() as f32 / tick_interval.as_nanos() as f32).ceil() as u16;
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

pub type InputTimeline = Timeline<Input>;

impl SyncedTimeline for Timeline<Input> {
    // TODO: how can we make this configurable? or maybe just store the TICK_DURATION in the timeline itself?

    /// We want the Predicted timeline to be:
    /// - RTT/2 ahead of the server timeline, so that inputs sent from the server arrive on time
    /// - On top of that, we will take a bit of margin based on the jitter
    /// - we can reduce the ahead-delay by the input_delay
    /// Because of the input-delay, the time we return might be in the past compared with the main timeline
    fn sync_objective<T: NetworkTimeline>(&self, main: &T, ping_manager: &PingManager) -> TickInstant {
        // TODO: should we do current estimate? or Server::now() already does that?
        let target = main.now();
        let network_delay = TickDelta::from_duration(ping_manager.rtt() / 2, self.tick_duration());
        let jitter_margin = TickDelta::from_duration(ping_manager.jitter() * self.context.config.jitter_multiple_margin as u32 + self.tick_duration() * self.context.config.tick_margin as u32, self.tick_duration());
        let input_delay: TickDelta = Tick(self.context.input_delay_ticks).into();
        target + network_delay + jitter_margin - input_delay
    }

    fn resync(&mut self, sync_objective: TickInstant) -> SyncEvent<Self> {
        let now = self.now();
        let target = sync_objective;
        self.now = target;
        SyncEvent {
            tick_delta: (target-now).to_i16(),
            marker: core::marker::PhantomData
        }
    }

    /// Adjust the current timeline to stay in sync with the [`MainTimeline`].
    ///
    /// Most of the times this will just be slight nudges to modify the speed of the [`SyncedTimeline`].
    /// If there's a big discrepancy, we will snap the [`SyncedTimeline`] to the [`MainTimeline`] by sending a SyncEvent
    fn sync<T: NetworkTimeline>(&mut self, main: &T, ping_manager: &PingManager) -> Option<SyncEvent<Self>> {
        // skip syncing if we haven't received enough information
        if ping_manager.pongs_recv < self.context.config.handshake_pings as u32 {
            return None
        }
        // TODO: should we call current_estimate()? now() should basically return the same thing
        let target = main.now();
        let objective = self.sync_objective(main, ping_manager);

        let error = objective - target;
        let is_ahead = error.is_positive();
        let error_duration = error.to_duration(self.tick_duration());
        let error_margin = self.tick_duration().mul_f32(self.context.config.error_margin);
        let max_error_margin = self.tick_duration().mul_f32(self.context.config.max_error_margin);
        if error_duration > max_error_margin {
            return Some(self.resync(objective));
        } else if error_duration > error_margin {
            let ratio = if is_ahead {
                1.0 / self.context.config.speedup_factor
            } else {
                1.0 * self.context.config.speedup_factor
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