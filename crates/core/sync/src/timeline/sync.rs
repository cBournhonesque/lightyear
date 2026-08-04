use crate::ping::manager::PingManager;
use crate::plugin::SyncSystems;
use crate::timeline::input::PredictionWindowWait;
use bevy_app::{App, Last, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::resource::IsResource;
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time, Virtual};
use bevy_utils::prelude::DebugName;
use core::time::Duration;
use lightyear_connection::client::{Client, Connected, Disconnected};
use lightyear_connection::host::HostClient;
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
use lightyear_connection::p2p::P2P;
use lightyear_core::prelude::{LocalTimeline, NetworkTimelinePlugin};
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{Overstep, TickInstant};
use lightyear_core::timeline::{NetworkTimeline, SyncEvent};
#[allow(unused_imports)]
use tracing::{debug, error, info, trace};

/// Marker component to indicate that the timeline has been synced
#[derive(Component, Debug)]
pub struct IsSynced<T> {
    pub(crate) marker: core::marker::PhantomData<T>,
}

impl<T> Default for IsSynced<T> {
    fn default() -> Self {
        IsSynced {
            marker: core::marker::PhantomData,
        }
    }
}

/// Triggered when a running P2P timeline is too far ahead to correct with bounded pacing.
///
/// A running deterministic peer cannot safely emit a local [`SyncEvent`], because other peers
/// would retain inputs under the old tick labels. The P2P session owner should treat this event as
/// fatal and abort the session, unless it implements a coordinated all-peer resynchronization
/// protocol.
#[derive(EntityEvent, Debug, Clone, Copy)]
pub struct P2PTimelineDiverged {
    /// Resource entity holding the application-global driving timeline.
    pub entity: Entity,
    /// P2P Link whose remote estimate produced the worst local lead.
    pub limiting_link: Entity,
    /// Local lead over the limiting Link, measured in fractional ticks.
    pub lead: f32,
}

/// Timeline that is synced to another timeline
pub trait SyncedTimeline: NetworkTimeline {
    /// Get the ideal [`TickInstant`] that this timeline should be at
    fn sync_objective<Remote: SyncTargetTimeline>(
        &self,
        other: &Remote,
        config: &Self::Config,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> TickInstant;

    /// Resync the timeline if they are too out of sync. Returns the number of tick deltas
    /// that should be applied
    fn resync(&mut self, sync_objective: TickInstant) -> i32;

    /// Sync the current timeline to the other timeline T.
    /// Usually this is achieved by slightly speeding up or slowing down the current timeline.
    /// If there is a big discrepancy we can do a `resync` instead.
    ///
    /// Returns the number of delta ticks that should be applied
    // TODO: should we use LinkStats instead of PingManager? and PingManager is a way to update the LinkStats?
    fn sync<Remote: SyncTargetTimeline>(
        &mut self,
        main: &Remote,
        config: &Self::Config,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> Option<i32>;

    /// Apply the common speed controller for an error measured in fractional ticks.
    ///
    /// Implementations apply speed changes and recovery toward `1.0`. A
    /// [`SyncAdjustment::Resync`] result is left to the synchronization policy: conventional
    /// client/server synchronization may snap, while a running P2P session must abort or perform
    /// a coordinated recovery instead.
    fn speed_adjustment(&mut self, config: &Self::Config, offset: f32) -> SyncAdjustment;

    fn is_synced(&self) -> bool;

    /// Returns the speed of your timeline relative to your system clock as an `f32`.
    /// A value of `1.0` means the timeline is running at normal speed.
    /// A value of `0.5` means the timeline is running at half speed,
    fn relative_speed(&self) -> f32;

    fn set_relative_speed(&mut self, ratio: f32);

    /// Reset the timeline to its initial state (used when a client reconnects)
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

/// Configuration for the sync manager, which is in charge of syncing the client's tick/time with the server's tick/time
///
/// The sync manager runs only on the client and maintains two different times:
/// - the prediction tick/time: this is the client time, which runs roughly RTT/2 ahead of the server time, so that input packets
///   for tick T sent from the client arrive on the server at tick T
/// - the interpolation tick/time: this is the interpolation timeline, which runs behind the server time so that interpolation
///   always has at least one packet to interpolate towards
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
    /// If the error margin is too big, we snap the prediction/interpolation time to the objective value
    pub max_error_margin: f32,
    /// How many consecutive errors have we seen that are in the same direction
    pub consecutive_errors: u8,
    /// Sign of the previous error
    pub previous_error_sign: bool,
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
            consecutive_errors: 0,
            previous_error_sign: true,
            consecutive_errors_threshold: 3,
            speedup_factor: 1.05,
        }
    }
}

#[derive(Debug, Reflect)]
pub struct SyncContext {
    /// How many consecutive errors have we seen that are in the same direction
    pub consecutive_errors: u8,
    /// Sign of the previous error
    pub previous_error_sign: bool,
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
    pub fn speed_adjustment(&mut self, config: &SyncConfig, offset: f32) -> SyncAdjustment {
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

/// Plugin to synchronize one timeline with a remote timeline.
///
/// `DRIVING` indicates whether the synchronized timeline drives [`Time<Virtual>`] and
/// [`LocalTimeline`]. `RESOURCE` selects whether the synchronized timeline and its configuration
/// are application-global resources or components on each remote link.
pub struct SyncedTimelinePlugin<
    Synced,
    Remote,
    const DRIVING: bool = false,
    const RESOURCE: bool = false,
> {
    pub(crate) _marker: core::marker::PhantomData<(Synced, Remote)>,
}

impl<Synced: SyncedTimeline, Remote: SyncTargetTimeline, const DRIVING: bool, const RESOURCE: bool>
    SyncedTimelinePlugin<Synced, Remote, DRIVING, RESOURCE>
{
    /// Reset a timeline when its session starts and align a driving timeline with local time.
    fn reset_timeline(timeline: &mut Synced, local_timeline: &LocalTimeline) {
        timeline.reset();
        if DRIVING {
            trace!("Set Driving timeline tick to LocalTimeline");
            let delta = local_timeline.tick() - timeline.tick();
            timeline.apply_delta(delta.into());
        }
    }

    /// Copy the application's current fixed-update phase into a driving timeline.
    fn sync_from_local(
        synced: &mut Synced,
        local_timeline: &LocalTimeline,
        fixed_time: &Time<Fixed>,
    ) {
        let local_tick = local_timeline.tick();
        let overstep = fixed_time.overstep_fraction();
        synced.set_now(TickInstant::from_tick_and_overstep(
            local_tick,
            Overstep::from_f32(overstep),
        ));
        trace!(
            target: "lightyear_debug::timeline",
            kind = "sync_from_local_timeline",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            timeline = ?DebugName::type_name::<Synced>(),
            local_tick = local_tick.0,
            timeline_tick = synced.tick().0,
            overstep,
            "driving timeline synced from LocalTimeline"
        );
    }

    /// Apply a synchronized timeline's speed correction to Bevy virtual time.
    fn apply_relative_speed(timeline: &Synced, virtual_time: &mut Time<Virtual>) {
        trace!(
            "Timeline {} sets the virtual time relative speed to {}",
            DebugName::type_name::<Synced>(),
            timeline.relative_speed()
        );
        trace!(
            target: "lightyear_debug::sync",
            kind = "relative_speed",
            schedule = "Last",
            sample_point = "Last",
            timeline = ?DebugName::type_name::<Synced>(),
            tick = timeline.tick().0,
            relative_speed = timeline.relative_speed(),
            "timeline relative speed applied to Virtual time"
        );
        // TODO: be able to apply the speed_ratio on top of any speed ratio already applied by the user.
        virtual_time.set_relative_speed(timeline.relative_speed());
    }

    /// Reset controller history without changing the driving timeline's current phase.
    fn reset_controller(sync_timeline: &mut Synced, relative_speed: f32) {
        let now = sync_timeline.now();
        sync_timeline.reset();
        // `sync_from_local_timeline` already copied the current application phase this frame.
        // Preserve it while clearing controller state so diagnostics never observe a spurious zero.
        sync_timeline.set_now(now);
        sync_timeline.set_relative_speed(relative_speed);
    }

    /// Synchronize one timeline and emit a [`SyncEvent`] if it snaps by whole ticks.
    fn sync_timeline(
        entity: Entity,
        sync_timeline: &mut Synced,
        config: &Synced::Config,
        main_timeline: &Remote,
        ping_manager: &PingManager,
        has_is_synced: bool,
        tick_duration: &TickDuration,
        commands: &mut Commands,
    ) {
        trace!(
            ?entity,
            ?has_is_synced,
            "In SyncTimelines from {:?} to {:?}",
            DebugName::type_name::<Synced>(),
            DebugName::type_name::<Remote>()
        );
        // return early if the remote timeline hasn't received any packets
        if !main_timeline.received_packet() {
            trace!(
                target: "lightyear_debug::sync",
                kind = "sync_skipped_no_packet",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                entity = ?entity,
                timeline = ?DebugName::type_name::<Synced>(),
                remote_timeline = ?DebugName::type_name::<Remote>(),
                timeline_tick = sync_timeline.tick().0,
                "sync skipped because remote timeline received no packet"
            );
            return;
        }
        if !has_is_synced && sync_timeline.is_synced() {
            debug!(
                "Timeline {:?} is synced to {:?}",
                DebugName::type_name::<Synced>(),
                DebugName::type_name::<Remote>()
            );
            commands
                .entity(entity)
                .insert(IsSynced::<Synced>::default());
            trace!(
                target: "lightyear_debug::sync",
                kind = "timeline_synced",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                entity = ?entity,
                timeline = ?DebugName::type_name::<Synced>(),
                remote_timeline = ?DebugName::type_name::<Remote>(),
                timeline_tick = sync_timeline.tick().0,
                "timeline marked synced"
            );
        }
        let before_now = sync_timeline.now();
        let remote_estimate = main_timeline.current_estimate();
        if let Some(tick_delta) =
            sync_timeline.sync(main_timeline, config, ping_manager, tick_duration.0)
        {
            trace!(
                target: "lightyear_debug::sync",
                kind = "sync_adjustment",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                entity = ?entity,
                timeline = ?DebugName::type_name::<Synced>(),
                remote_timeline = ?DebugName::type_name::<Remote>(),
                timeline_tick = sync_timeline.tick().0,
                remote_tick = main_timeline.tick().0,
                before = ?before_now,
                after = ?sync_timeline.now(),
                remote_estimate = ?remote_estimate,
                tick_delta,
                relative_speed = sync_timeline.relative_speed(),
                rtt_ms = ping_manager.rtt().as_secs_f64() * 1000.0,
                jitter_ms = ping_manager.jitter().as_secs_f64() * 1000.0,
                "timeline sync emitted SyncEvent"
            );
            // if it's the driving pipeline, also update the LocalTimeline in `handle_sync_event`
            commands.trigger(SyncEvent::<Synced::Config>::new(entity, tick_delta));
        } else {
            trace!(
                target: "lightyear_debug::sync",
                kind = "sync_sample",
                schedule = "PostUpdate",
                sample_point = "PostUpdate",
                entity = ?entity,
                timeline = ?DebugName::type_name::<Synced>(),
                remote_timeline = ?DebugName::type_name::<Remote>(),
                timeline_tick = sync_timeline.tick().0,
                remote_tick = main_timeline.tick().0,
                before = ?before_now,
                after = ?sync_timeline.now(),
                remote_estimate = ?remote_estimate,
                relative_speed = sync_timeline.relative_speed(),
                rtt_ms = ping_manager.rtt().as_secs_f64() * 1000.0,
                jitter_ms = ping_manager.jitter().as_secs_f64() * 1000.0,
                "timeline sync sampled"
            );
        }
    }

    /// Apply a driving timeline's whole-tick correction to [`LocalTimeline`].
    fn handle_sync_event(
        trigger: On<SyncEvent<Synced::Config>>,
        mut local_timeline: ResMut<LocalTimeline>,
    ) {
        local_timeline.apply_delta(trigger.tick_delta);
        let new_tick = local_timeline.tick();
        debug!(
            tick_delta = ?trigger.tick_delta,
            ?new_tick,
            "Apply delta to LocalTimeline from driving pipeline {:?}'s SyncEvent", DebugName::type_name::<Synced>()
        );
        trace!(
            target: "lightyear_debug::sync",
            kind = "sync_event_apply",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            entity = ?trigger.entity,
            timeline = ?DebugName::type_name::<Synced>(),
            tick_delta = trigger.tick_delta,
            local_tick = new_tick.0,
            "applied SyncEvent to LocalTimeline"
        );
    }
}

impl<Synced: SyncedTimeline, Remote: SyncTargetTimeline, const DRIVING: bool>
    SyncedTimelinePlugin<Synced, Remote, DRIVING, false>
{
    /// Reset a link-held synchronized timeline when that link connects.
    fn handle_connect(
        trigger: On<Add, Connected>,
        local_timeline: Res<LocalTimeline>,
        mut query: Query<&mut Synced>,
    ) {
        if let Ok(mut timeline) = query.get_mut(trigger.entity) {
            Self::reset_timeline(&mut timeline, &local_timeline);
        }
    }

    /// Mark a host-client's link-held timeline as synchronized without a handshake.
    fn handle_host_client(trigger: On<Add, HostClient>, mut commands: Commands) {
        commands
            .entity(trigger.entity)
            .insert(IsSynced::<Synced>::default());
    }

    /// Remove the synchronization marker from a disconnected link.
    fn handle_disconnect(trigger: On<Add, Disconnected>, mut commands: Commands) {
        commands.entity(trigger.entity).remove::<IsSynced<Synced>>();
    }

    /// Copy local fixed-update phase into every link-held driving timeline.
    fn sync_from_local_timeline(
        local_timeline: Res<LocalTimeline>,
        fixed_time: Res<Time<Fixed>>,
        mut query: Query<&mut Synced>,
    ) {
        query.iter_mut().for_each(|mut synced| {
            Self::sync_from_local(&mut synced, &local_timeline, &fixed_time)
        });
    }

    /// Apply the single connected, synchronized link timeline's relative speed.
    fn update_virtual_time(
        mut virtual_time: ResMut<Time<Virtual>>,
        query: Query<&Synced, (With<IsSynced<Synced>>, With<Connected>, Without<HostClient>)>,
    ) {
        if let Ok(timeline) = query.single() {
            Self::apply_relative_speed(timeline, &mut virtual_time);
        }
    }

    /// Synchronize each link-held timeline with the remote timeline on the same link.
    fn sync_timelines(
        tick_duration: Res<TickDuration>,
        mut commands: Commands,
        mut query: Query<
            (
                Entity,
                &mut Synced,
                &Synced::Config,
                &Remote,
                &PingManager,
                Has<IsSynced<Synced>>,
            ),
            (With<Connected>, Without<HostClient>),
        >,
    ) {
        query.iter_mut().for_each(
            |(entity, mut synced, config, remote, ping_manager, is_synced)| {
                Self::sync_timeline(
                    entity,
                    &mut synced,
                    config,
                    remote,
                    ping_manager,
                    is_synced,
                    &tick_duration,
                    &mut commands,
                );
            },
        );
    }
}

impl<Synced: SyncedTimeline, Remote: SyncTargetTimeline, const DRIVING: bool>
    SyncedTimelinePlugin<Synced, Remote, DRIVING, true>
where
    Synced::Config: Resource,
{
    /// Reset the global synchronized timeline when its client link connects.
    fn handle_connect(
        trigger: On<Add, Connected>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        local_timeline: Res<LocalTimeline>,
        mut timeline: Single<&mut Synced, With<IsResource>>,
    ) {
        if clients.get(trigger.entity).is_ok() {
            Self::reset_timeline(&mut timeline, &local_timeline);
        }
    }

    /// Mark the resource entity as synchronized for a host-client session.
    fn handle_host_client(
        trigger: On<Add, HostClient>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        timeline_entity: Single<Entity, (With<Synced>, With<IsResource>)>,
        mut commands: Commands,
    ) {
        if clients.get(trigger.entity).is_ok() {
            commands
                .entity(*timeline_entity)
                .insert(IsSynced::<Synced>::default());
        }
    }

    /// Remove the synchronization marker from the resource entity when its client disconnects.
    fn handle_disconnect(
        trigger: On<Add, Disconnected>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        timeline_entity: Single<Entity, (With<Synced>, With<IsResource>)>,
        mut commands: Commands,
    ) {
        if clients.get(trigger.entity).is_ok() {
            commands
                .entity(*timeline_entity)
                .remove::<IsSynced<Synced>>();
        }
    }

    /// Copy local fixed-update phase into the global driving timeline.
    fn sync_from_local_timeline(
        local_timeline: Res<LocalTimeline>,
        fixed_time: Res<Time<Fixed>>,
        mut timeline: Single<&mut Synced, With<IsResource>>,
    ) {
        Self::sync_from_local(&mut timeline, &local_timeline, &fixed_time);
    }

    /// Apply the resource timeline's relative speed to virtual time.
    ///
    /// P2P always owns virtual time, but runs at normal speed when no usable phase estimate exists.
    /// Other topologies run at normal speed until their resource timeline is synchronized.
    fn update_virtual_time(
        metadata: Res<NetworkingMetadata>,
        prediction_window_wait: Res<PredictionWindowWait>,
        timeline: Single<(&Synced, Has<IsSynced<Synced>>), With<IsResource>>,
        mut virtual_time: ResMut<Time<Virtual>>,
    ) {
        let (timeline, is_synced) = timeline.into_inner();
        match metadata.mode {
            NetworkTopology::Client(_) | NetworkTopology::P2P { .. }
                if is_synced && prediction_window_wait.is_waiting() =>
            {
                // Keep the phase controller's desired speed on the timeline while preventing the
                // next FixedMain run. Network receive and timeline observation continue in the
                // variable schedules, so confirmed input can advance and release the wait.
                virtual_time.set_relative_speed(0.0);
                trace!(
                    target: "lightyear_debug::sync",
                    kind = "prediction_window_wait",
                    schedule = "Last",
                    sample_point = "Last",
                    prediction_depth = prediction_window_wait.prediction_depth(),
                    maximum_predicted_ticks = prediction_window_wait.maximum_predicted_ticks(),
                    confirmed_tick = ?prediction_window_wait.confirmed_tick(),
                    desired_relative_speed = timeline.relative_speed(),
                    "paused virtual time at the deterministic prediction-window limit"
                );
            }
            NetworkTopology::P2P { .. } => {
                Self::apply_relative_speed(timeline, &mut virtual_time);
            }
            NetworkTopology::Client(_) | NetworkTopology::HostClient { .. } if is_synced => {
                Self::apply_relative_speed(timeline, &mut virtual_time);
            }
            _ => virtual_time.set_relative_speed(1.0),
        }
    }

    /// Synchronize the resource timeline from the objective selected by the cached topology.
    ///
    /// Conventional client/server mode uses one remote Link and may snap by whole ticks. P2P mode
    /// reads the already-smoothed estimate on every currently connected P2P Link, selects the
    /// largest local lead, and feeds that aggregate error into the common speed controller once.
    /// Fixed-roster agreement and gameplay start readiness belong to the P2P session layer.
    fn sync_timelines(
        tick_duration: Res<TickDuration>,
        metadata: Res<NetworkingMetadata>,
        config: Res<Synced::Config>,
        timeline: Single<(Entity, &mut Synced, Has<IsSynced<Synced>>), With<IsResource>>,
        remotes: Query<
            (&Remote, &PingManager),
            (With<Client>, With<Connected>, Without<HostClient>),
        >,
        mut commands: Commands,
    ) {
        let (entity, mut synced, mut is_synced) = timeline.into_inner();

        // NetworkingMetadata only reports changed when its public topology changes. Resetting here
        // prevents controller history from leaking between Link sets. A running P2P timeline keeps
        // its readiness marker: an existing deterministic session must never locally relabel ticks
        // merely because a peer connected or disconnected. A fresh app remains unready until the
        // initial P2P alignment below completes.
        if metadata.is_changed() {
            Self::reset_controller(&mut synced, 1.0);
            let preserve_running_p2p =
                is_synced && matches!(&metadata.mode, NetworkTopology::P2P { .. });
            if is_synced && !preserve_running_p2p {
                commands.entity(entity).remove::<IsSynced<Synced>>();
            }
            is_synced = preserve_running_p2p;
        }

        match &metadata.mode {
            NetworkTopology::Client(client) => {
                let Ok((remote, ping_manager)) = remotes.get(*client) else {
                    return;
                };
                Self::sync_timeline(
                    entity,
                    &mut synced,
                    &config,
                    remote,
                    ping_manager,
                    is_synced,
                    &tick_duration,
                    &mut commands,
                );
            }
            NetworkTopology::P2P {
                connected,
                declared_links,
            } if DRIVING => {
                // P2P Links can be declared before their connection handshake completes. Waiting
                // for all declared Links lets a lobby establish its fixed roster up front without
                // allowing the first connection to start input capture prematurely.
                let all_declared_links_connected =
                    is_synced || connected.len() == usize::from(*declared_links);
                let mut all_initialized = !connected.is_empty() && all_declared_links_connected;
                let mut sampled_any = false;
                let mut limiting = None;
                for &link_entity in connected {
                    let Ok((remote, _ping_manager)) = remotes.get(link_entity) else {
                        all_initialized = false;
                        trace!(
                            target: "lightyear_debug::sync",
                            kind = "p2p_sync_missing_link_state",
                            schedule = "PostUpdate",
                            sample_point = "PostUpdate",
                            ?link_entity,
                            "ignoring P2P Link without sync state"
                        );
                        continue;
                    };
                    if !remote.is_initialized() {
                        all_initialized = false;
                        continue;
                    }
                    sampled_any |= remote.received_packet();
                    // Unlike client/server, P2P has no single asymmetric `sync_objective`: each
                    // initialized remote estimate is an execution-phase target. Taking the maximum
                    // of `local - remote` finds the Link furthest behind this app (the worst local
                    // lead), matching GGRS's maximum frame-advantage policy. Only that aggregate is
                    // passed to the shared controller, so the app slows toward its slowest peer.
                    let remote_estimate = remote.current_estimate();
                    let lead = (synced.now() - remote_estimate).to_f32();
                    if limiting.is_none_or(|(_, worst, _)| lead > worst) {
                        limiting = Some((link_entity, lead, remote_estimate));
                    }
                }

                // Before the timeline becomes ready, no input-capture system using
                // `SyncedInputTimeline` can run. It is therefore safe to align the local tick labels
                // once. Including the local phase in the minimum makes every starting peer converge
                // toward the slowest observed execution phase instead of swapping clocks.
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
                        synced.now()
                    };
                    let tick_delta = synced.resync(objective);
                    synced.set_relative_speed(1.0);
                    commands.trigger(SyncEvent::<Synced::Config>::new(entity, tick_delta));
                    commands
                        .entity(entity)
                        .insert(IsSynced::<Synced>::default());
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

                // Match conventional synchronization: controller hysteresis advances on network
                // observations, not once per render frame using the same estimates.
                if !sampled_any {
                    return;
                }
                let Some((limiting_link, worst_lead, _)) = limiting else {
                    return;
                };
                let adjustment = synced.speed_adjustment(&config, worst_lead.max(0.0));
                if matches!(adjustment, SyncAdjustment::Resync) {
                    synced.set_relative_speed(1.0);
                    commands.trigger(P2PTimelineDiverged {
                        entity,
                        limiting_link,
                        lead: worst_lead,
                    });
                    error!(
                        ?limiting_link,
                        worst_lead,
                        "running P2P timeline diverged beyond bounded pacing; abort the session"
                    );
                    trace!(
                        target: "lightyear_debug::sync",
                        kind = "p2p_sync_phase_gap",
                        schedule = "PostUpdate",
                        sample_point = "PostUpdate",
                        ?limiting_link,
                        worst_lead,
                        "running P2P phase gap requires session abort or coordinated recovery"
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
                    relative_speed = synced.relative_speed(),
                    "applied one aggregate P2P pacing decision"
                );
            }
            NetworkTopology::HostClient { .. } if !is_synced => {
                commands
                    .entity(entity)
                    .insert(IsSynced::<Synced>::default());
            }
            _ => {}
        }
    }
}

impl<Synced, Remote, const DRIVING: bool, const RESOURCE: bool> Default
    for SyncedTimelinePlugin<Synced, Remote, DRIVING, RESOURCE>
{
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<Synced: SyncedTimeline, Remote: SyncTargetTimeline, const DRIVING: bool> Plugin
    for SyncedTimelinePlugin<Synced, Remote, DRIVING, false>
{
    fn build(&self, app: &mut App) {
        app.add_plugins(NetworkTimelinePlugin::<Synced>::default());

        app.register_required_components::<Synced, PingManager>();
        app.register_required_components::<Synced, Remote>();
        if DRIVING {
            app.init_resource::<PredictionWindowWait>();
        }
        app.add_observer(Self::handle_connect);
        app.add_observer(Self::handle_host_client);
        app.add_observer(Self::handle_disconnect);
        // NOTE: we don't have to run this in PostUpdate, we could run this right after RunFixedMainLoop?
        app.add_systems(PostUpdate, Self::sync_timelines.in_set(SyncSystems::Sync));
        if DRIVING {
            app.add_systems(
                PostUpdate,
                Self::sync_from_local_timeline
                    .in_set(SyncSystems::Sync)
                    .before(Self::sync_timelines),
            );
            app.add_systems(Last, Self::update_virtual_time);
            app.add_observer(Self::handle_sync_event);
        }
    }
}

impl<Synced: SyncedTimeline + Resource + Default, Remote: SyncTargetTimeline, const DRIVING: bool>
    Plugin for SyncedTimelinePlugin<Synced, Remote, DRIVING, true>
where
    Synced::Config: Resource + Default,
{
    fn build(&self, app: &mut App) {
        app.add_plugins(NetworkTimelinePlugin::<Synced>::default());
        app.init_resource::<Synced>();
        app.init_resource::<Synced::Config>();
        if DRIVING {
            app.init_resource::<PredictionWindowWait>();
        }

        app.add_observer(Self::handle_connect);
        app.add_observer(Self::handle_host_client);
        app.add_observer(Self::handle_disconnect);
        app.add_systems(PostUpdate, Self::sync_timelines.in_set(SyncSystems::Sync));
        if DRIVING {
            app.add_systems(
                PostUpdate,
                Self::sync_from_local_timeline
                    .in_set(SyncSystems::Sync)
                    .before(Self::sync_timelines),
            );
            app.add_systems(Last, Self::update_virtual_time);
            app.add_observer(Self::handle_sync_event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeline::input::{InputTimeline, InputTimelineConfig};
    use alloc::vec::Vec;
    use bevy_app::{FixedUpdate, PostUpdate};
    use bevy_time::TimeUpdateStrategy;
    use lightyear_core::id::{PeerId, RemoteId};
    use lightyear_core::plugin::CorePlugins;
    use lightyear_core::tick::Tick;
    use lightyear_core::time::TickDelta;
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
            SyncedTimelinePlugin::<InputTimeline, TestRemote, true, true>::default(),
        ));
        app.insert_resource(TimeUpdateStrategy::ManualDuration(tick_duration));
        app.init_resource::<NetworkingMetadata>();
        app.init_resource::<FixedRuns>();
        app.add_systems(FixedUpdate, count_fixed_runs);
        let timeline_id = app.world().component_id::<InputTimeline>().unwrap();
        let timeline_entity = app.world().resource_entities().get(timeline_id).unwrap();
        app.world_mut()
            .entity_mut(timeline_entity)
            .insert(IsSynced::<InputTimeline>::default());
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::P2P {
            connected: Default::default(),
            declared_links: 1,
        };
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
            SyncedTimelinePlugin::<InputTimeline, TestRemote, true, true>::default(),
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
                    P2P,
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
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::P2P {
            connected: links[..2].iter().copied().collect(),
            declared_links: 3,
        };

        app.world_mut().run_schedule(PostUpdate);

        let timeline_id = app.world().component_id::<InputTimeline>().unwrap();
        let timeline_entity = app.world().resource_entities().get(timeline_id).unwrap();
        assert_eq!(
            app.world().resource::<LocalTimeline>().tick(),
            local_now.tick()
        );
        assert_eq!(app.world().resource::<InputTimeline>().now(), local_now);
        assert_eq!(
            app.world().resource::<InputTimeline>().relative_speed(),
            1.0
        );
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_none(),
            "input capture must remain gated until every declared P2P Link is connected"
        );

        // Connecting the final Link is still not enough until its remote timeline has initialized.
        app.world_mut().entity_mut(links[2]).insert(Connected);
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::P2P {
            connected: links.iter().copied().collect(),
            declared_links: 3,
        };
        app.world_mut().run_schedule(PostUpdate);
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_none()
        );

        // Once every current Link has initialized, align both driving timelines to the slowest
        // observed phase and only then expose the synchronized input timeline.
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
            app.world().resource::<InputTimeline>().now(),
            initial_objective
        );
        assert_eq!(
            app.world().resource::<InputTimeline>().relative_speed(),
            1.0
        );
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_some(),
            "the normal readiness marker is inserted only after initial P2P alignment"
        );

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

        let slowed_speed = app.world().resource::<InputTimeline>().relative_speed();
        assert!(
            slowed_speed < 1.0,
            "the maximum four-tick lead must cross the two-tick deadband"
        );
        assert_eq!(
            app.world().resource::<LocalTimeline>().tick(),
            initial_objective.tick()
        );
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_some()
        );

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
            app.world().resource::<InputTimeline>().relative_speed(),
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
            app.world().resource::<InputTimeline>().relative_speed(),
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

        let timeline = app.world().resource::<InputTimeline>();
        assert_eq!(timeline.now(), initial_objective);
        let recovering_speed = timeline.relative_speed();
        assert!(
            recovering_speed > slowed_speed && recovering_speed <= 1.0,
            "excluding the only lead outside the deadband must start speed recovery"
        );
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_some()
        );

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
            app.world().resource::<InputTimeline>().relative_speed(),
            recovering_speed,
            "missing observations must preserve controller hysteresis"
        );
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_some()
        );

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
            app.world().resource::<InputTimeline>().relative_speed(),
            1.0
        );
        assert_eq!(
            app.world().resource::<LocalTimeline>().tick(),
            initial_objective.tick(),
            "a running P2P session must not relabel local ticks"
        );
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_some()
        );
        let divergences = &app.world().resource::<RecordedDivergences>().0;
        assert_eq!(divergences.len(), 1);
        assert_eq!(divergences[0].entity, timeline_entity);
        assert_eq!(divergences[0].limiting_link, links[0]);
        assert_eq!(divergences[0].lead, 20.0);

        // A P2P roster change resets controller history but cannot revoke readiness or resynchronize
        // a running deterministic world. An empty connected set keeps application time advancing
        // normally; the session owner is responsible for treating peer loss as fatal if required.
        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::P2P {
            connected: Default::default(),
            declared_links: 3,
        };
        app.world_mut().run_schedule(PostUpdate);

        let timeline = app.world().resource::<InputTimeline>();
        assert_eq!(timeline.now(), initial_objective);
        assert_eq!(timeline.relative_speed(), 1.0);
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_some()
        );

        app.world_mut().run_schedule(Last);
        assert_eq!(
            app.world().resource::<Time<Virtual>>().relative_speed(),
            1.0
        );

        app.world_mut().resource_mut::<NetworkingMetadata>().mode = NetworkTopology::Undefined;
        app.world_mut().run_schedule(PostUpdate);
        app.world_mut().run_schedule(Last);
        assert!(
            app.world()
                .get::<IsSynced<InputTimeline>>(timeline_entity)
                .is_none()
        );
        assert_eq!(
            app.world().resource::<Time<Virtual>>().relative_speed(),
            1.0,
            "leaving P2P topology must restore normal application time"
        );
    }
}
