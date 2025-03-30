use crate::ping::manager::PingManager;
use crate::ping::plugin::{PingPlugin, PingSet};
use crate::timeline::sync::SyncedTimeline;
use crate::timeline::{Main, Timeline};
use bevy::app::{App, Plugin};
use bevy::prelude::{Commands, Entity, Fixed, Has, PostUpdate, Query, Res, ResMut, SystemSet, Time, Virtual, With};

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum SyncSet {
    /// Sync SyncedTimelines to the Remote timelines using networking information (RTT/jitter) from the PingManager
    Sync,
}

pub struct SyncPlugin;

impl SyncPlugin {
    pub(crate) fn update_virtual_time<T: SyncedTimeline>(
        mut virtual_time: ResMut<Time<Virtual>>,
        query: Query<&T, With<Main<T>>>)
    {
        if let Ok(timeline) = query.single() {
            // TODO: be able to apply the speed_ratio on top of any speed ratio already applied by the user.
            virtual_time.set_relative_speed(timeline.relative_speed());
        }
    }

    /// Update the timeline in FixedUpdate based on the Time<Virtual>
    /// Should we use this only in FixedUpdate::First? because we need the tick in FixedUpdate to be correct for the timeline
    pub(crate) fn advance_timelines<T: SyncedTimeline>(
        fixed_time: Res<Time<Fixed>>,
        mut query: Query<(&mut T, Has<Main<T>>)>,
    ) {
        let delta = fixed_time.delta();
        query.iter_mut().for_each(|(mut t, is_main)| {
            // the main timeline has already been used to update the game's speed, so we don't want to apply the relative_speed again!
            if is_main {
                t.advance(delta);
            } else {
                t.advance(delta * t.relative_speed());
            }
        })
    }

    /// Sync timeline T to timeline M
    pub(crate) fn sync_timelines<T: SyncedTimeline, M: Timeline>(
        mut commands: Commands,
        mut query: Query<(Entity, &mut T, &M, &PingManager)>,
    ) {
        query.iter_mut().for_each(|(entity, mut sync_timeline, main_timeline, ping_manager)| {
            if let Some(sync_event) = sync_timeline.sync(main_timeline, ping_manager) {
                commands.trigger_targets(sync_event, entity);
            }
        })
    }
}


impl Plugin for SyncPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(PingPlugin);
        app.configure_sets(PostUpdate, (PingSet::Send, SyncSet::Sync));
    }
}