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

use crate::server::plugin::ServerPlugin;
use crate::shared::config::Mode;
use crate::shared::events::connection::ConnectionEvents;
use crate::shared::events::plugin::EventsPlugin;
use crate::shared::plugin::SharedPlugin;
use crate::shared::time_manager::TimePlugin;
use crate::transport::PacketSender;

use super::config::ClientConfig;

pub struct ClientPlugin {
    pub config: ClientConfig,
}

impl ClientPlugin {
    pub fn new(config: ClientConfig) -> Self {
        Self { config }
    }
}

// TODO: create this as PluginGroup so that users can easily disable sub plugins?
// TODO: override `ready` and `finish` to make sure that the transport/backend is connected
//  before the plugin is ready
impl Plugin for ClientPlugin {
    fn build(&self, app: &mut App) {
        app
            // RESOURCES //
            .insert_resource(self.config.clone());

        // TODO: how do we make sure that SharedPlugin is only added once if we want to switch between
        //  HostServer and Separate mode?
        if self.config.shared.mode == Mode::Separate {
            app
                // PLUGINS
                .add_plugins(SharedPlugin {
                    config: self.config.shared.clone(),
                    ..default()
                });
        }

        app
            // PLUGINS //
            .add_plugins(ClientNetworkingPlugin::default())
            .add_plugins(ClientEventsPlugin::default())
            .add_plugins(ClientDiagnosticsPlugin::default())
            .add_plugins(ClientReplicationPlugin::default())
            .add_plugins(PredictionPlugin::default())
            .add_plugins(InterpolationPlugin::new(self.config.interpolation.clone()));
    }
}
