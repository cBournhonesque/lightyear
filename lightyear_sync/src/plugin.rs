use crate::ping::plugin::PingPlugin;
use bevy::app::{App, Plugin};
use bevy::prelude::*;

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone, Copy)]
pub enum SyncSet {
    /// Sync SyncedTimelines to the Remote timelines using networking information (RTT/jitter) from the PingManager
    Sync,
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
