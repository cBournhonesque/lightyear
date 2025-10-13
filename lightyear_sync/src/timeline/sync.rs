use crate::ping::manager::PingManager;
use crate::plugin::SyncSystems;
use bevy_app::{App, Last, Plugin, PostUpdate};
use bevy_ecs::prelude::*;
use bevy_reflect::Reflect;
use bevy_time::{Time, Virtual};
use bevy_utils::prelude::DebugName;
use core::time::Duration;
use lightyear_connection::client::{Connected, Disconnected};
use lightyear_connection::host::HostClient;
use lightyear_core::prelude::{LocalTimeline, NetworkTimelinePlugin};
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{TickDelta, TickInstant};
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
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> TickInstant;

    /// Resync the timeline if they are too out of sync. Returns the number of tick deltas
    /// that should be applied
    fn resync(&mut self, sync_objective: TickInstant) -> i16;

    /// Sync the current timeline to the other timeline T.
    /// Usually this is achieved by slightly speeding up or slowing down the current timeline.
    /// If there is a big discrepancy we can do a `resync` instead.
    ///
    /// Returns the number of delta ticks that should be applied
    // TODO: should we use LinkStats instead of PingManager? and PingManager is a way to update the LinkStats?
    fn sync<Remote: SyncTargetTimeline>(
        &mut self,
        main: &Remote,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> Option<i16>;

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
    /// How many ticks to we apply as margin when computing the time
    ///  a packet will get received by the server
    pub jitter_margin: Duration,
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
    pub(crate) fn jitter_margin(&self, jitter: Duration) -> Duration {
        jitter * self.jitter_multiple as u32 + self.jitter_margin
    }
}

impl Default for SyncConfig {
    fn default() -> Self {
        SyncConfig {
            jitter_multiple: 4,
            jitter_margin: Duration::from_millis(5),
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

#[derive(Debug)]
pub enum SyncAdjustment {
    Resync,
    SpeedAdjust(f32),
    DoNothing,
}

impl SyncConfig {
    pub fn speed_adjustment(&mut self, offset: f32) -> SyncAdjustment {
        let current_error_sign = offset.is_sign_positive();
        let previous_error_sign = self.previous_error_sign;
        self.previous_error_sign = current_error_sign;
        if offset.abs() > self.max_error_margin {
            self.consecutive_errors = 0;
            SyncAdjustment::Resync
        } else if offset.abs() > self.error_margin {
            self.consecutive_errors = self.consecutive_errors.saturating_add(1);
            // skip if we haven't seen enough consecutive errors in the same direction
            if (current_error_sign ^ previous_error_sign)
                || self.consecutive_errors < self.consecutive_errors_threshold
            {
                self.previous_error_sign = current_error_sign;
                return SyncAdjustment::DoNothing;
            }
            let base_factor = self.speedup_factor - 1.0;
            let error_ratio = (offset.abs() / self.max_error_margin).clamp(0.0, 1.0);

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

/// Plugin to sync the Synced timeline to the Remote timeline
///
/// The const generic is used to indicate if the timeline is driving/updating the [`Time<Virtual>`] and [`LocalTimeline`].
pub struct SyncedTimelinePlugin<Synced, Remote, const DRIVING: bool = false> {
    pub(crate) _marker: core::marker::PhantomData<(Synced, Remote)>,
}

impl<Synced: SyncedTimeline, Remote: SyncTargetTimeline, const DRIVING: bool>
    SyncedTimelinePlugin<Synced, Remote, DRIVING>
{
    /// On connection, reset the Synced timeline.
    pub(crate) fn handle_connect(
        trigger: On<Add, Connected>,
        mut query: Query<(&mut Synced, &LocalTimeline)>,
    ) {
        if let Ok((mut timeline, local_timeline)) = query.get_mut(trigger.entity) {
            timeline.reset();
            if DRIVING {
                trace!("Set Driving timeline tick to LocalTimeline");
                let delta = local_timeline.tick() - timeline.tick();
                timeline.apply_delta(delta.into());
            }
        }
    }

    /// For HostClient, we directly set IsSynced on connection (since there is no messages exchanged) and the
    /// Synced timeline is always the same as the Remote timeline
    pub(crate) fn handle_host_client(trigger: On<Add, HostClient>, mut commands: Commands) {
        commands
            .entity(trigger.entity)
            .insert(IsSynced::<Synced>::default());
    }

    /// On disconnection, remove IsSynced.
    pub(crate) fn handle_disconnect(trigger: On<Add, Disconnected>, mut commands: Commands) {
        commands.entity(trigger.entity).remove::<IsSynced<Synced>>();
    }

    pub(crate) fn update_virtual_time(
        mut virtual_time: ResMut<Time<Virtual>>,
        query: Query<&Synced, (With<IsSynced<Synced>>, With<Connected>, Without<HostClient>)>,
    ) {
        if let Ok(timeline) = query.single() {
            trace!(
                "Timeline {} sets the virtual time relative speed to {}",
                DebugName::type_name::<Synced>(),
                timeline.relative_speed()
            );
            // TODO: be able to apply the speed_ratio on top of any speed ratio already applied by the user.
            virtual_time.set_relative_speed(timeline.relative_speed());
        }
    }

    /// Sync timeline T to timeline [`RemoteTimeline`](super::remote::RemoteTimeline) by either
    /// - speeding up/slowing down the timeline T to match timeline Remote
    /// - emitting a [`SyncEvent<T>`]
    pub(crate) fn sync_timelines(
        tick_duration: Res<TickDuration>,
        mut commands: Commands,
        mut query: Query<
            (
                Entity,
                &mut Synced,
                &Remote,
                &mut LocalTimeline,
                &PingManager,
                Has<IsSynced<Synced>>,
            ),
            (With<Connected>, Without<HostClient>),
        >,
    ) {
        // TODO: return early if we haven't received any remote packets? (nothing to sync to)
        query.iter_mut().for_each(|(entity, mut sync_timeline, main_timeline, mut local_timeline, ping_manager, has_is_synced)| {
            trace!(?entity, ?has_is_synced, "In SyncTimelines from {:?} to {:?}", DebugName::type_name::<Synced>(), DebugName::type_name::<Remote>());
            // return early if the remote timeline hasn't received any packets
            if !main_timeline.received_packet() {
                return;
            }
            if !has_is_synced && sync_timeline.is_synced()  {
                debug!("Timeline {:?} is synced to {:?}", DebugName::type_name::<Synced>(), DebugName::type_name::<Remote>());
                commands.entity(entity).insert(IsSynced::<Synced>::default());
            }
            if let Some(tick_delta) = sync_timeline.sync(main_timeline, ping_manager, tick_duration.0) {
                // if it's the driving pipeline, also update the LocalTimeline
                if DRIVING {
                    let local_tick = local_timeline.tick();
                    let synced_tick = sync_timeline.tick();
                    local_timeline.apply_delta(TickDelta::from_i16(tick_delta));
                    debug!(
                        ?local_tick, ?synced_tick, ?tick_delta, new_local_tick = ?local_timeline.tick(),
                        "Apply delta to LocalTimeline from driving pipeline {:?}'s SyncEvent", DebugName::type_name::<Synced>());
                }
                commands.trigger(SyncEvent::<Synced::Context>::new(entity, tick_delta));
            }
        })
    }
}

impl<Synced, Remote, const DRIVING: bool> Default
    for SyncedTimelinePlugin<Synced, Remote, DRIVING>
{
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<Synced: SyncedTimeline, Remote: SyncTargetTimeline, const DRIVING: bool> Plugin
    for SyncedTimelinePlugin<Synced, Remote, DRIVING>
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
            app.add_systems(Last, Self::update_virtual_time);
        }
    }
}
