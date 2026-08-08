use crate::ping::manager::PingManager;
use crate::timeline::sync::{SyncConfig, SyncContext, SyncTargetTimeline, TimelineSync};

use bevy_ecs::change_detection::Tick as ChangeTick;
use bevy_ecs::prelude::*;
use bevy_ecs::query::FilteredAccessSet;
use bevy_ecs::system::{ReadOnlySystemParam, SystemMeta, SystemParam, SystemParamValidationError};
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_reflect::Reflect;
use core::{marker::PhantomData, time::Duration};
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_core::time::{TickDelta, TickInstant};
use lightyear_core::timeline::{LocalTimeline, LocalTimelineShift};
use lightyear_link::{Link, LinkStats};
use tracing::trace;

/// Configuration shared by local clock synchronization and input scheduling.
///
/// Input delay is part of the clock objective because it affects how far
/// the local simulation must leaf the remote simulation.
#[derive(Debug, Resource, Reflect)]
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
    /// Maximum number of ticks the local simulation may predict beyond confirmed remote input.
    #[inline]
    pub fn maximum_predicted_ticks(&self) -> u16 {
        self.input_delay_config.maximum_predicted_ticks
    }

    /// Recompute input delay after the global local timeline shifts by whole ticks.
    pub(crate) fn recompute_input_delay_on_local_timeline_shift(
        _trigger: On<LocalTimelineShift>,
        tick_duration: Res<TickDuration>,
        metadata: Res<NetworkingMetadata>,
        links: Query<&Link>,
        config: Res<InputTimelineConfig>,
        mut timeline: ResMut<LocalTimelineSync>,
    ) {
        let before = timeline.input_delay();
        if !timeline.recompute_input_delay(&config, &metadata.mode, &links, tick_duration.0) {
            return;
        }
        trace!(
            target: "lightyear_debug::sync",
            kind = "input_delay_recomputed_on_sync",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            input_delay_ticks_before = before,
            input_delay_ticks_after = timeline.input_delay(),
            topology = ?metadata.mode,
            "local timeline shift: recomputed global input delay"
        );
    }

    /// Recompute input delay when the global configuration resource is inserted or replaced.
    pub(crate) fn recompute_input_delay_on_config_update(
        _trigger: On<Insert, InputTimelineConfig>,
        tick_duration: Res<TickDuration>,
        metadata: Res<NetworkingMetadata>,
        links: Query<&Link>,
        config: Res<InputTimelineConfig>,
        mut timeline: ResMut<LocalTimelineSync>,
    ) {
        if !timeline.recompute_input_delay(&config, &metadata.mode, &links, tick_duration.0) {
            // Configuration is commonly installed before a Client or HostClient connects. The
            // configured minimum is topology-independent; the first LocalTimelineShift will
            // replace it
            // with the latency-aware aggregate once the relevant Links are ready.
            timeline.input_delay_ticks = config.input_delay_config.minimum_input_delay_ticks;
        }
        trace!(
            input_delay_ticks = timeline.input_delay(),
            config = ?config.input_delay_config,
            topology = ?metadata.mode,
            "recomputed global input delay after config update"
        );
    }
}

/// Number of confirmed-input ticks that must be recovered before a prediction-window wait ends.
///
/// Waiting at the exact prediction limit and immediately resuming after one newly confirmed tick
/// would alternate between running and waiting every fixed tick. Requiring two ticks of headroom
/// lets the simulation make useful progress after it resumes.
pub const PREDICTION_WINDOW_HYSTERESIS_TICKS: u16 = 2;

/// Application-global signal that stops deterministic simulation at its safe prediction limit.
///
/// Deterministic replication updates this resource from the minimum confirmed-input frontier
/// across all remote players and registered input types. The driving timeline synchronization
/// system reads it when applying its speed to `Time<Virtual>`: while waiting, virtual time receives
/// an effective relative speed of zero, but the timeline's phase-controller speed is preserved.
///
/// This signal is deliberately specific to confirmed-input availability. P2P phase lead continues
/// to use the normal timeline slowdown controller and does not request a hard wait.
#[derive(Resource, Debug, Clone, Copy, Reflect)]
pub struct PredictionWindowWait {
    waiting: bool,
    origin_tick: Option<Tick>,
    current_tick: Tick,
    confirmed_tick: Option<Tick>,
    prediction_depth: i32,
    maximum_predicted_ticks: u16,
}

impl Default for PredictionWindowWait {
    fn default() -> Self {
        Self {
            waiting: false,
            origin_tick: None,
            current_tick: Tick(0),
            confirmed_tick: None,
            prediction_depth: 0,
            maximum_predicted_ticks: 0,
        }
    }
}

impl PredictionWindowWait {
    /// Returns whether deterministic simulation must currently wait for remote input.
    #[inline]
    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    /// Current local lead over the global confirmed-input frontier, in ticks.
    ///
    /// This can be negative when input delay has provided confirmed remote input for future ticks.
    #[inline]
    pub fn prediction_depth(&self) -> i32 {
        self.prediction_depth
    }

    /// Latest global confirmed-input frontier used by the wait decision.
    #[inline]
    pub fn confirmed_tick(&self) -> Option<Tick> {
        self.confirmed_tick
    }

    /// Effective prediction limit used by the wait decision.
    #[inline]
    pub fn maximum_predicted_ticks(&self) -> u16 {
        self.maximum_predicted_ticks
    }

    /// Clear all state when there is no active deterministic input session.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Update the wait decision from the current and globally confirmed input ticks.
    ///
    /// `maximum_predicted_ticks` must already account for the rollback-history bound. The method
    /// starts waiting when another fixed tick would exceed that bound, and resumes only after
    /// [`PREDICTION_WINDOW_HYSTERESIS_TICKS`] ticks of headroom have been recovered.
    ///
    /// Before any remote input exists, the first observed local tick becomes the session origin.
    /// One initial fixed tick is allowed so the fixed input pipeline can capture and send its first
    /// sample even for a zero-sized prediction window.
    ///
    /// Returns `true` when the waiting state changed.
    pub fn update(
        &mut self,
        current_tick: Tick,
        confirmed_tick: Option<Tick>,
        maximum_predicted_ticks: u16,
    ) -> bool {
        let origin_tick = *self.origin_tick.get_or_insert(current_tick);
        let frontier = confirmed_tick.unwrap_or(origin_tick);
        let prediction_depth = current_tick - frontier;
        let maximum = i32::from(maximum_predicted_ticks);
        // A window smaller than the preferred hysteresis cannot require future input beyond the
        // current tick: if every peer waited there, no one could capture that future input. Fall
        // back to resuming at zero prediction depth for those small windows.
        let resume_at =
            i32::from(maximum_predicted_ticks.saturating_sub(PREDICTION_WINDOW_HYSTERESIS_TICKS));

        let waiting = if confirmed_tick.is_none() && current_tick == origin_tick {
            false
        } else if self.waiting {
            prediction_depth > resume_at
        } else {
            prediction_depth >= maximum
        };
        let changed = waiting != self.waiting;

        self.waiting = waiting;
        self.current_tick = current_tick;
        self.confirmed_tick = confirmed_tick;
        self.prediction_depth = prediction_depth;
        self.maximum_predicted_ticks = maximum_predicted_ticks;
        changed
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

/// Runtime state for synchronizing the application-global [`LocalTimeline`].
///
/// This resource is not a clock. The authoritative fractional simulation instant is constructed
/// from `LocalTimeline` and `Time<Fixed>` when synchronization runs.
#[derive(Resource, Debug, Reflect)]
pub struct LocalTimelineSync {
    controller: SyncContext,
    /// Current input_delay_ticks that are being applied
    input_delay_ticks: u16,
    relative_speed: f32,
    is_synced: bool,
}

impl LocalTimelineSync {
    /// Return the input delay in number of ticks
    pub fn input_delay(&self) -> u16 {
        self.input_delay_ticks
    }

    /// Return whether the local simulation clock has completed initial synchronization.
    pub fn is_synced(&self) -> bool {
        self.is_synced
    }

    /// Override synchronization readiness.
    ///
    /// This is primarily intended for session drivers that do not require remote sampling, such
    /// as in-process host clients. Normal network clients are updated by the sync plugin.
    pub fn set_synced(&mut self, synced: bool) {
        self.is_synced = synced;
    }

    /// Return the pacing factor requested by network clock synchronization.
    pub fn relative_speed(&self) -> f32 {
        self.relative_speed
    }
}

impl Default for LocalTimelineSync {
    fn default() -> Self {
        Self {
            controller: SyncContext::default(),
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
        let jitter_margin = sync_config.jitter_margin(link_stats.jitter, tick_interval);
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

/// Read-only access to the application-global [`LocalTimeline`] after it has synchronized.
///
/// This parameter combines the canonical simulation clock with its [`LocalTimelineSync`]
/// controller. Systems using it are skipped until synchronization is ready, and can read both the
/// current simulation tick and the input delay without fetching the controller separately.
pub struct SyncedLocalTimeline<'w, 's> {
    timeline: Res<'w, LocalTimeline>,
    sync: Res<'w, LocalTimelineSync>,
    marker: PhantomData<&'s ()>,
}

impl core::ops::Deref for SyncedLocalTimeline<'_, '_> {
    type Target = LocalTimeline;

    fn deref(&self) -> &Self::Target {
        &self.timeline
    }
}

impl SyncedLocalTimeline<'_, '_> {
    /// Return the canonical simulation tick.
    pub fn current_tick(&self) -> Tick {
        self.timeline.tick()
    }

    /// Return the number of ticks added when assigning local inputs to the simulation timeline.
    pub fn input_delay(&self) -> u16 {
        self.sync.input_delay()
    }
}

// SAFETY: This parameter delegates all access to read-only resource parameters.
unsafe impl SystemParam for SyncedLocalTimeline<'_, '_> {
    type State = (
        <Res<'static, LocalTimeline> as SystemParam>::State,
        <Res<'static, LocalTimelineSync> as SystemParam>::State,
    );
    type Item<'world, 'state> = SyncedLocalTimeline<'world, 'state>;

    fn init_state(world: &mut World) -> Self::State {
        (
            <Res<'static, LocalTimeline> as SystemParam>::init_state(world),
            <Res<'static, LocalTimelineSync> as SystemParam>::init_state(world),
        )
    }

    fn init_access(
        state: &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        <Res<'static, LocalTimeline> as SystemParam>::init_access(
            &state.0,
            system_meta,
            component_access_set,
            world,
        );
        <Res<'static, LocalTimelineSync> as SystemParam>::init_access(
            &state.1,
            system_meta,
            component_access_set,
            world,
        );
    }

    #[inline]
    unsafe fn get_param<'world, 'state>(
        state: &'state mut Self::State,
        system_meta: &SystemMeta,
        world: UnsafeWorldCell<'world>,
        change_tick: ChangeTick,
    ) -> Result<Self::Item<'world, 'state>, SystemParamValidationError> {
        // SAFETY: `init_access` delegated to both read-only resources, and the caller guarantees
        // that this is the same World used by `init_state`.
        let timeline = unsafe {
            <Res<'static, LocalTimeline> as SystemParam>::get_param(
                &mut state.0,
                system_meta,
                world,
                change_tick,
            )
        }?;
        let sync = unsafe {
            <Res<'static, LocalTimelineSync> as SystemParam>::get_param(
                &mut state.1,
                system_meta,
                world,
                change_tick,
            )
        }?;
        if !sync.is_synced() {
            return Err(SystemParamValidationError::skipped::<Self>(
                "LocalTimeline is not synchronized",
            ));
        }
        Ok(SyncedLocalTimeline {
            timeline,
            sync,
            marker: PhantomData,
        })
    }
}

// SAFETY: `SyncedLocalTimeline` only delegates to read-only system parameters.
unsafe impl ReadOnlySystemParam for SyncedLocalTimeline<'_, '_> {}

impl LocalTimelineSync {
    /// Recompute the global input delay from the Links selected by the current topology.
    ///
    /// Conventional modes use their sole client Link. P2P uses the maximum required delay across
    /// all connected peer Links so that one tick-indexed local input stream is safe for every peer.
    /// Returns `false` when the topology has no complete set of ready Link statistics.
    pub(crate) fn recompute_input_delay(
        &mut self,
        config: &InputTimelineConfig,
        topology: &NetworkTopology,
        links: &Query<&Link>,
        tick_duration: Duration,
    ) -> bool {
        let delay_for = |entity| {
            links.get(entity).ok().map(|link| {
                config
                    .input_delay_config
                    .input_delay_ticks(link.stats, &config.sync, tick_duration)
            })
        };
        let input_delay_ticks = match topology {
            NetworkTopology::Client(entity) => delay_for(*entity),
            NetworkTopology::HostClient { client, .. } => delay_for(*client),
            NetworkTopology::P2P { connected, .. } if !connected.is_empty() => connected
                .iter()
                .try_fold(0, |maximum, entity| Some(maximum.max(delay_for(*entity)?))),
            NetworkTopology::Undefined
            | NetworkTopology::Server(_)
            | NetworkTopology::P2P { .. }
            | NetworkTopology::Invalid(_) => None,
        };
        let Some(input_delay_ticks) = input_delay_ticks else {
            return false;
        };
        self.input_delay_ticks = input_delay_ticks;
        true
    }
}

impl TimelineSync for LocalTimelineSync {
    type Config = InputTimelineConfig;

    fn sync_context(&mut self) -> &mut SyncContext {
        &mut self.controller
    }

    fn sync_config(config: &Self::Config) -> &SyncConfig {
        &config.sync
    }

    /// Compute the desired local simulation instant.
    ///
    /// The local simulation runs far enough ahead of the remote timeline for inputs to arrive,
    /// with RTT and jitter margins. Input delay reduces that required clock lead because inputs are
    /// assigned to later ticks without advancing the simulation clock itself.
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
            config
                .sync
                .jitter_margin(ping_manager.jitter(), tick_duration),
            tick_duration,
        );
        let input_delay: TickDelta = Tick(self.input_delay_ticks as u32).into();
        let sync_error_margin = TickDelta::from_duration(
            tick_duration.mul_f32(config.sync.error_margin),
            tick_duration,
        );
        // Inputs received by the server in `PreUpdate` are first read in
        // `FixedPreUpdate`, after `LocalTimeline` advances in `FixedFirst`.
        // Therefore an input packet that arrives while the server is at tick T
        // must contain inputs for at least tick T + 1.
        //
        // `sync_error_margin` compensates for the sync controller's allowed
        // deadband: the controller may let the local timeline drift behind the
        // objective by this much without correcting.
        let obj =
            remote + network_delay + jitter_margin + TickDelta::from_i32(1) + sync_error_margin
                - input_delay;
        trace!(
            ?remote,
            ?network_delay,
            ?jitter_margin,
            ?sync_error_margin,
            ?input_delay,
            "LocalTimeline sync objective: {:?}",
            obj
        );
        obj
    }

    fn resync(&mut self, now: TickInstant, sync_objective: TickInstant) -> i32 {
        (sync_objective - now).to_i32()
    }

    fn is_synced(&self) -> bool {
        self.is_synced
    }

    fn set_synced(&mut self, synced: bool) {
        self.is_synced = synced;
    }

    fn relative_speed(&self) -> f32 {
        self.relative_speed
    }

    fn set_relative_speed(&mut self, ratio: f32) {
        self.relative_speed = ratio;
    }

    fn reset(&mut self) {
        trace!("Resetting LocalTimelineSync");
        self.controller = SyncContext::default();
        self.is_synced = false;
        self.relative_speed = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::remote::RemoteTimeline;
    use bevy_app::{App, Update};
    use bevy_utils::default;
    use lightyear_core::timeline::NetworkTimeline;

    fn assert_tick_instant_close(actual: TickInstant, expected: TickInstant) {
        let error = (actual - expected).to_f32().abs();
        assert!(
            error < 0.001,
            "expected {expected:?}, got {actual:?}, error {error}"
        );
    }

    #[derive(Resource, Default)]
    struct SyncedParamRuns(u8);

    #[test]
    fn prediction_window_wait_uses_hysteresis() {
        let mut wait = PredictionWindowWait::default();

        assert!(!wait.update(Tick(100), None, 4));
        assert!(!wait.is_waiting());
        assert_eq!(wait.prediction_depth(), 0);

        assert!(wait.update(Tick(104), None, 4));
        assert!(wait.is_waiting());
        assert_eq!(wait.prediction_depth(), 4);

        // One newly confirmed tick is not enough to resume and immediately hit the limit again.
        assert!(!wait.update(Tick(104), Some(Tick(101)), 4));
        assert!(wait.is_waiting());

        // Two ticks of recovered headroom release the wait.
        assert!(wait.update(Tick(104), Some(Tick(102)), 4));
        assert!(!wait.is_waiting());
        assert_eq!(wait.prediction_depth(), 2);
    }

    #[test]
    fn zero_prediction_window_can_bootstrap_then_wait_for_future_input() {
        let mut wait = PredictionWindowWait::default();

        // The first fixed tick is allowed to capture and send the initial local input sample.
        assert!(!wait.update(Tick(20), None, 0));
        assert!(wait.update(Tick(21), None, 0));
        assert!(wait.is_waiting());

        // A zero-sized window cannot require future input beyond the current tick, or every peer
        // could wait forever. Confirming the current tick releases this degenerate case.
        assert!(wait.update(Tick(21), Some(Tick(21)), 0));
        assert!(!wait.is_waiting());
        assert_eq!(wait.prediction_depth(), 0);
    }

    #[test]
    fn synced_local_timeline_checks_local_sync_readiness() {
        fn require_synced_timeline(
            timeline: SyncedLocalTimeline,
            mut runs: ResMut<SyncedParamRuns>,
        ) {
            assert_eq!(timeline.current_tick(), timeline.tick());
            assert_eq!(timeline.input_delay(), 3);
            runs.0 += 1;
        }

        let mut app = App::new();
        app.init_resource::<LocalTimeline>();
        app.init_resource::<LocalTimelineSync>();
        app.init_resource::<SyncedParamRuns>();
        app.add_systems(Update, require_synced_timeline);

        app.update();
        assert_eq!(app.world().resource::<SyncedParamRuns>().0, 0);

        let mut sync = app.world_mut().resource_mut::<LocalTimelineSync>();
        sync.input_delay_ticks = 3;
        sync.set_synced(true);
        drop(sync);

        app.update();
        assert_eq!(app.world().resource::<SyncedParamRuns>().0, 1);
    }

    #[test]
    fn local_timeline_objective_preserves_margin_after_sync_deadband() {
        let tick_duration = Duration::from_millis(10);
        let mut remote = RemoteTimeline::default();
        remote.set_now(TickInstant::from(Tick(100)));

        let mut ping_manager = PingManager::default();
        ping_manager.rtt_estimator_ewma.final_stats.rtt = Duration::from_millis(40);
        ping_manager.rtt_estimator_ewma.final_stats.jitter = Duration::from_millis(5);

        let mut config = InputTimelineConfig::default();
        config.sync.jitter_multiple = 2;
        config.sync.jitter_margin = 1.0;
        config.sync.error_margin = 0.75;

        let objective = LocalTimelineSync::default().sync_objective(
            &remote,
            &config,
            &ping_manager,
            tick_duration,
        );

        // remote 100 + RTT/2 2 ticks + jitter margin 2 ticks
        // + server input pipeline 1 tick + controller deadband 0.75 ticks.
        assert_tick_instant_close(objective, TickInstant::lit("105.75"));

        let earliest_uncorrected_timeline = objective - TickDelta::lit("0.75");
        // Even if the sync controller chooses not to correct a -0.75 tick error,
        // the client is still at the delivery objective that includes RTT/2 and
        // jitter margin plus the server's one-tick input pipeline delay.
        assert_tick_instant_close(earliest_uncorrected_timeline, TickInstant::lit("105"));
    }

    #[test]
    fn input_delay_still_offsets_local_timeline_objective() {
        let tick_duration = Duration::from_millis(10);
        let mut remote = RemoteTimeline::default();
        remote.set_now(TickInstant::from(Tick(100)));

        let mut ping_manager = PingManager::default();
        ping_manager.rtt_estimator_ewma.final_stats.rtt = Duration::from_millis(40);
        ping_manager.rtt_estimator_ewma.final_stats.jitter = Duration::from_millis(5);

        let mut config =
            InputTimelineConfig::default().with_input_delay(InputDelayConfig::fixed_input_delay(2));
        config.sync.jitter_multiple = 2;
        config.sync.jitter_margin = 1.0;
        config.sync.error_margin = 0.75;

        let mut timeline = LocalTimelineSync::default();
        timeline.input_delay_ticks = 2;

        let objective = timeline.sync_objective(&remote, &config, &ping_manager, tick_duration);

        assert_tick_instant_close(objective, TickInstant::lit("103.75"));
    }

    /// The server reads inputs in `FixedPreUpdate`, after receiving packets in
    /// `PreUpdate` and advancing its tick in `FixedFirst`. Inputs sent by the
    /// client must therefore target at least `remote + 1`, even under
    /// worst-case controller drift (`offset = -error_margin`).
    ///
    /// The post-replicon `+sync_error_margin` term in the objective
    /// cancels with the symmetric controller drift, leaving the safety
    /// margin riding on the explicit one-tick server input pipeline margin
    /// plus `network_delay + jitter_margin`.
    #[test]
    fn sync_objective_keeps_sent_input_tick_ahead_under_worst_case_drift() {
        let tick_duration = Duration::from_millis(10);
        let mut remote = RemoteTimeline::default();
        remote.set_now(TickInstant::from(Tick(100)));

        // Localhost — zero RTT, zero jitter.
        let mut ping_manager = PingManager::default();
        ping_manager.rtt_estimator_ewma.final_stats.rtt = Duration::ZERO;
        ping_manager.rtt_estimator_ewma.final_stats.jitter = Duration::ZERO;

        // User tightens `jitter_margin` below 1.0 for snappier sync, while
        // still using input delay. The local tick itself can be behind
        // `remote + 1`; the sent input tick must not be.
        let mut config =
            InputTimelineConfig::default().with_input_delay(InputDelayConfig::fixed_input_delay(2));
        config.sync.jitter_margin = 0.5;
        assert!(
            config.sync.error_margin >= 1.0,
            "test premise: error_margin is at least 1 tick"
        );

        let mut timeline = LocalTimelineSync::default();
        timeline.input_delay_ticks = 2;
        let objective = timeline.sync_objective(&remote, &config, &ping_manager, tick_duration);

        // Controller may legitimately let `local - objective` reach
        // `-error_margin` without correcting (see `SyncContext::speed_adjustment`).
        let worst_case_drift = TickDelta::from_duration(
            tick_duration.mul_f32(config.sync.error_margin),
            tick_duration,
        );
        let worst_case_local = objective - worst_case_drift;
        let sent_input_tick = worst_case_local + TickDelta::from_i32(2);

        let required_input_tick = TickInstant::from(Tick(101));
        assert!(
            sent_input_tick >= required_input_tick,
            "worst-case sent input tick is {sent_input_tick:?}, but the \
             server reads inputs in FixedPreUpdate after receiving packets in \
             PreUpdate and advancing in FixedFirst, so the packet must contain \
             input for at least {required_input_tick:?} (= remote + 1).",
        );
    }

    #[test]
    fn p2p_input_delay_uses_maximum_across_links() {
        let mut app = App::new();
        let mut fast_link = Link::default();
        fast_link.stats.rtt = Duration::from_millis(10);
        let fast = app.world_mut().spawn(fast_link).id();
        let mut slow_link = Link::default();
        slow_link.stats.rtt = Duration::from_millis(50);
        let slow = app.world_mut().spawn(slow_link).id();

        let mut metadata = NetworkingMetadata::default();
        metadata.mode = NetworkTopology::P2P {
            connected: [fast, slow].into_iter().collect(),
            declared_links: 2,
        };
        app.insert_resource(metadata);
        app.insert_resource(TickDuration(Duration::from_millis(10)));
        let mut config =
            InputTimelineConfig::default().with_input_delay(InputDelayConfig::balanced());
        config.sync.jitter_multiple = 0;
        config.sync.jitter_margin = 0.0;
        app.insert_resource(config);
        app.init_resource::<LocalTimelineSync>();
        app.add_systems(
            Update,
            |mut timeline: ResMut<LocalTimelineSync>,
             config: Res<InputTimelineConfig>,
             metadata: Res<NetworkingMetadata>,
             links: Query<&Link>,
             tick_duration: Res<TickDuration>| {
                assert!(timeline.recompute_input_delay(
                    &config,
                    &metadata.mode,
                    &links,
                    tick_duration.0,
                ));
            },
        );

        app.update();

        // The 10ms Link needs one delayed tick, while the 50ms Link reaches the balanced
        // configuration's three-tick pre-prediction cap. The global delay must satisfy both.
        assert_eq!(app.world().resource::<LocalTimelineSync>().input_delay(), 3);
    }

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
            7
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
            13
        );
    }
}
