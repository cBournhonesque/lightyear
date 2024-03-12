//! Defines the client bevy plugin
use std::ops::DerefMut;
use std::sync::Mutex;

use bevy::prelude::*;

use crate::client::connection::ConnectionManager;
use crate::client::diagnostics::ClientDiagnosticsPlugin;
use crate::client::events::ClientEventsPlugin;
use crate::client::input::InputPlugin;
use crate::client::interpolation::plugin::InterpolationPlugin;
use crate::client::metadata::MetadataPlugin;
use crate::client::networking::ClientNetworkingPlugin;
use crate::client::prediction::plugin::PredictionPlugin;
use crate::client::replication::ClientReplicationPlugin;
use crate::protocol::component::ComponentProtocol;
use crate::protocol::message::MessageProtocol;
use crate::protocol::Protocol;
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

// TODO: override `ready` and `finish` to make sure that the transport/backend is connected
//  before the plugin is ready
impl<P: Protocol> Plugin for ClientPlugin<P> {
    fn build(&self, app: &mut App) {
        let config = self.config.lock().unwrap().deref_mut().take().unwrap();

        let netclient = config.client_config.net.clone().build_client();
        let tick_duration = config.client_config.shared.tick.tick_duration;

        app
            // RESOURCES //
            .insert_resource(config.client_config.clone())
            // TODO: move these into the Networking/Replication plugins
            .insert_resource(netclient)
            .insert_resource(ConnectionManager::<P>::new(
                config.protocol.channel_registry(),
                config.client_config.packet,
                config.client_config.sync,
                config.client_config.ping,
                config.client_config.prediction.input_delay_ticks,
            ))
            // PLUGINS //
            .add_plugins(SharedPlugin::<P> {
                config: config.client_config.shared.clone(),
                ..default()
            })
            .add_plugins(ClientEventsPlugin::<P>::default())
            .add_plugins(ClientNetworkingPlugin::<P>::default())
            .add_plugins(ClientReplicationPlugin::<P>::new(tick_duration))
            .add_plugins(MetadataPlugin)
            .add_plugins(InputPlugin::<P>::default())
            .add_plugins(PredictionPlugin::<P>::new(config.client_config.prediction))
            .add_plugins(InterpolationPlugin::<P>::new(
                config.client_config.interpolation.clone(),
            ))
            .add_plugins(TimePlugin {
                send_interval: config.client_config.shared.client_send_interval,
            })
            .add_plugins(ClientDiagnosticsPlugin::<P>::default());
    }
}
