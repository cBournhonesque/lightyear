use crate::timeline::TimelinePlugin;
use bevy::app::App;
use bevy::prelude::Plugin;
use core::time::Duration;

pub struct CorePlugins {
    pub tick_duration: Duration,
}

impl Plugin for CorePlugins {
    fn build(&self, app: &mut App) {
        app.add_plugins(TimelinePlugin {
            tick_duration: self.tick_duration,
        });
    }
}