//! Bevy [`bevy::prelude::Plugin`] used by both the server and the client
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use replication::hierarchy::HierarchySyncPlugin;

use crate::client::config::ClientConfig;
use crate::prelude::Protocol;
use crate::shared::config::SharedConfig;
use crate::shared::replication;
use crate::shared::tick_manager::TickManagerPlugin;

pub struct SharedPlugin<P: Protocol> {
    pub config: SharedConfig,
    pub _marker: std::marker::PhantomData<P>,
}

impl<P: Protocol> Default for SharedPlugin<P> {
    fn default() -> Self {
        Self {
            config: SharedConfig::default(),
            _marker: std::marker::PhantomData,
        }
    }
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

impl<P: Protocol> Plugin for SharedPlugin<P> {
    fn build(&self, app: &mut App) {
        // RESOURCES
        // NOTE: this tick duration must be the same as any previous existing fixed timesteps
        app.insert_resource(Time::<Fixed>::from_seconds(
            self.config.tick.tick_duration.as_secs_f64(),
        ));

        // PLUGINS
        app.add_plugins(HierarchySyncPlugin::<P>::default());
        app.add_plugins(TickManagerPlugin {
            config: self.config.tick.clone(),
        });
    }
}
