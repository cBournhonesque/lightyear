use crate::timeline::TimelinePlugin;
use bevy::app::App;
use bevy::prelude::Plugin;
use bevy::time::TimePlugin;
use core::time::Duration;

pub struct CorePlugins {
    pub tick_duration: Duration,
}

impl Plugin for CorePlugins {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<TimePlugin>() {
            app.add_plugins(TimePlugin);
        }
        app.add_plugins(TimelinePlugin {
            tick_duration: self.tick_duration,
        });
    }
}