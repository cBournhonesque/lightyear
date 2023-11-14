use bevy::prelude::{App, Fixed, FixedUpdate, Plugin, Time};
use tracing::Level;

use crate::plugin::systems::tick::increment_tick;
use config::SharedConfig;
pub use replication::ReplicationData;
pub use sets::ReplicationSet;

pub(crate) mod config;
pub mod events;
pub(crate) mod log;
mod replication;
pub mod sets;
pub mod systems;

pub struct SharedPlugin {
    pub config: SharedConfig,
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // RESOURCES
        // NOTE: this tick duration must be the same as any previous existing fixed timesteps
        app.insert_resource(Time::<Fixed>::from_seconds(
            self.config.tick.tick_duration.as_secs_f64(),
        ));
        app.init_resource::<ReplicationData>();
        // SYSTEMS
        // app.add_systems(FixedUpdate, increment_tick);

        // TODO: set log config
        app.add_plugins(log::LogPlugin {
            level: Level::DEBUG,
            filter: "wgpu=error,bevy_render=warn,naga=error,bevy_app=error,bevy=error".to_string(),
        });
    }
}
