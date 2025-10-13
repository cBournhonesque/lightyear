use crate::ping::plugin::PingPlugin;
use bevy_app::{App, Plugin, PostUpdate};
use bevy_ecs::schedule::SystemSet;

#[deprecated(since = "0.25", note = "Use SyncSystems instead")]
pub type SyncSet = SyncSystems;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum SyncSystems {
    /// Sync SyncedTimelines to the Remote timelines using networking information (RTT/jitter) from the PingManager
    Sync,
}

pub struct TimelineSyncPlugin;

impl Plugin for TimelineSyncPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PingPlugin>() {
            app.add_plugins(PingPlugin);
        }
        app.configure_sets(PostUpdate, SyncSystems::Sync);
    }
}
