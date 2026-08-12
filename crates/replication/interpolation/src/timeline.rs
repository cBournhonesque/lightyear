//! When a ReplicationSender first connects to a ReplicationReceiver, it sends a
//! a trigger to inform the receiver of its SendInterval. This interval is used
//! by the receiver to determine how the InterpolationTime should be configured

use bevy_app::{App, Plugin, PostUpdate, PreUpdate};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::change_detection::Tick as ChangeTick;
use bevy_ecs::prelude::*;
use bevy_ecs::query::FilteredAccessSet;
use bevy_ecs::system::{ReadOnlySystemParam, SystemMeta, SystemParam, SystemParamValidationError};
use bevy_ecs::world::unsafe_world_cell::UnsafeWorldCell;
use bevy_reflect::Reflect;
use bevy_time::{Time, Virtual};
use core::{marker::PhantomData, time::Duration};
use lightyear_connection::client::{Client, Connected, Disconnected, PeerMetadata};
use lightyear_connection::host::HostClient;
use lightyear_connection::network_topology::{NetworkTopology, NetworkingMetadata};
use lightyear_connection::p2p::P2P;
use lightyear_core::prelude::{Rollback, TimelineSystems};
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{TickDelta, TickInstant};
use lightyear_core::timeline::{NetworkTimeline, Timeline, TimelineConfig};
use lightyear_messages::prelude::RemoteEvent;
use lightyear_replication::metadata::SenderMetadata;
use lightyear_sync::plugin::SyncSystems;
use lightyear_sync::prelude::PingManager;
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::timeline::sync::{SyncConfig, SyncContext, SyncTargetTimeline, TimelineSync};
use tracing::{debug, trace};

/// Config to specify how the snapshot interpolation should behave
#[derive(Resource, Clone, Copy, Debug, Reflect)]
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
    pub sync: SyncConfig,
}

impl Default for InterpolationConfig {
    fn default() -> Self {
        Self {
            min_delay: Duration::from_millis(5),
            send_interval_ratio: 1.7,
            sync: SyncConfig::default(),
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

impl TimelineConfig for InterpolationConfig {
    type Context = InterpolationContext;
    type Timeline = InterpolationTimeline;
}

#[derive(Default, Debug, Reflect)]
pub struct InterpolationContext {
    pub(crate) remote_send_interval: Duration,
    sync: SyncContext,
    relative_speed: f32,
    is_synced: bool,
}

/// Application-global presentation timeline used to sample interpolation histories.
///
/// Unlike the local simulation clock, this is a genuine independent timeline: it intentionally
/// trails remote simulation time and advances at its own synchronization-controlled speed.
#[derive(Resource, Default, Deref, DerefMut, Reflect)]
pub struct InterpolationTimeline(Timeline<InterpolationConfig>);

impl InterpolationTimeline {
    /// Returns whether the presentation timeline has synchronized to a remote source.
    pub fn is_synced(&self) -> bool {
        self.context.is_synced
    }
}

/// Read-only access to the application-global [`InterpolationTimeline`] after it has synchronized.
///
/// Systems using this parameter are skipped until [`InterpolationTimeline::is_synced`] returns
/// true. The parameter dereferences directly to the presentation timeline.
pub struct SyncedInterpolationTimeline<'w, 's> {
    timeline: Res<'w, InterpolationTimeline>,
    marker: PhantomData<&'s ()>,
}

impl core::ops::Deref for SyncedInterpolationTimeline<'_, '_> {
    type Target = InterpolationTimeline;

    fn deref(&self) -> &Self::Target {
        &self.timeline
    }
}

// SAFETY: This parameter delegates all access to the read-only `Res<InterpolationTimeline>`
// parameter.
unsafe impl SystemParam for SyncedInterpolationTimeline<'_, '_> {
    type State = <Res<'static, InterpolationTimeline> as SystemParam>::State;
    type Item<'world, 'state> = SyncedInterpolationTimeline<'world, 'state>;

    fn init_state(world: &mut World) -> Self::State {
        <Res<'static, InterpolationTimeline> as SystemParam>::init_state(world)
    }

    fn init_access(
        state: &Self::State,
        system_meta: &mut SystemMeta,
        component_access_set: &mut FilteredAccessSet,
        world: &mut World,
    ) {
        <Res<'static, InterpolationTimeline> as SystemParam>::init_access(
            state,
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
        // SAFETY: `init_access` delegated to `Res<InterpolationTimeline>`, and the caller
        // guarantees that this is the same World used by `init_state`.
        let timeline = unsafe {
            <Res<'static, InterpolationTimeline> as SystemParam>::get_param(
                state,
                system_meta,
                world,
                change_tick,
            )
        }?;
        if !timeline.is_synced() {
            return Err(SystemParamValidationError::skipped::<Self>(
                "InterpolationTimeline is not synchronized",
            ));
        }
        Ok(SyncedInterpolationTimeline {
            timeline,
            marker: PhantomData,
        })
    }
}

// SAFETY: `SyncedInterpolationTimeline` only delegates to read-only system parameters.
unsafe impl ReadOnlySystemParam for SyncedInterpolationTimeline<'_, '_> {}

impl TimelineSync for InterpolationTimeline {
    type Config = InterpolationConfig;

    fn sync_context(&mut self) -> &mut SyncContext {
        &mut self.context.sync
    }

    fn sync_config(config: &Self::Config) -> &SyncConfig {
        &config.sync
    }

    fn sync_objective<T: SyncTargetTimeline>(
        &self,
        remote: &T,
        config: &InterpolationConfig,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> TickInstant {
        let delay =
            TickDelta::from_duration(config.to_duration(self.remote_send_interval), tick_duration);
        // take extra margin if there is jitter
        let jitter_margin = TickDelta::from_duration(
            config
                .sync
                .jitter_margin(ping_manager.jitter(), tick_duration),
            tick_duration,
        );
        let target = remote.current_estimate();
        let obj = target - (delay + jitter_margin);
        trace!(
            ?target,
            ?delay,
            ?jitter_margin,
            send_interval = ?self.remote_send_interval,
            "InterpolationTimeline objective: {:?}", obj
        );
        obj
    }

    fn resync(&mut self, now: TickInstant, sync_objective: TickInstant) -> i32 {
        let target = sync_objective;
        let tick_delta = (target - now).to_i32();
        trace!(?tick_delta, "Resync Interpolation timeline!");
        self.now = target;
        tick_delta
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
        self.sync = SyncContext::default();
        self.is_synced = false;
        self.relative_speed = 1.0;
        self.now = Default::default();
    }
}

pub struct TimelinePlugin;

impl TimelinePlugin {
    /// Reset the presentation timeline for a newly connected conventional client session.
    fn handle_connect(
        trigger: On<Add, Connected>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        mut timeline: ResMut<InterpolationTimeline>,
    ) {
        if clients.contains(trigger.entity) {
            timeline.reset();
            timeline.context.remote_send_interval = Duration::default();
        }
    }

    /// Mark an in-process host client's presentation timeline ready without network sampling.
    fn handle_host_client(
        trigger: On<Add, HostClient>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        mut timeline: ResMut<InterpolationTimeline>,
    ) {
        if clients.contains(trigger.entity) {
            timeline.set_synced(true);
        }
    }

    /// Reset presentation synchronization when a conventional client disconnects.
    fn handle_disconnect(
        trigger: On<Add, Disconnected>,
        clients: Query<(), (With<Client>, Without<P2P>)>,
        mut timeline: ResMut<InterpolationTimeline>,
    ) {
        if clients.contains(trigger.entity) {
            timeline.reset();
            timeline.context.remote_send_interval = Duration::default();
        }
    }

    /// Cache the replication interval advertised by the remote replication source.
    ///
    /// A conventional client has one source, so new metadata replaces the previous value. P2P
    /// uses the largest observed interval as a conservative delay for its single presentation
    /// cursor.
    fn receive_sender_metadata(
        trigger: On<RemoteEvent<SenderMetadata>>,
        tick_duration: Res<TickDuration>,
        peer_metadata: Res<PeerMetadata>,
        metadata: Res<NetworkingMetadata>,
        mut interpolation_timeline: ResMut<InterpolationTimeline>,
    ) {
        let delta = TickDelta::from(trigger.trigger.send_interval);
        let duration = delta.to_duration(tick_duration.0);
        if !peer_metadata.mapping.contains_key(&trigger.from) {
            return;
        }
        let is_p2p = metadata.mode.is_p2p();
        debug!("Updating remote send interval to {:?}", duration);
        // A single presentation cursor must be safe for every active replication source. Use the
        // slowest advertised update interval across joined P2P peers; conventional client/server
        // mode has only one source.
        interpolation_timeline.context.remote_send_interval = if is_p2p {
            core::cmp::max(
                interpolation_timeline.context.remote_send_interval,
                duration,
            )
        } else {
            duration
        };
    }

    /// Run one interpolation synchronization sample against the selected remote source.
    fn sync_with_remote(
        source: Entity,
        timeline: &mut InterpolationTimeline,
        config: &InterpolationConfig,
        remote: &RemoteTimeline,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) {
        if !remote.received_packet() {
            return;
        }
        let before = timeline.now();
        let tick_delta = timeline.sync(before, remote, config, ping_manager, tick_duration);
        trace!(
            target: "lightyear_debug::sync",
            kind = "interpolation_sync",
            schedule = "PostUpdate",
            sample_point = "PostUpdate",
            ?source,
            ?before,
            after = ?timeline.now(),
            remote_estimate = ?remote.current_estimate(),
            ?tick_delta,
            relative_speed = timeline.relative_speed(),
            "synchronized the application-global presentation timeline"
        );
    }

    /// Select the presentation source for the current topology and synchronize the resource.
    ///
    /// Client/server mode follows its sole server. P2P mode selects the earliest delayed
    /// objective, keeping the single presentation cursor behind every active replication source.
    fn sync_timeline(
        tick_duration: Res<TickDuration>,
        metadata: Res<NetworkingMetadata>,
        config: Res<InterpolationConfig>,
        mut timeline: ResMut<InterpolationTimeline>,
        remotes: Query<
            (&RemoteTimeline, &PingManager),
            (With<Client>, With<Connected>, Without<HostClient>),
        >,
    ) {
        if metadata.is_changed() {
            // Source-set changes reset controller hysteresis without rewinding the presentation
            // cursor. The selected source below can immediately snap it to a new valid objective.
            let now = timeline.now();
            timeline.reset();
            timeline.set_now(now);
        }

        match &metadata.mode {
            NetworkTopology::Client(client) => {
                let Ok((remote, ping_manager)) = remotes.get(*client) else {
                    return;
                };
                Self::sync_with_remote(
                    *client,
                    &mut timeline,
                    &config,
                    remote,
                    ping_manager,
                    tick_duration.0,
                );
            }
            NetworkTopology::P2P(joined) => {
                let mut selected = None;
                for &link in joined {
                    let Ok((remote, ping_manager)) = remotes.get(link) else {
                        continue;
                    };
                    if !remote.received_packet() {
                        continue;
                    }
                    if ping_manager.latency_samples_recv() < config.sync.handshake_pings as u32 {
                        continue;
                    }
                    let objective =
                        timeline.sync_objective(remote, &config, ping_manager, tick_duration.0);
                    if selected.is_none_or(|(_, earliest)| objective < earliest) {
                        selected = Some((link, objective));
                    }
                }
                let Some((source, _)) = selected else {
                    return;
                };
                let Ok((remote, ping_manager)) = remotes.get(source) else {
                    return;
                };
                Self::sync_with_remote(
                    source,
                    &mut timeline,
                    &config,
                    remote,
                    ping_manager,
                    tick_duration.0,
                );
            }
            NetworkTopology::HostClient { .. } => timeline.set_synced(true),
            NetworkTopology::Undefined
            | NetworkTopology::Server(_)
            | NetworkTopology::Invalid(_) => timeline.set_synced(false),
        }
    }

    /// Update the timeline in PreUpdate based on the [`Time<Virtual>`]
    pub(crate) fn advance_timeline(
        time: Res<Time<Virtual>>,
        tick_duration: Res<TickDuration>,
        // make sure to not update the timelines during Rollback
        rollback: Option<Res<Rollback>>,
        mut timeline: ResMut<InterpolationTimeline>,
    ) {
        if rollback.is_some() {
            return;
        }
        // A deterministic prediction-window wait pauses `Time<Virtual>` so FixedUpdate cannot
        // advance beyond the rollback window. Its delta is already zero in that state; avoid
        // dividing by the zero relative speed while reconstructing the unscaled frame delta.
        if time.relative_speed() == 0.0 {
            return;
        }
        let delta = time.delta();
        // Make sure to account for the fact that Time<Virtual> is already updated from the local
        // timeline synchronization controller.
        let new_delta = delta
            .div_f32(time.relative_speed())
            .mul_f32(timeline.relative_speed());
        trace!("Interpolation timeline advance by {new_delta:?}");
        timeline.apply_duration(new_delta, tick_duration.0);
    }
}

impl Plugin for TimelinePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InterpolationTimeline>();
        app.init_resource::<InterpolationConfig>();
        app.add_observer(Self::handle_connect);
        app.add_observer(Self::handle_host_client);
        app.add_observer(Self::handle_disconnect);
        app.add_systems(PostUpdate, Self::sync_timeline.in_set(SyncSystems::Sync));
        app.add_systems(
            PreUpdate,
            Self::advance_timeline.in_set(TimelineSystems::Advance),
        );
        app.add_observer(Self::receive_sender_metadata);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_app::Update;

    #[derive(Resource, Default)]
    struct SyncedParamRuns(u8);

    #[test]
    fn synced_interpolation_timeline_checks_presentation_readiness() {
        fn require_synced_timeline(
            timeline: SyncedInterpolationTimeline,
            mut runs: ResMut<SyncedParamRuns>,
        ) {
            let _ = timeline.now();
            runs.0 += 1;
        }

        let mut app = App::new();
        app.init_resource::<InterpolationTimeline>();
        app.init_resource::<SyncedParamRuns>();
        app.add_systems(Update, require_synced_timeline);

        app.update();
        assert_eq!(app.world().resource::<SyncedParamRuns>().0, 0);

        app.world_mut()
            .resource_mut::<InterpolationTimeline>()
            .set_synced(true);

        app.update();
        assert_eq!(app.world().resource::<SyncedParamRuns>().0, 1);
    }

    #[test]
    fn paused_virtual_time_does_not_advance_interpolation_timeline() {
        let mut app = App::new();
        let mut virtual_time = Time::<Virtual>::default();
        virtual_time.set_relative_speed(0.0);
        app.insert_resource(virtual_time);
        app.insert_resource(TickDuration(Duration::from_millis(16)));
        app.add_systems(PreUpdate, TimelinePlugin::advance_timeline);

        app.init_resource::<InterpolationTimeline>();
        let before = app.world().resource::<InterpolationTimeline>().now();

        app.update();

        let after = app.world().resource::<InterpolationTimeline>().now();
        assert_eq!(after, before);
    }
}
