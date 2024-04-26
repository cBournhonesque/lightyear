//! Defines the client bevy plugin
use std::ops::DerefMut;
use std::sync::Mutex;

use bevy::prelude::*;

use crate::client::connection::ConnectionManager;
use crate::client::diagnostics::ClientDiagnosticsPlugin;
use crate::client::events::ClientEventsPlugin;
use crate::client::input::InputPlugin;
use crate::client::interpolation::plugin::InterpolationPlugin;
use crate::client::networking::ClientNetworkingPlugin;
use crate::client::prediction::plugin::PredictionPlugin;
use crate::client::replication::ClientReplicationPlugin;
use crate::connection::client::{ClientConnection, NetConfig};
use crate::prelude::MessageRegistry;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
use crate::server::plugin::ServerPlugin;
use crate::shared::config::Mode;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::events::plugin::EventsPlugin;
use crate::shared::plugin::SharedPlugin;
use crate::shared::time_manager::TimePlugin;
use crate::transport::PacketSender;

use super::config::ClientConfig;

pub struct PluginConfig<P: Protocol> {
    client_config: ClientConfig,
    protocol: P,
}

impl<P: Protocol> PluginConfig<P> {
    pub fn new(client_config: ClientConfig, protocol: P) -> Self {
        PluginConfig {
            client_config,
            protocol,
        }
    }
}

pub struct ClientPlugin<P: Protocol> {
    // we add Mutex<Option> so that we can get ownership of the inner from an immutable reference in build()
    config: Mutex<Option<PluginConfig<P>>>,
}

impl<P: Protocol> ClientPlugin<P> {
    pub fn new(config: PluginConfig<P>) -> Self {
        Self {
            config: Mutex::new(Some(config)),
        }
    }
}

// TODO: create this as PluginGroup so that users can easily disable sub plugins?
// TODO: override `ready` and `finish` to make sure that the transport/backend is connected
//  before the plugin is ready
impl<P: Protocol> Plugin for ClientPlugin<P> {
    fn build(&self, app: &mut App) {
        let config = self.config.lock().unwrap().deref_mut().take().unwrap();

        app
            // RESOURCES //
            .insert_resource(config.client_config.clone())
            .insert_resource(config.protocol.clone())
            .init_resource::<MessageRegistry>()
            // PLUGINS //
            .add_plugins(ClientNetworkingPlugin::<P>::default())
            .add_plugins(ClientEventsPlugin::<P>::default())
            .add_plugins(ClientDiagnosticsPlugin::<P>::default())
            // .add_plugins(ClientReplicationPlugin::<P>::default())
            .add_plugins(PredictionPlugin::<P>::default())
            .add_plugins(InterpolationPlugin::<P>::new(
                config.client_config.interpolation.clone(),
            ));

        // TODO: how do we make sure that SharedPlugin is only added once if we want to switch between
        //  HostServer and Separate mode?
        if config.client_config.shared.mode == Mode::Separate {
            app
                // PLUGINS
                .add_plugins(SharedPlugin::<P> {
                    config: config.client_config.shared.clone(),
                    ..default()
                });
        }
    }
}
