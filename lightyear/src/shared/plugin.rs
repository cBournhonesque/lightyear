//! Bevy [`bevy::prelude::Plugin`] used by both the server and the client
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::client::config::ClientConfig;
use crate::shared::config::SharedConfig;
use crate::shared::log;
use crate::shared::tick_manager::TickManagerPlugin;

pub struct SharedPlugin {
    pub config: SharedConfig,
}

/// You can use this as a SystemParam to identify whether you're running on the client or the server
#[derive(SystemParam)]
pub struct NetworkIdentity<'w, 's> {
    config: Option<Res<'w, ClientConfig>>,
    _marker: std::marker::PhantomData<&'s ()>,
}

impl<'w, 's> NetworkIdentity<'w, 's> {
    pub fn is_client(&self) -> bool {
        self.config.is_some()
    }
    pub fn is_server(&self) -> bool {
        self.config.is_none()
    }
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
        app.add_plugins(TickManagerPlugin {
            config: self.config.tick.clone(),
        });
    }
}
