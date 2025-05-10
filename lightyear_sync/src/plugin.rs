use crate::ping::manager::PingManager;
use crate::ping::plugin::PingPlugin;
use crate::prelude::InputTimeline;
use crate::timeline::sync::{IsSynced, SyncTargetTimeline, SyncedTimeline};
use crate::timeline::DrivingTimeline;
use bevy::app::{App, FixedFirst, Plugin};
use bevy::prelude::*;
use lightyear_connection::client::{Connected, Disconnect, Disconnected};
use lightyear_core::prelude::LocalTimeline;
use lightyear_core::tick::TickDuration;
use lightyear_core::time::TickDelta;
use lightyear_core::timeline::{NetworkTimeline, NetworkTimelinePlugin, Rollback};

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum SyncSet {
    /// Sync SyncedTimelines to the Remote timelines using networking information (RTT/jitter) from the PingManager
    Sync,
}


/// Plugin to sync the Synced timeline to the Remote timeline
///
/// The const generic is used to indicate if the timeline is driving/updating the Time<Virtual> and LocalTimeline.
pub struct SyncedTimelinePlugin<Synced, Remote, const DRIVING: bool = false>{
    pub(crate) _marker: core::marker::PhantomData<(Synced, Remote)>,
}

impl<Synced: SyncedTimeline, Remote: SyncTargetTimeline, const DRIVING: bool> SyncedTimelinePlugin<Synced, Remote, DRIVING> {
    /// On connection, reset the Synced timeline.
    pub(crate) fn handle_connect(
        trigger: Trigger<OnAdd, Connected>,
        mut query: Query<(&mut Synced, &LocalTimeline)>,
    ) {
        if let Ok((mut timeline, local_timeline)) = query.get_mut(trigger.target()) {
            timeline.reset();
            if DRIVING {
                let delta = local_timeline.tick() - timeline.tick();
                timeline.apply_delta(delta.into());
            }
        }
    }


    /// On disconnection, remove IsSynced.
    pub(crate) fn handle_disconnect(
        trigger: Trigger<OnAdd, Disconnected>,
        mut commands: Commands
    ) {
        commands.entity(trigger.target()).remove::<IsSynced<Synced>>();
    }


    pub(crate) fn update_virtual_time(
        mut virtual_time: ResMut<Time<Virtual>>,
        query: Query<&Synced, (With<IsSynced<Synced>>, With<DrivingTimeline<Synced>>, With<Connected>)>)
    {
        if let Ok(timeline) = query.single() {
            trace!("Timeline {} sets the virtual time relative speed to {}", core::any::type_name::<Synced>(), timeline.relative_speed());
            // TODO: be able to apply the speed_ratio on top of any speed ratio already applied by the user.
            virtual_time.set_relative_speed(timeline.relative_speed());
        }
    }

    /// Sync timeline T to timeline Remote by either
    /// - speeding up/slowing down the timeline T to match timeline Remote
    /// - emitting a SyncEvent<T>
    pub(crate) fn sync_timelines(
        tick_duration: Res<TickDuration>,
        mut commands: Commands,
        mut query: Query<(Entity, &mut Synced, &Remote, &mut LocalTimeline, &PingManager, Has<IsSynced<Synced>>), With<Connected>>,
    ) {
        // TODO: return early if we haven't received any remote packets? (nothing to sync to)

        query.iter_mut().for_each(|(entity, mut sync_timeline, main_timeline, mut local_timeline, ping_manager, has_is_synced)| {
            // return early if the remote timeline hasn't received any packets
            if !main_timeline.is_ready() {
                return;
            }
            if !has_is_synced && sync_timeline.is_synced()  {
                debug!("Timeline {:?} is synced to {:?}", core::any::type_name::<Synced>(), core::any::type_name::<Remote>());
                commands.entity(entity).insert(IsSynced::<Synced>::default());
            }
            if let Some(sync_event) = sync_timeline.sync(main_timeline, ping_manager, tick_duration.0) {
                // if it's the driving pipeline, also update the LocalTimeline
                if DRIVING {
                    let local_tick = local_timeline.tick();
                    let synced_tick = sync_timeline.tick();
                    let delta = sync_event.tick_delta;
                    local_timeline.apply_delta(TickDelta::from_i16(sync_event.tick_delta));
                    debug!(
                        ?local_tick, ?synced_tick, ?delta, new_local_tick = ?local_timeline.tick(),
                        "Apply delta to LocalTimeline from driving pipeline {:?}'s SyncEvent", core::any::type_name::<Synced>());
                }
                commands.trigger_targets(sync_event, entity);
            }
        })
    }

}

impl<Synced, Remote, const DRIVING: bool> Default for SyncedTimelinePlugin<Synced, Remote, DRIVING> {
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
        app.add_observer(Self::handle_disconnect);
        // NOTE: we don't have to run this in PostUpdate, we could run this right after RunFixedMainLoop?
        app.add_systems(PostUpdate, Self::sync_timelines.in_set(SyncSet::Sync));
        if DRIVING {
            app.add_systems(Last, Self::update_virtual_time);
        }
    }
}

pub struct SyncPlugin;


impl Plugin for SyncPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PingPlugin>() {
            app.add_plugins(PingPlugin);
        }
        app.configure_sets(PostUpdate, SyncSet::Sync);
    }
}