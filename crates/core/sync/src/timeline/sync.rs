use crate::ping::manager::PingManager;
use crate::plugin::SyncSystems;
use crate::timeline::input::{InputTimelineConfig, LocalTimelineSync, PredictionWindowWait};
use bevy_app::{App, Last, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time, Virtual};
use bevy_utils::prelude::DebugName;
use core::time::Duration;
use lightyear_connection::client::{Client, Connected, Disconnected};
use lightyear_connection::host::HostClient;
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
use lightyear_connection::p2p::P2P;
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::TickInstant;
use lightyear_core::timeline::{LocalTimelineShift, NetworkTimeline};
use tracing::{error, trace};

/// Triggered when a running P2P timeline is too far ahead to correct with bounded pacing.
///
/// A running deterministic peer cannot safely emit a local [`LocalTimelineShift`], because other
/// peers would retain inputs under the old tick labels. The P2P session owner should treat this
/// event as fatal and abort the session, unless it implements a coordinated all-peer
/// resynchronization protocol.
#[derive(Event, Debug, Clone, Copy)]
pub struct P2PTimelineDiverged {
    /// P2P Link whose remote estimate produced the worst local lead.
    pub limiting_link: Entity,
    /// Local lead over the limiting Link, measured in fractional ticks.
    pub lead: f32,
}

/// Runtime controller for synchronizing a local clock or cursor to a remote timeline.
///
/// Implementors define their synchronization objective and how a large correction is applied.
/// The common controller sequence is provided by [`TimelineSync::sync`]: it waits for enough
/// latency samples, computes the current error, snaps on initial synchronization, and otherwise
/// slews the controlled clock through [`SyncContext`].
///
/// This trait intentionally does not require [`NetworkTimeline`]. [`LocalTimelineSync`] controls
/// the application-global `LocalTimeline` without owning a second clock, while interpolation owns
/// and directly adjusts its independent presentation timeline.
pub trait TimelineSync {
    /// Settings used by this synchronization policy.
    type Config;

    /// Return the mutable hysteresis state used by the common speed controller.
    fn sync_context(&mut self) -> &mut SyncContext;

    /// Select the common controller settings from this policy's configuration.
    fn sync_config(config: &Self::Config) -> &SyncConfig;

    /// Get the ideal [`TickInstant`] that the controlled clock should follow.
    fn sync_objective<Remote: SyncTargetTimeline>(
        &self,
        other: &Remote,
        config: &Self::Config,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> TickInstant;

    /// Apply a discontinuous correction and return its whole-tick delta.
    ///
    /// `now` is explicit because not every controller owns the clock it disciplines.
    fn resync(&mut self, now: TickInstant, sync_objective: TickInstant) -> i32;

    /// Synchronize the controlled clock to `remote` from its current `now` instant.
    ///
    /// Usually this slightly changes the relative speed. Initial synchronization and errors larger
    /// than [`SyncConfig::max_error_margin`] instead call [`TimelineSync::resync`]. The returned
    /// delta lets the caller propagate a discontinuity to any tick-indexed state it owns.
    fn sync<Remote: SyncTargetTimeline>(
        &mut self,
        now: TickInstant,
        remote: &Remote,
        config: &Self::Config,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> Option<i32>
    where
        Self: Sized,
    {
        let sync_config = Self::sync_config(config);
        if ping_manager.latency_samples_recv() < sync_config.handshake_pings as u32 {
            return None;
        }

        let objective = self.sync_objective(remote, config, ping_manager, tick_duration);
        let error_ticks = (now - objective).to_f32();
        let adjustment = if !self.is_synced() {
            SyncAdjustment::Resync
        } else {
            self.speed_adjustment(config, error_ticks)
        };
        trace!(
            controller = ?DebugName::type_name::<Self>(),
            ?now,
            ?objective,
            ?adjustment,
            ?error_ticks,
            error_margin = sync_config.error_margin,
            max_error_margin = sync_config.max_error_margin,
            "timeline synchronization sample"
        );
        self.set_synced(true);

        match adjustment {
            SyncAdjustment::Resync => Some(self.resync(now, objective)),
            SyncAdjustment::SpeedAdjust(_) | SyncAdjustment::DoNothing => None,
        }
    }

    /// Apply the common speed controller for an error measured in fractional ticks.
    ///
    /// Implementations apply speed changes and recovery toward `1.0`. A
    /// [`SyncAdjustment::Resync`] result is left to the synchronization policy: conventional
    /// client/server synchronization may snap, while a running P2P session must abort or perform
    /// a coordinated recovery instead.
    fn speed_adjustment(&mut self, config: &Self::Config, offset: f32) -> SyncAdjustment {
        let adjustment = self
            .sync_context()
            .speed_adjustment(Self::sync_config(config), offset);
        match adjustment {
            SyncAdjustment::SpeedAdjust(ratio) => self.set_relative_speed(ratio),
            SyncAdjustment::DoNothing => {
                // Within acceptable margins, gradually return to normal speed.
                let current = self.relative_speed();
                if (current - 1.0).abs() > 0.001 {
                    self.set_relative_speed(current + (1.0 - current) * 0.1);
                }
            }
            SyncAdjustment::Resync => {}
        }
        adjustment
    }

    fn is_synced(&self) -> bool;

    /// Set whether the timeline has completed its initial synchronization.
    fn set_synced(&mut self, synced: bool);

    /// Returns the speed of your timeline relative to your system clock as an `f32`.
    /// A value of `1.0` means the timeline is running at normal speed.
    /// A value of `0.5` means the timeline is running at half speed,
    fn relative_speed(&self) -> f32;

    fn set_relative_speed(&mut self, ratio: f32);

    /// Reset the controller to its initial state (used when a client reconnects).
    fn reset(&mut self);
}

pub trait SyncTargetTimeline: NetworkTimeline + Default {
    fn current_estimate(&self) -> TickInstant;

    /// Returns true after this timeline has received enough information to estimate remote time.
    fn is_initialized(&self) -> bool {
        true
    }

    /// Returns true if the SyncTimelines are allowed to use this timeline as a sync target this frame
    fn received_packet(&self) -> bool;
}

/// Immutable settings shared by local-clock and interpolation synchronization controllers.
///
/// The controllers keep their mutable hysteresis in [`SyncContext`]. Local simulation uses these
/// settings to pace the application-global `LocalTimeline`; interpolation uses them to pace its
/// independent presentation timeline.
#[derive(Clone, Copy, Debug, Reflect)]
pub struct SyncConfig {
    /// How much multiple of jitter do we apply as margin when computing the time
    /// a packet will get received by the server
    /// (worst case will be RTT / 2 + jitter * multiple_margin + jitter_margin)
    /// % of packets that will be received within k * jitter
    /// 1: 65%, 2: 95%, 3: 99.7%
    pub jitter_multiple: u8,
    /// Fixed safety margin added on top of the jitter-derived one, expressed as a
    /// fractional number of ticks.
    ///
    /// The default is `1.0`, which guarantees the client timeline lands at least
    /// one full tick ahead of the server timeline under zero-RTT/zero-jitter
    /// conditions (e.g. loopback). Any less than `1.0` risks the server
    /// simulating tick `T` before the client's input for `T` has arrived,
    /// producing a desync in deterministic replication.
    pub jitter_margin: f32,
    /// Number of pings to exchange with the server before finalizing the handshake
    pub handshake_pings: u8,
    /// Error margin for upstream throttle (in multiple of ticks)
    pub error_margin: f32,
    /// If the error is too large, snap the controlled timeline to its objective.
    pub max_error_margin: f32,
    /// How many consecutive errors are allowed before we start adjusting the speed
    pub consecutive_errors_threshold: u8,
    // TODO: instead of constant speedup_factor, the speedup should be linear w.r.t the offset
    /// By how much should we speed up the simulation to make ticks stay in sync with server?
    pub speedup_factor: f32,
}

impl SyncConfig {
    /// Total jitter-based safety margin as a `Duration`, combining the measured
    /// jitter scaled by [`Self::jitter_multiple`] with the fixed fractional-tick
    /// margin in [`Self::jitter_margin`].
    pub fn jitter_margin(&self, jitter: Duration, tick_duration: Duration) -> Duration {
        jitter * self.jitter_multiple as u32 + tick_duration.mul_f32(self.jitter_margin)
    }
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            jitter_multiple: 4,
            jitter_margin: 1.0,
            handshake_pings: 3,
            error_margin: 1.0,
            max_error_margin: 10.0,
            consecutive_errors_threshold: 3,
            speedup_factor: 1.05,
        }
    }
}

/// Mutable hysteresis shared by timeline synchronization controllers.
///
/// This type is public because synchronization policies live in separate Lightyear crates. Its
/// fields and adjustment algorithm remain private; applications normally interact with a
/// [`TimelineSync`] implementation instead.
#[derive(Debug, Reflect)]
pub struct SyncContext {
    /// How many consecutive errors have we seen that are in the same direction
    consecutive_errors: u8,
    /// Sign of the previous error
    previous_error_sign: bool,
}

impl Default for SyncContext {
    fn default() -> Self {
        Self {
            consecutive_errors: 0,
            previous_error_sign: true,
        }
    }
}

#[derive(Debug)]
pub enum SyncAdjustment {
    Resync,
    SpeedAdjust(f32),
    DoNothing,
}

impl SyncContext {
    fn speed_adjustment(&mut self, config: &SyncConfig, offset: f32) -> SyncAdjustment {
        let current_error_sign = offset.is_sign_positive();
        let previous_error_sign = self.previous_error_sign;
        self.previous_error_sign = current_error_sign;
        if offset.abs() > config.max_error_margin {
            self.consecutive_errors = 0;
            SyncAdjustment::Resync
        } else if offset.abs() > config.error_margin {
            self.consecutive_errors = self.consecutive_errors.saturating_add(1);
            // skip if we haven't seen enough consecutive errors in the same direction
            if (current_error_sign ^ previous_error_sign)
                || self.consecutive_errors < config.consecutive_errors_threshold
            {
                self.previous_error_sign = current_error_sign;
                return SyncAdjustment::DoNothing;
            }
            let base_factor = config.speedup_factor - 1.0;
            let error_ratio = (offset.abs() / config.max_error_margin).clamp(0.0, 1.0);

            // Apply progressively stronger adjustment as error increases
            let adjustment = 1.0 + (base_factor * error_ratio * 2.0);

            // Slow down if we are ahead
            let ratio = if offset > 0.0 {
                1.0 / adjustment
            } else {
                adjustment
            };
            SyncAdjustment::SpeedAdjust(ratio)
        } else {
            self.consecutive_errors = 0;
            SyncAdjustment::DoNothing
        }
    }
}

/// Synchronizes the application-global `LocalTimeline` without maintaining a shadow clock.
///
/// `Remote` remains generic so the controller can be tested with deterministic remote estimates.
pub struct LocalTimelineSyncPlugin<Remote> {
    marker: core::marker::PhantomData<Remote>,
}

impl<Remote> Default for LocalTimelineSyncPlugin<Remote> {
    fn default() -> Self {
        Self {
            marker: core::marker::PhantomData,
        }
    }
}

impl<Remote: SyncTargetTimeline> LocalTimelineSyncPlugin<Remote> {
    /// Apply the synchronization controller's pacing factor to Bevy virtual time.
    ///
    /// `LocalTimeline` remains the authoritative tick clock; the synchronization resource only
    /// supplies the relative speed used to make future fixed updates converge on the target.
    fn apply_relative_speed(
        sync: &LocalTimelineSync,
        local_timeline: &LocalTimeline,
        virtual_time: &mut Time<Virtual>,
    ) {
        trace!(
            target: "lightyear_debug::sync",
            kind = "relative_speed",
            schedule = "Last",
            sample_point = "Last",
            timeline = "LocalTimeline",
            tick = local_timeline.tick().0,
            relative_speed = sync.relative_speed(),
            "local timeline synchronization speed applied to Virtual time"
        );
        // TODO: compose this with a relative speed set by the application.
        virtual_time.set_relative_speed(sync.relative_speed());
    }

    /// Reset synchronization controller state when a conventional client session connects.
    ///
    /// P2P links are handled as a topology-wide set by [`Self::sync_timelines`], so an individual
    /// P2P link connecting must not reset an already-running deterministic session.
    fn handle_connect(
        trigger: On<Add, Connected>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        mut sync: ResMut<LocalTimelineSync>,
    ) {
        if clients.get(trigger.entity).is_ok() {
            sync.reset();
        }
    }

    /// Mark an in-process host client synchronized immediately.
    ///
    /// A host client shares the server's world clock and therefore does not need latency samples
    /// or a remote-clock correction before local input systems can run.
    fn handle_host_client(
        trigger: On<Add, HostClient>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        mut sync: ResMut<LocalTimelineSync>,
    ) {
        if clients.get(trigger.entity).is_ok() {
            sync.set_synced(true);
        }
    }

    /// Clear conventional client readiness when its session disconnects.
    ///
    /// P2P readiness follows [`NetworkingMetadata`] so that one link's lifecycle cannot
    /// independently relabel or pause the world clock.
    fn handle_disconnect(
        trigger: On<Add, Disconnected>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        mut sync: ResMut<LocalTimelineSync>,
    ) {
        if clients.get(trigger.entity).is_ok() {
            sync.set_synced(false);
        }
    }

    /// Apply local-clock pacing, prediction-window waiting, or normal speed for this frame.
    ///
    /// Prediction-window exhaustion temporarily overrides the desired synchronization speed with
    /// zero without discarding the controller's stored correction.
    fn update_virtual_time(
        metadata: Res<NetworkingMetadata>,
        prediction_window_wait: Res<PredictionWindowWait>,
        local_timeline: Res<LocalTimeline>,
        sync: Res<LocalTimelineSync>,
        p2p_links: Query<&P2P>,
        mut virtual_time: ResMut<Time<Virtual>>,
    ) {
        let is_synced = sync.is_synced();
        let p2p_timeline =
            metadata.mode.is_p2p() || p2p_links.iter().any(|state| *state == P2P::Candidate);
        if is_synced
            && prediction_window_wait.is_waiting()
            && (metadata.mode.is_client() || p2p_timeline)
        {
            virtual_time.set_relative_speed(0.0);
            trace!(
                target: "lightyear_debug::sync",
                kind = "prediction_window_wait",
                schedule = "Last",
                sample_point = "Last",
                prediction_depth = prediction_window_wait.prediction_depth(),
                maximum_predicted_ticks = prediction_window_wait.maximum_predicted_ticks(),
                confirmed_tick = ?prediction_window_wait.confirmed_tick(),
                desired_relative_speed = sync.relative_speed(),
                "paused virtual time at the deterministic prediction-window limit"
            );
        } else if p2p_timeline
            || (is_synced
                && matches!(
                    metadata.mode,
                    NetworkTopology::Client(_) | NetworkTopology::HostClient { .. }
                ))
        {
            Self::apply_relative_speed(&sync, &local_timeline, &mut virtual_time);
        } else {
            virtual_time.set_relative_speed(1.0);
        }
    }

    /// Synchronize the local simulation instant against one conventional remote clock estimate.
    ///
    /// This preserves the previous `InputTimeline` behavior: wait for latency samples, compute an
    /// objective that includes network margin and subtracts input delay, slew for small errors,
    /// and emit a whole-tick [`LocalTimelineShift`] for large errors.
    fn sync_client_server(
        source: Entity,
        local_now: TickInstant,
        sync: &mut LocalTimelineSync,
        config: &InputTimelineConfig,
        remote: &Remote,
        ping_manager: &PingManager,
        tick_duration: &TickDuration,
        commands: &mut Commands,
    ) {
        if !remote.received_packet() {
            return;
        }
        let remote_estimate = remote.current_estimate();
        let before_speed = sync.relative_speed();
        if let Some(tick_delta) =
            sync.sync(local_now, remote, config, ping_manager, tick_duration.0)
        {
            trace!(
                target: "lightyear_debug::sync",
                kind = "sync_adjustment",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                ?source,
                timeline = "LocalTimeline",
                remote_timeline = ?DebugName::type_name::<Remote>(),
                local_tick = local_now.tick().0,
                remote_tick = remote.tick().0,
                remote_estimate = ?remote_estimate,
                tick_delta,
                relative_speed = sync.relative_speed(),
                rtt_ms = ping_manager.rtt().as_secs_f64() * 1000.0,
                jitter_ms = ping_manager.jitter().as_secs_f64() * 1000.0,
                "local timeline sync emitted LocalTimelineShift"
            );
            commands.trigger(LocalTimelineShift { delta: tick_delta });
        } else {
            trace!(
                target: "lightyear_debug::sync",
                kind = "sync_sample",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                ?source,
                timeline = "LocalTimeline",
                remote_timeline = ?DebugName::type_name::<Remote>(),
                local_tick = local_now.tick().0,
                remote_tick = remote.tick().0,
                remote_estimate = ?remote_estimate,
                relative_speed_before = before_speed,
                relative_speed = sync.relative_speed(),
                rtt_ms = ping_manager.rtt().as_secs_f64() * 1000.0,
                jitter_ms = ping_manager.jitter().as_secs_f64() * 1000.0,
                "local timeline sync sampled"
            );
        }
    }

    /// Synchronize against a candidate barrier cohort or an active joined cohort.
    fn sync_p2p_peers(
        synchronization_peers: impl Iterator<Item = Entity>,
        local_now: TickInstant,
        is_synced: bool,
        sync: &mut LocalTimelineSync,
        config: &InputTimelineConfig,
        remotes: &Query<
            (&Remote, &PingManager),
            (With<Client>, With<Connected>, Without<HostClient>),
        >,
        commands: &mut Commands,
    ) {
        let mut found_any = false;
        let mut all_initialized = true;
        let mut sampled_any = false;
        let mut limiting = None;
        for link_entity in synchronization_peers {
            found_any = true;
            let Ok((remote, _ping_manager)) = remotes.get(link_entity) else {
                all_initialized = false;
                continue;
            };
            if !remote.is_initialized() {
                all_initialized = false;
                continue;
            }
            sampled_any |= remote.received_packet();
            let remote_estimate = remote.current_estimate();
            let lead = (local_now - remote_estimate).to_f32();
            if limiting.is_none_or(|(_, worst, _)| lead > worst) {
                limiting = Some((link_entity, lead, remote_estimate));
            }
        }
        all_initialized &= found_any;

        if !is_synced {
            if !all_initialized || !sampled_any {
                return;
            }
            let Some((limiting_link, worst_lead, remote_estimate)) = limiting else {
                return;
            };
            let objective = if worst_lead > 0.0 {
                remote_estimate
            } else {
                local_now
            };
            let tick_delta = sync.resync(local_now, objective);
            sync.set_synced(true);
            sync.set_relative_speed(1.0);
            commands.trigger(LocalTimelineShift { delta: tick_delta });
            trace!(
                target: "lightyear_debug::sync",
                kind = "p2p_initial_sync",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                ?limiting_link,
                worst_lead,
                ?objective,
                tick_delta,
                "initial P2P timeline aligned before input capture became ready"
            );
            return;
        }

        if !sampled_any {
            return;
        }
        let Some((limiting_link, worst_lead, _)) = limiting else {
            return;
        };
        let adjustment = sync.speed_adjustment(config, worst_lead.max(0.0));
        if matches!(adjustment, SyncAdjustment::Resync) {
            sync.set_relative_speed(1.0);
            commands.trigger(P2PTimelineDiverged {
                limiting_link,
                lead: worst_lead,
            });
            error!(
                ?limiting_link,
                worst_lead,
                "running P2P timeline diverged beyond bounded pacing; abort the session"
            );
            return;
        }
        trace!(
            target: "lightyear_debug::sync",
            kind = "p2p_sync_aggregate",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            ?limiting_link,
            worst_lead,
            relative_speed = sync.relative_speed(),
            "applied one aggregate P2P pacing decision"
        );
    }

    /// Select the topology's clock source and update [`LocalTimelineSync`].
    ///
    /// Client/server mode follows its sole authoritative server. P2P mode performs one initial
    /// alignment after every declared link is ready, then feeds the worst local lead into the
    /// shared speed controller without applying unsafe mid-session tick shifts. Deterministic
    /// gameplay start readiness belongs to the P2P session layer.
    fn sync_timelines(
        tick_duration: Res<TickDuration>,
        fixed_time: Res<Time<Fixed>>,
        metadata: Res<NetworkingMetadata>,
        config: Res<InputTimelineConfig>,
        local_timeline: Res<LocalTimeline>,
        mut sync: ResMut<LocalTimelineSync>,
        remotes: Query<
            (&Remote, &PingManager),
            (With<Client>, With<Connected>, Without<HostClient>),
        >,
        p2p_links: Query<(Entity, Ref<P2P>)>,
        mut commands: Commands,
    ) {
        let mut is_synced = sync.is_synced();
        let local_now = local_timeline.instant(&fixed_time);
        let has_candidates = p2p_links.iter().any(|(_, state)| *state == P2P::Candidate);
        let candidates_changed = p2p_links
            .iter()
            .any(|(_, state)| *state == P2P::Candidate && state.is_changed());

        if metadata.is_changed() || candidates_changed {
            let preserve_running_p2p =
                is_synced && matches!(&metadata.mode, NetworkTopology::P2P(_));
            sync.reset();
            sync.set_synced(preserve_running_p2p);
            is_synced = preserve_running_p2p;
        }

        if has_candidates {
            Self::sync_p2p_peers(
                p2p_links
                    .iter()
                    .filter_map(|(entity, state)| (*state == P2P::Candidate).then_some(entity)),
                local_now,
                is_synced,
                &mut sync,
                &config,
                &remotes,
                &mut commands,
            );
            return;
        }

        match &metadata.mode {
            NetworkTopology::Client(client) => {
                let Ok((remote, ping_manager)) = remotes.get(*client) else {
                    return;
                };
                Self::sync_client_server(
                    *client,
                    local_now,
                    &mut sync,
                    &config,
                    remote,
                    ping_manager,
                    &tick_duration,
                    &mut commands,
                );
            }
            NetworkTopology::P2P(synchronization_peers) => Self::sync_p2p_peers(
                synchronization_peers.iter().copied(),
                local_now,
                is_synced,
                &mut sync,
                &config,
                &remotes,
                &mut commands,
            ),
            NetworkTopology::HostClient { .. } if !is_synced => {
                sync.set_synced(true);
            }
            _ => {}
        }
    }

    /// Apply a whole-tick synchronization correction to the authoritative [`LocalTimeline`].
    ///
    /// Input, prediction, and prespawn observers receive the same event and shift their indexed
    /// histories consistently with the world clock.
    fn handle_local_timeline_shift(
        trigger: On<LocalTimelineShift>,
        mut local_timeline: ResMut<LocalTimeline>,
    ) {
        local_timeline.apply_delta(trigger.delta);
        trace!(
            target: "lightyear_debug::sync",
            kind = "local_timeline_shift_apply",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            timeline = "LocalTimeline",
            tick_delta = trigger.delta,
            local_tick = local_timeline.tick().0,
            "applied LocalTimelineShift to LocalTimeline"
        );
    }
}

impl<Remote: SyncTargetTimeline> Plugin for LocalTimelineSyncPlugin<Remote> {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalTimelineSync>();
        app.init_resource::<InputTimelineConfig>();
        app.init_resource::<PredictionWindowWait>();

        app.add_observer(Self::handle_connect);
        app.add_observer(Self::handle_host_client);
        app.add_observer(Self::handle_disconnect);
        app.add_observer(Self::handle_local_timeline_shift);
        app.add_systems(PostUpdate, Self::sync_timelines.in_set(SyncSystems::Sync));
        app.add_systems(Last, Self::update_virtual_time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::input::{InputTimelineConfig, LocalTimelineSync};
    use alloc::vec::Vec;
    use bevy_app::{FixedUpdate, PostUpdate};
    use bevy_time::TimeUpdateStrategy;
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_core::plugin::CorePlugins;
    use lightyear_core::tick::Tick;
    use lightyear_core::time::{Overstep, TickDelta};
    use lightyear_core::timeline::TimelineConfig;

    #[derive(Component, Default)]
    struct TestRemoteConfig;

    #[derive(Component, Default)]
    struct TestRemote {
        now: TickInstant,
        estimate: TickInstant,
        initialized: bool,
        received_packet: bool,
    }

    #[derive(Resource, Default)]
    struct RecordedDivergences(Vec<P2PTimelineDiverged>);

    fn record_divergence(
        trigger: On<P2PTimelineDiverged>,
        mut divergences: ResMut<RecordedDivergences>,
    ) {
        divergences.0.push(*trigger);
    }

    #[derive(Resource, Default)]
    struct FixedRuns(u32);

    #[test]
    fn prediction_window_wait_stops_bevy_fixed_schedules() {
        fn count_fixed_runs(mut runs: ResMut<FixedRuns>) {
            runs.0 += 1;
        }

        let tick_duration = Duration::from_millis(10);
        let mut app = App::new();
        app.add_plugins((
            CorePlugins { tick_duration },
            LocalTimelineSyncPlugin::<TestRemote>::default(),
        ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(tick_duration));
        app.init_resource::<NetworkingMetadata>();
        app.init_resource::<FixedRuns>();
        app.add_systems(FixedUpdate, count_fixed_runs);
        app.world_mut()
            .resource_mut::<LocalTimelineSync>()
            .set_synced(true);
        app.world_mut().resource_mut::<NetworkingMetadata>().mode =
            NetworkTopology::P2P([Entity::PLACEHOLDER].into_iter().collect());
        {
            let mut wait = app.world_mut().resource_mut::<PredictionWindowWait>();
            wait.update(Tick(0), None, 1);
            wait.update(Tick(1), None, 1);
        }

        // The signal is applied in Last, after this frame's fixed loop has already run.
        app.update();
        let before_wait = app.world().resource::<FixedRuns>().0;
        assert_eq!(
            app.world().resource::<Time<Virtual>>().relative_speed(),
            0.0
        );

        app.update();
        assert_eq!(app.world().resource::<FixedRuns>().0, before_wait);

        // Releasing the signal similarly takes effect for the following application frame.
        app.world_mut()
            .resource_mut::<PredictionWindowWait>()
            .update(Tick(1), Some(Tick(1)), 1);
        app.update();
        assert_eq!(app.world().resource::<FixedRuns>().0, before_wait);
        app.update();
        assert!(app.world().resource::<FixedRuns>().0 > before_wait);
    }

    #[test]
    fn prediction_window_wait_also_pauses_during_p2p_candidate_sync() {
        let mut app = App::new();
        app.add_plugins((
            CorePlugins {
                tick_duration: Duration::from_millis(10),
            },
            LocalTimelineSyncPlugin::<TestRemote>::default(),
        ));
        app.init_resource::<NetworkingMetadata>();
        app.world_mut().spawn(P2P::Candidate);
        app.world_mut()
            .resource_mut::<LocalTimelineSync>()
            .set_synced(true);
        {
            let mut wait = app.world_mut().resource_mut::<PredictionWindowWait>();
            wait.update(Tick(0), None, 1);
            wait.update(Tick(1), None, 1);
        }

        app.world_mut().run_schedule(Last);

        assert_eq!(
            app.world().resource::<Time<Virtual>>().relative_speed(),
            0.0
        );
    }

    impl TimelineConfig for TestRemoteConfig {
        type Context = ();
        type Timeline = TestRemote;
    }

    impl NetworkTimeline for TestRemote {
        type Config = TestRemoteConfig;

        fn now(&self) -> TickInstant {
            self.now
        }

        fn tick(&self) -> Tick {
            self.now.tick()
        }

        fn overstep(&self) -> Overstep {
            self.now.overstep()
        }

        fn set_now(&mut self, now: TickInstant) {
            self.now = now;
        }

        fn apply_delta(&mut self, delta: TickDelta) {
            self.now = self.now + delta;
        }
    }

    impl SyncTargetTimeline for TestRemote {
        fn current_estimate(&self) -> TickInstant {
            self.estimate
        }

        fn is_initialized(&self) -> bool {
            self.initialized
        }

        fn received_packet(&self) -> bool {
            self.received_packet
        }
    }

    #[test]
    fn resource_pipeline_initializes_then_paces_from_the_worst_p2p_lead() {
        let mut app = App::new();
        app.add_plugins((
            CorePlugins {
                tick_duration: Duration::from_millis(10),
            },
            LocalTimelineSyncPlugin::<TestRemote>::default(),
        ));
        app.init_resource::<NetworkingMetadata>();
        app.init_resource::<RecordedDivergences>();
        app.add_observer(record_divergence);
        app.update();

        app.insert_resource(InputTimelineConfig::default().with_sync_config(SyncConfig {
            handshake_pings: 0,
            error_margin: 2.0,
            max_error_margin: 10.0,
            consecutive_errors_threshold: 1,
            ..Default::default()
        }));
        app.world_mut()
            .resource_mut::<LocalTimeline>()
            .apply_delta(100);
        let local_now = TickInstant::from(app.world().resource::<LocalTimeline>().tick());

        let mut links = Vec::new();
        // The middle Link has the slowest observed execution phase. Declare the final Link without
        // connecting it first to verify that the first connection cannot start input capture while
        // a member of the fixed roster is still joining.
        for (peer, lead) in [(1, 1), (2, 4), (3, 0)] {
            let estimate = local_now - TickDelta::from_i32(lead);
            let entity = app
                .world_mut()
                .spawn((
                    P2P::Candidate,
                    RemoteId(PeerId::Local(peer)),
                    TestRemote {
                        now: estimate,
                        estimate,
                        initialized: peer != 3,
                        received_packet: true,
                    },
                    PingManager::default(),
                ))
                .id();
            if peer != 3 {
                app.world_mut().entity_mut(entity).insert(Connected);
            }
            links.push(entity);
        }
        app.world_mut().run_schedule(PostUpdate);

        assert_eq!(
            app.world().resource::<LocalTimeline>().tick(),
            local_now.tick()
        );
        assert_eq!(
            app.world().resource::<LocalTimelineSync>().relative_speed(),
            1.0
        );
        assert!(
            !app.world().resource::<LocalTimelineSync>().is_synced(),
            "input capture must remain gated until every declared P2P Link is connected"
        );

        // Connecting the final Link is still not enough until its remote timeline has initialized.
        app.world_mut().entity_mut(links[2]).insert(Connected);
        app.world_mut().run_schedule(PostUpdate);
        assert!(!app.world().resource::<LocalTimelineSync>().is_synced());

        // Once every current Link has initialized, align the local timeline to the slowest
        // observed phase and only then expose synchronized input scheduling.
        app.world_mut()
            .entity_mut(links[2])
            .get_mut::<TestRemote>()
            .unwrap()
            .initialized = true;
        app.world_mut().run_schedule(PostUpdate);

        let initial_objective = local_now - TickDelta::from_i32(4);
        assert_eq!(
            app.world().resource::<LocalTimeline>().tick(),
            initial_objective.tick()
        );
        assert_eq!(
            app.world().resource::<LocalTimelineSync>().relative_speed(),
            1.0
        );
        assert!(
            app.world().resource::<LocalTimelineSync>().is_synced(),
            "readiness is exposed only after initial P2P alignment"
        );

        // The session barrier can now complete. Gameplay-facing systems, including the
        // prediction-window wait, only become active once the topology transitions out of its
        // starting phase.
        for &link in &links {
            app.world_mut().entity_mut(link).insert(P2P::Joined);
        }
        app.world_mut().resource_mut::<NetworkingMetadata>().mode =
            NetworkTopology::P2P(links.iter().copied().collect());

        // After readiness, only the middle Link exceeds the controller deadband. A correct
        // maximum aggregate must therefore slow the app, independent of the controller's exact
        // speed formula.
        for (&link, lead) in links.iter().zip([1, 4, 0]) {
            let mut link = app.world_mut().entity_mut(link);
            let mut remote = link.get_mut::<TestRemote>().unwrap();
            remote.estimate = initial_objective - TickDelta::from_i32(lead);
            remote.received_packet = true;
        }
        app.world_mut().run_schedule(PostUpdate);

        let slowed_speed = app.world().resource::<LocalTimelineSync>().relative_speed();
        assert!(
            slowed_speed < 1.0,
            "the maximum four-tick lead must cross the two-tick deadband"
        );
        assert_eq!(
            app.world().resource::<LocalTimeline>().tick(),
            initial_objective.tick()
        );
        assert!(app.world().resource::<LocalTimelineSync>().is_synced());

        // Prediction-window exhaustion is a hard wait layered over the phase controller. It
        // pauses virtual time without overwriting the slowdown that should be restored on resume.
        {
            let mut wait = app.world_mut().resource_mut::<PredictionWindowWait>();
            wait.update(Tick(0), None, 4);
            wait.update(Tick(4), None, 4);
        }
        app.world_mut().run_schedule(Last);
        assert_eq!(
            app.world().resource::<Time<Virtual>>().relative_speed(),
            0.0
        );
        assert_eq!(
            app.world().resource::<LocalTimelineSync>().relative_speed(),
            slowed_speed,
            "hard waiting must preserve the phase controller's desired speed"
        );

        {
            let mut wait = app.world_mut().resource_mut::<PredictionWindowWait>();
            wait.update(Tick(4), Some(Tick(2)), 4);
        }
        app.world_mut().run_schedule(Last);
        assert_eq!(
            app.world().resource::<Time<Virtual>>().relative_speed(),
            slowed_speed,
            "recovering two confirmed ticks must resume at the phase-controller speed"
        );

        // Render frames without a new network observation must preserve both the controller state
        // and its current correction. Recovery is driven by later fresh observations.
        {
            let world = app.world_mut();
            let mut query = world.query::<&mut TestRemote>();
            query
                .iter_mut(world)
                .for_each(|mut remote| remote.received_packet = false);
        }
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(
            app.world().resource::<LocalTimelineSync>().relative_speed(),
            slowed_speed,
            "a frame without a fresh observation must not alter P2P pacing"
        );

        // Exclude the four-tick Link and provide a fresh observation on the remaining Links. Their
        // maximum lead is one tick, inside the deadband, so the controller begins recovering.
        app.world_mut()
            .entity_mut(links[1])
            .get_mut::<TestRemote>()
            .unwrap()
            .initialized = false;
        {
            let world = app.world_mut();
            let mut query = world.query::<&mut TestRemote>();
            query
                .iter_mut(world)
                .for_each(|mut remote| remote.received_packet = true);
        }
        app.world_mut().run_schedule(PostUpdate);

        let local_sync = app.world().resource::<LocalTimelineSync>();
        let recovering_speed = local_sync.relative_speed();
        assert!(
            recovering_speed > slowed_speed && recovering_speed <= 1.0,
            "excluding the only lead outside the deadband must start speed recovery"
        );
        assert!(app.world().resource::<LocalTimelineSync>().is_synced());

        // Losing every usable estimate without a topology change is still not a new observation;
        // keep the last correction until timing resumes or the topology itself changes.
        {
            let world = app.world_mut();
            let mut query = world.query::<&mut TestRemote>();
            query.iter_mut(world).for_each(|mut remote| {
                remote.initialized = false;
                remote.received_packet = false;
            });
        }
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(
            app.world().resource::<LocalTimelineSync>().relative_speed(),
            recovering_speed,
            "missing observations must preserve controller hysteresis"
        );
        assert!(app.world().resource::<LocalTimelineSync>().is_synced());

        // Once input capture is active, a resync-sized gap is unsafe to apply locally. Report the
        // limiting Link to the session owner and leave all tick labels unchanged.
        {
            let mut link = app.world_mut().entity_mut(links[0]);
            let mut remote = link.get_mut::<TestRemote>().unwrap();
            remote.initialized = true;
            remote.received_packet = true;
            remote.estimate = initial_objective - TickDelta::from_i32(20);
        }
        app.world_mut().run_schedule(PostUpdate);
        assert_eq!(
            app.world().resource::<LocalTimelineSync>().relative_speed(),
            1.0
        );
        assert_eq!(
            app.world().resource::<LocalTimeline>().tick(),
            initial_objective.tick(),
            "a running P2P session must not relabel local ticks"
        );
        assert!(app.world().resource::<LocalTimelineSync>().is_synced());
        let divergences = &app.world().resource::<RecordedDivergences>().0;
        assert_eq!(divergences.len(), 1);
        assert_eq!(divergences[0].limiting_link, links[0]);
        assert_eq!(divergences[0].lead, 20.0);

        // Once every joined Link disconnects, the cached topology leaves P2P mode and timeline
        // readiness is revoked. Application time itself returns to normal speed.
        for &link in &links {
            app.world_mut().entity_mut(link).remove::<Connected>();
        }
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::Undefined;
        app.world_mut().run_schedule(PostUpdate);

        let local_sync = app.world().resource::<LocalTimelineSync>();
        assert_eq!(local_sync.relative_speed(), 1.0);
        assert!(!local_sync.is_synced());

        app.world_mut().run_schedule(Last);
        assert_eq!(
            app.world().resource::<Time<Virtual>>().relative_speed(),
            1.0,
            "leaving P2P topology must restore normal application time"
        );
    }
}
