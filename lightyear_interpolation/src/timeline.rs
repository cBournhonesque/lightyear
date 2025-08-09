//! When a ReplicationSender first connects to a ReplicationReceiver, it sends a
//! a trigger to inform the receiver of its SendInterval. This interval is used
//! by the receiver to determine how the InterpolationTime should be configured

use bevy_app::{App, Plugin, PreUpdate};
use bevy_derive::{Deref, DerefMut};
use bevy_ecs::{
    component::Component,
    observer::Trigger,
    query::{With, Without},
    system::{Query, Res},
};
use bevy_reflect::Reflect;
use bevy_time::{Time, Virtual};
use bevy_utils::default;
use core::time::Duration;
use lightyear_connection::client::{Client, Connected};
use lightyear_core::prelude::Rollback;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::{TickDelta, TickInstant};
use lightyear_core::timeline::{NetworkTimeline, SyncEvent, Timeline, TimelineContext};
use lightyear_messages::prelude::RemoteTrigger;
use lightyear_replication::message::SenderMetadata;
use lightyear_sync::prelude::PingManager;
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::timeline::sync::{
    SyncAdjustment, SyncConfig, SyncTargetTimeline, SyncedTimeline, SyncedTimelinePlugin,
};
use tracing::{debug, trace};

/// Config to specify how the snapshot interpolation should behave
#[derive(Clone, Copy, Debug, Reflect)]
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
}

impl Default for InterpolationConfig {
    fn default() -> Self {
        Self {
            min_delay: Duration::from_millis(5),
            send_interval_ratio: 1.7,
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

#[derive(Default, Reflect)]
pub struct Interpolation {
    tick_duration: Duration,
    relative_speed: f32,
    pub remote_send_interval: Duration,
    pub interpolation_config: InterpolationConfig,
    pub sync_config: SyncConfig,
    is_synced: bool,
}

#[derive(Component, Deref, DerefMut, Default, Reflect)]
pub struct InterpolationTimeline(Timeline<Interpolation>);

impl TimelineContext for Interpolation {}

impl InterpolationTimeline {
    fn new(
        tick_duration: Duration,
        interpolation_config: InterpolationConfig,
        sync_config: SyncConfig,
    ) -> Self {
        let interpolation = Interpolation {
            tick_duration,
            interpolation_config,
            sync_config,
            ..default()
        };
        InterpolationTimeline(interpolation.into())
    }
}

impl SyncedTimeline for InterpolationTimeline {
    fn sync_objective<T: SyncTargetTimeline>(
        &self,
        remote: &T,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> TickInstant {
        let delay = TickDelta::from_duration(
            self.interpolation_config
                .to_duration(self.remote_send_interval),
            tick_duration,
        );
        // take extra margin if there is jitter
        let jitter_margin = TickDelta::from_duration(
            ping_manager.jitter() * self.sync_config.jitter_multiple as u32
                + self.sync_config.jitter_margin,
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

    fn resync(&mut self, sync_objective: TickInstant) -> SyncEvent<Self> {
        let now = self.now();
        let target = sync_objective;
        let tick_delta = (target - now).to_i16();
        trace!(?tick_delta, "Resync Interpolation timeline!");
        self.now = target;
        SyncEvent::<Self> {
            tick_delta,
            marker: core::marker::PhantomData,
        }
    }

    // TODO: this code is duplicated in the Predicted timeline
    /// Adjust the current timeline to stay in sync with the [`RemoteTimeline`].
    ///
    /// Most of the times this will just be slight nudges to modify the speed of the [`SyncedTimeline`].
    /// If there's a big discrepancy, we will snap the [`SyncedTimeline`] to the [`RemoteTimeline`] by sending a SyncEvent
    fn sync<T: SyncTargetTimeline>(
        &mut self,
        remote: &T,
        ping_manager: &PingManager,
        tick_duration: Duration,
    ) -> Option<SyncEvent<Self>> {
        // skip syncing if we haven't received enough information
        if ping_manager.pongs_recv < self.sync_config.handshake_pings as u32 {
            return None;
        }
        self.is_synced = true;
        // TODO: should we call current_estimate()? now() should basically return the same thing
        let now = self.now();
        let objective = self.sync_objective(remote, ping_manager, tick_duration);
        let error = now - objective;
        let error_ticks = error.to_f32();
        let adjustment = self.sync_config.speed_adjustment(error_ticks);
        trace!(
            ?now,
            ?objective,
            ?adjustment,
            ?error_ticks,
            error_margin = ?self.sync_config.error_margin,
            max_error_margin = ?self.sync_config.max_error_margin,
            "InterpolationTimeline sync"
        );
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
        self.is_synced = false;
        self.relative_speed = 1.0;
        self.now = Default::default();
        // TODO: also reset tick duration?
    }
}

pub struct TimelinePlugin;

impl TimelinePlugin {
    fn receive_sender_metadata(
        trigger: Trigger<RemoteTrigger<SenderMetadata>>,
        tick_duration: Res<TickDuration>,
        mut query: Query<&mut InterpolationTimeline>,
    ) {
        let delta = TickDelta::from(trigger.trigger.send_interval);
        let duration = delta.to_duration(tick_duration.0);
        query.iter_mut().for_each(|mut interpolation_timeline| {
            debug!("Updating remote send interval to {:?}", duration);
            interpolation_timeline.context.remote_send_interval = duration;
        })
    }

    /// Update the timeline in Update based on the [`Time<Virtual>`]
    pub(crate) fn advance_timeline(
        time: Res<Time<Virtual>>,
        tick_duration: Res<TickDuration>,
        // make sure to not update the timelines during Rollback
        mut query: Query<&mut InterpolationTimeline, (With<Connected>, Without<Rollback>)>,
    ) {
        let delta = time.delta();
        query.iter_mut().for_each(|mut t| {
            // make sure to account for the fact that Time<Virtual> is already updated from the Driving timeline
            let new_delta = delta
                .div_f32(time.relative_speed())
                .mul_f32(t.relative_speed());
            trace!("Interpolation timeline advance by {new_delta:?}");
            t.apply_duration(new_delta, tick_duration.0);
        })
    }
}

impl Plugin for TimelinePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<InterpolationTimeline>();
        app.register_required_components::<Client, InterpolationTimeline>();
        app.add_plugins(SyncedTimelinePlugin::<InterpolationTimeline, RemoteTimeline>::default());
        app.add_systems(PreUpdate, Self::advance_timeline);
        app.add_observer(Self::receive_sender_metadata);
    }
}
