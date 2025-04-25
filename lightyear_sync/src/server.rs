use crate::plugin::SyncPlugin;
use bevy::app::{App, Plugin};
use lightyear_connection::client_of::Server;
use lightyear_core::prelude::LocalTimeline;

pub struct ServerPlugin;

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<SyncPlugin>() {
            app.add_plugins(SyncPlugin);
        }
        app.register_required_components::<Server, LocalTimeline>();
    }
}