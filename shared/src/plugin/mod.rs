use bevy_app::prelude::Plugin;
use bevy_app::App;

pub use replication::ReplicationData;
pub use sets::ReplicationSet;

mod replication;
mod sets;
pub mod systems;

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReplicationData>();
    }
}
