use crate::plugin::TimelineSyncPlugin;
use bevy::app::{App, Plugin};

pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TimelineSyncPlugin>() {
            app.add_plugins(TimelineSyncPlugin);
        }
    }
}
