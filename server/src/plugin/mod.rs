use std::ops::DerefMut;
use std::sync::Mutex;

use bevy_app::{App, Plugin as PluginType, PostUpdate, PreUpdate};
use bevy_ecs::prelude::{IntoSystemConfigs, IntoSystemSetConfig};

use lightyear_shared::{Protocol, ReplicationSend, ReplicationSet};

use crate::config::ServerConfig;
use crate::plugin::sets::ServerSet;
use crate::plugin::systems::{receive, send};
use crate::Server;

pub(crate) mod events;
mod sets;
mod systems;

pub struct PluginConfig<P: Protocol> {
    server_config: ServerConfig,
    protocol: P,
}

// TODO: put all this in ClientConfig?
impl<P: Protocol> PluginConfig<P> {
    pub fn new(server_config: ServerConfig, protocol: P) -> Self {
        PluginConfig {
            server_config,
            protocol,
        }
    }
}
pub struct Plugin<P: Protocol> {
    // we add Mutex<Option> so that we can get ownership of the inner from an immutable reference
    // in build()
    config: Mutex<Option<PluginConfig<P>>>,
}

impl<P: Protocol> Plugin<P> {
    pub fn new(config: PluginConfig<P>) -> Self {
        Self {
            config: Mutex::new(Some(config)),
        }
    }
}

impl<P: Protocol> PluginType for Plugin<P> {
    fn build(&self, app: &mut App) {
        let mut config = self.config.lock().unwrap().deref_mut().take().unwrap();
        let server = Server::new(config.server_config, config.protocol);

        P::add_replication_send_systems::<Server<P>>(app);

        app
            // RESOURCES //
            .insert_resource(server)
            // SYSTEM SETS //
            .configure_set(PreUpdate, ServerSet::Receive)
            .configure_set(PostUpdate, ReplicationSet::Send)
            .configure_set(PostUpdate, ServerSet::Send.after(ReplicationSet::Send))
            // EVENTS //
            .add_event::<events::ConnectEvent>()
            .add_event::<events::DisconnectEvent>()
            // SYSTEMS //
            .add_systems(PreUpdate, receive::<P>.in_set(ServerSet::Receive))
            .add_systems(PostUpdate, send::<P>.in_set(ServerSet::Send));
    }
}
