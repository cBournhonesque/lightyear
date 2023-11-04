use bevy::prelude::{App, Plugin};
use tracing::Level;

pub use replication::ReplicationData;
pub use sets::ReplicationSet;

pub mod events;
pub(crate) mod log;
mod replication;
mod sets;
pub mod systems;

pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ReplicationData>();

        // TODO: set log config
        app.add_plugins(log::LogPlugin {
            level: Level::DEBUG,
            filter: "wgpu=error,bevy_render=warn,naga=error,bevy_app=info".to_string(),
        });
    }
}
