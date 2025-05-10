//! When a ReplicationSender first connects to a ReplicationReceiver, it sends a
//! a trigger to inform the receiver of its SendInterval. This interval is used
//! by the receiver to determine how the InterpolationTime should be configured

use crate::manager::InterpolationManager;
use bevy::ecs::component::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use bevy::prelude::{default, Component, Deref, DerefMut, Reflect};
use core::time::Duration;
use lightyear_connection::client::{Client, Connected};
use lightyear_connection::direction::NetworkDirection;
use lightyear_core::prelude::Rollback;
use lightyear_core::tick::{Tick, TickDuration};
use lightyear_core::time::{Overstep, PositiveTickDelta, TickDelta, TickInstant, TimeDelta};
use lightyear_core::timeline::{NetworkTimeline, SyncEvent, Timeline};
use lightyear_messages::prelude::{AppTriggerExt, RemoteTrigger};
use lightyear_replication::message::SenderMetadata;
use lightyear_replication::prelude::ReplicationSender;
use lightyear_serde::reader::Reader;
use lightyear_serde::writer::WriteInteger;
use lightyear_serde::{SerializationError, ToBytes};
use lightyear_sync::plugin::SyncedTimelinePlugin;
use lightyear_sync::prelude::client::RemoteTimeline;
use lightyear_sync::prelude::{DrivingTimeline, PingManager};
use lightyear_sync::timeline::sync::{SyncConfig, SyncedTimeline};
use tracing::trace;

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
    pub(crate) interpolation_config: InterpolationConfig,
    pub(crate) sync_config: SyncConfig,
    is_synced: bool,
}


#[derive(Component, Deref, DerefMut, Default, Reflect)]
pub struct InterpolationTimeline(Timeline<Interpolation>);

impl InterpolationTimeline {

    fn new(tick_duration: Duration, interpolation_config: InterpolationConfig, sync_config: SyncConfig) -> Self {
        let mut interpolation = Interpolation {
            tick_duration,
            interpolation_config,
            sync_config,
            ..default()
        };
        InterpolationTimeline(interpolation.into())
    }
}

impl SyncedTimeline for InterpolationTimeline {
    fn sync_objective<T: NetworkTimeline>(&self, remote: &T, ping_manager: &PingManager, tick_duration: Duration) -> TickInstant {
        let delay = TickDelta::from_duration(self.interpolation_config.to_duration(self.remote_send_interval), tick_duration);
        // take extra margin if there is jitter
        let jitter_margin = TickDelta::from_duration(ping_manager.jitter() * self.sync_config.jitter_multiple as u32 + self.sync_config.jitter_margin, tick_duration);
        let target = remote.now();
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
    /// Adjust the current timeline to stay in sync with the [`MainTimeline`].
    ///
    /// Most of the times this will just be slight nudges to modify the speed of the [`SyncedTimeline`].
    /// If there's a big discrepancy, we will snap the [`SyncedTimeline`] to the [`MainTimeline`] by sending a SyncEvent
    fn sync<T: NetworkTimeline>(&mut self, remote: &T, ping_manager: &PingManager, tick_duration: Duration) -> Option<SyncEvent<Self>> {
        // skip syncing if we haven't received enough information
        if ping_manager.pongs_recv < self.sync_config.handshake_pings as u32 {
            return None
        }
        self.is_synced = true;
        // TODO: should we call current_estimate()? now() should basically return the same thing
        let now = self.now();
        let objective = self.sync_objective(remote, ping_manager, tick_duration);
        let error = now - objective;
        let is_ahead = error.is_positive();
        let error_duration = error.to_duration(tick_duration);
        let error_margin = tick_duration.mul_f32(self.sync_config.error_margin);
        let max_error_margin = tick_duration.mul_f32(self.sync_config.max_error_margin);
        trace!(?now, ?objective, ?error_duration, ?is_ahead, ?error_margin, ?max_error_margin, "InterpolationTimeline sync");
        if error_duration > max_error_margin {
            return Some(self.resync(objective));
        } else if error_duration > error_margin {
            let ratio = if is_ahead {
                1.0 / self.sync_config.speedup_factor
            } else {
                1.0 * self.sync_config.speedup_factor
            };
            self.set_relative_speed(ratio);
        }
        // if it's the first time, we still send a SyncEvent (so that IsSynced is inserted)
        if ping_manager.pongs_recv == self.sync_config.handshake_pings as u32 {
            return Some(SyncEvent::new(0));
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

    /// Update the timeline in Update based on the Time<Virtual>
    pub(crate) fn advance_timeline(
        time: Res<Time>,
        tick_duration: Res<TickDuration>,
        // make sure to not update the timelines during Rollback
        mut query: Query<&mut InterpolationTimeline, (With<Connected>, Without<Rollback>)>,
    ) {
        let delta = time.delta();
        query.iter_mut().for_each(|mut t| {
            // TODO: maybe the interpolation timeline should progress at the same speed at the main timeline?
            //  the driving timeline already updates based on our estimate of the remote timeline, so maybe there's no need to
            //  do extra speedups on top of that!
            // let new_delta = delta.mul_f32(t.relative_speed());
            let new_delta = delta;
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