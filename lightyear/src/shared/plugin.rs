//! Bevy [`bevy::prelude::Plugin`] used by both the server and the client
use crate::client::prediction::plugin::is_in_rollback;
use crate::client::prediction::Rollback;
use crate::prelude::{FixedUpdateSet, TickManager};
use bevy::app::FixedUpdate;
use bevy::prelude::*;

use crate::shared::config::SharedConfig;
use crate::shared::log;
use crate::shared::tick_manager::TickManagerPlugin;

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

        // PLUGINS
        // TODO: increment_tick should be shared
        // app.add_systems(FixedUpdate, increment_tick);
        let log_config = self.config.log.clone();
        app.add_plugins(log::LogPlugin { config: log_config });
        app.add_plugins(TickManagerPlugin {
            config: self.config.tick.clone(),
        });
    }
}
