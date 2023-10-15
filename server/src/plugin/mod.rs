mod systems;

use bevy_app::{App, Plugin as PluginType};

pub struct Plugin {}

impl Plugin {
    pub fn new() -> Self {
        Self {}
    }
}

impl PluginType for Plugin {
    fn build(&self, app: &mut App) {
        // let client = Client::new();

        // app
        // RESOURCES //
        // .insert_resource(client);
        // EVENTS //
        // SYSTEM SETS //
        // SYSTEMS //
    }
}
