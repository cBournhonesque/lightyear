use crate::ping::manager::PingManager;
use crate::ping::plugin::PingPlugin;
use crate::timeline::sync::{IsSynced, SyncedTimeline};
use crate::timeline::DrivingTimeline;
use bevy::app::{App, FixedFirst, Plugin};
use bevy::prelude::*;
use lightyear_core::timeline::{NetworkTimeline, NetworkTimelinePlugin, Timeline, TimelineContext};
use tracing::trace;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum SyncSet {
    /// Sync SyncedTimelines to the Remote timelines using networking information (RTT/jitter) from the PingManager
    Sync,
}


pub struct SyncedTimelinePlugin<Synced, Remote>{
    pub(crate) _marker: core::marker::PhantomData<(Synced, Remote)>,
}

impl<Synced, Remote> Default for SyncedTimelinePlugin<Synced, Remote> {
    fn default() -> Self {
        Self {
            _marker: core::marker::PhantomData,
        }
    }
}

impl<Synced: SyncedTimeline, Remote: NetworkTimeline + Default> Plugin
for SyncedTimelinePlugin<Synced, Remote>
{
    fn build(&self, app: &mut App) {
        app.add_plugins(NetworkTimelinePlugin::<Synced>::default());

        app.register_required_components::<Synced, PingManager>();
        app.register_required_components::<Synced, Remote>();
        app.add_systems(FixedFirst, SyncPlugin::advance_synced_timelines::<Synced>);
        // NOTE: we don't have to run this in PostUpdate, we could run this right after RunFixedMainLoop?
        app.add_systems(PostUpdate,
            SyncPlugin::sync_timelines::<Synced, Remote>.in_set(SyncSet::Sync));
    }
}

pub struct SyncPlugin;

impl SyncPlugin {


    pub(crate) fn update_virtual_time<Synced: SyncedTimeline>(
        mut virtual_time: ResMut<Time<Virtual>>,
        query: Query<&Synced, With<DrivingTimeline<Synced>>>)
    {
        if let Ok(timeline) = query.single() {
            trace!("Set virtual time relative speed to {}", timeline.relative_speed());
            // TODO: be able to apply the speed_ratio on top of any speed ratio already applied by the user.
            virtual_time.set_relative_speed(timeline.relative_speed());
        }
    }

    /// Update the timeline in FixedUpdate based on the Time<Virtual>
    /// Should we use this only in FixedUpdate::First? because we need the tick in FixedUpdate to be correct for the timeline
    pub(crate) fn advance_synced_timelines<Synced: SyncedTimeline>(
        fixed_time: Res<Time<Fixed>>,
        mut query: Query<(&mut Synced, Has<DrivingTimeline<Synced>>)>,
    ) {
        let delta = fixed_time.delta();
        query.iter_mut().for_each(|(mut t, is_main)| {
            // the main timeline has already been used to update the game's speed, so we don't want to apply the relative_speed again!
            if is_main {
                t.apply_duration(delta);
            } else {
                let new_delta = delta.mul_f32(t.relative_speed());
                t.apply_duration(new_delta);
            }
        })
    }

    /// Sync timeline T to timeline Remote by either
    /// - speeding up/slowing down the timeline T to match timeline Remote
    /// - emitting a SyncEvent<T>
    pub(crate) fn sync_timelines<Synced: SyncedTimeline, Remote: NetworkTimeline> (
        mut commands: Commands,
        mut query: Query<(Entity, &mut Synced, &Remote, &PingManager)>,
    ) {
        query.iter_mut().for_each(|(entity, mut sync_timeline, main_timeline, ping_manager)| {
            if let Some(sync_event) = sync_timeline.sync(main_timeline, ping_manager) {
                commands.trigger_targets(sync_event, entity);
                commands.entity(entity).insert(IsSynced::<Synced>::default());
            }
        })
    }

}


impl Plugin for SyncPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PingPlugin>() {
            app.add_plugins(PingPlugin);
        }
        app.configure_sets(PostUpdate, SyncSet::Sync);
    }
}