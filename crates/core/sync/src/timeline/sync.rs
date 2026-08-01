use crate::ping::manager::PingManager;
use crate::plugin::SyncSystems;
use bevy_app::{App, Last, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_ecs::resource::IsResource;
use bevy_reflect::Reflect;
use bevy_time::{Fixed, Time, Virtual};
use bevy_utils::prelude::DebugName;
use core::time::Duration;
use lightyear_connection::client::{Client, Connected, Disconnected};
use lightyear_connection::host::HostClient;
use lightyear_core::prelude::{LocalTimeline, NetworkTimelinePlugin};
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{Overstep, TickInstant};
use lightyear_core::timeline::{NetworkTimeline, SyncEvent};
#[allow(unused_imports)]
use tracing::{debug, info, trace};

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
        clients: Query<(), With<Client>>,
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
        clients: Query<(), With<Client>>,
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
        clients: Query<(), With<Client>>,
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

    /// Apply the synchronized resource timeline's relative speed to virtual time.
    fn update_virtual_time(
        timeline: Single<&Synced, (With<IsResource>, With<IsSynced<Synced>>)>,
        mut virtual_time: ResMut<Time<Virtual>>,
    ) {
        Self::apply_relative_speed(&timeline, &mut virtual_time);
    }

    /// Synchronize the resource timeline with the single connected, non-host client link.
    ///
    /// P2P aggregation will provide a different remote-target policy while retaining the same
    /// resource timeline and synchronization controller.
    fn sync_timelines(
        tick_duration: Res<TickDuration>,
        config: Res<Synced::Config>,
        timeline: Single<(Entity, &mut Synced, Has<IsSynced<Synced>>), With<IsResource>>,
        remote: Single<
            (&Remote, &PingManager),
            (With<Client>, With<Connected>, Without<HostClient>),
        >,
        mut commands: Commands,
    ) {
        let (entity, mut synced, is_synced) = timeline.into_inner();
        let (remote, ping_manager) = remote.into_inner();
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
