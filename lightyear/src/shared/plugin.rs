//! Bevy [`bevy::prelude::Plugin`] used by both the server and the client
use crate::_internal::ShouldBeInterpolated;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::prelude::{
    AppComponentExt, ChannelDirection, ChannelRegistry, ComponentRegistry, IoConfig,
    LinkConditionerConfig, MessageRegistry, Mode, PingConfig, PrePredicted, PreSpawnedPlayerObject,
    ShouldBePredicted, TickConfig, TransportConfig,
};
use crate::server::config::ServerConfig;
use crate::server::message::ServerMessage::Message;
use crate::shared::config::SharedConfig;
use crate::shared::tick_manager::TickManagerPlugin;
use crate::shared::time_manager::TimePlugin;

#[derive(Default, Debug)]
pub struct SharedPlugin {
    pub config: SharedConfig,
}

/// You can use this as a SystemParam to identify whether you're running on the client or the server
#[derive(SystemParam)]
pub struct NetworkIdentity<'w, 's> {
    config: Option<Res<'w, ServerConfig>>,
    _marker: std::marker::PhantomData<&'s ()>,
}

impl<'w, 's> NetworkIdentity<'w, 's> {
    pub fn is_client(&self) -> bool {
        self.config.is_none()
    }
    pub fn is_server(&self) -> bool {
        self.config.is_some()
    }
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<Mode>()
            .register_type::<SharedConfig>()
            .register_type::<TickConfig>()
            .register_type::<PingConfig>()
            .register_type::<LinkConditionerConfig>()
            .register_type::<IoConfig>();

        // RESOURCES
        // NOTE: this tick duration must be the same as any previous existing fixed timesteps
        app.insert_resource(ChannelRegistry::new());
        app.insert_resource(ComponentRegistry::default());
        app.insert_resource(MessageRegistry::default());
        app.insert_resource(Time::<Fixed>::from_seconds(
            self.config.tick.tick_duration.as_secs_f64(),
        ));

        // PLUGINS
        // we always keep running the tick_manager and time_manager even the client or server are stopped
        app.add_plugins(TickManagerPlugin {
            config: self.config.tick.clone(),
        });
        app.add_plugins(TimePlugin {
            server_send_interval: self.config.server_send_interval,
            client_send_interval: self.config.client_send_interval,
        });
    }
}
