//! Bevy [`Plugin`] used by both the server and the client
use crate::client::config::ClientConfig;
use crate::connection::client::{ClientConnection, NetClient};
use crate::connection::server::ServerConnections;
use crate::prelude::client::ComponentSyncMode;
use crate::prelude::server::NetworkingState;
use crate::prelude::{
    AppComponentExt, AppMessageExt, ChannelDirection, ChannelRegistry, ClientId, ComponentRegistry,
    LinkConditionerConfig, MessageRegistry, Mode, ParentSync, PingConfig, PrePredicted,
    PreSpawnedPlayerObject, ShouldBePredicted, TickConfig,
};
use crate::server::run_conditions::is_started_ref;
use crate::shared::config::SharedConfig;
use crate::shared::replication::authority::AuthorityChange;
use crate::shared::replication::components::{Controlled, ShouldBeInterpolated};
use crate::shared::tick_manager::TickManagerPlugin;
use crate::shared::time_manager::TimePlugin;
use crate::transport::io::{IoState, IoStats};
use crate::transport::middleware::compression::CompressionConfig;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::utils::Duration;

#[derive(Default, Debug)]
pub struct SharedPlugin {
    pub config: SharedConfig,
}

/// You can use this as a SystemParam to identify whether you're running on the client or the server
#[derive(SystemParam)]
pub struct NetworkIdentity<'w, 's> {
    client: Option<Res<'w, ClientConnection>>,
    client_config: Option<Res<'w, ClientConfig>>,
    server: Option<Res<'w, ServerConnections>>,
    server_state: Option<Res<'w, State<NetworkingState>>>,
    _marker: std::marker::PhantomData<&'s ()>,
}

/// Identifies the network role of the current peer
#[derive(Debug, PartialEq)]
pub enum Identity {
    /// This peer is a client.
    /// (note that both the client and server plugins could be running in the same process; but this peer is still acting like a client.
    /// (for example if the server plugin is stopped))
    ///
    /// If the client is connected, also contains the client id. If not, contains None.
    Client(Option<ClientId>),
    /// This peer is a server.
    Server,
    /// This peer is both a server and a client
    HostServer,
}

impl FromWorld for Identity {
    fn from_world(world: &mut World) -> Self {
        let Some(config) = world.get_resource::<ClientConfig>() else {
            return Identity::Server;
        };
        if matches!(config.shared.mode, Mode::HostServer)
            && is_started_ref(world.get_resource_ref::<State<NetworkingState>>())
        {
            Identity::HostServer
        } else {
            let client_id = world
                .get_resource::<ClientConnection>()
                .as_ref()
                .map(|c| c.client.id());
            Identity::Client(client_id)
        }
    }
}

impl Identity {
    pub fn is_client(&self) -> bool {
        matches!(self, &Identity::Client(_))
    }
    pub fn is_server(&self) -> bool {
        self == &Identity::Server || self == &Identity::HostServer
    }
}

impl NetworkIdentity<'_, '_> {
    pub fn identity(&self) -> Identity {
        let Some(config) = &self.client_config else {
            return Identity::Server;
        };
        if matches!(config.shared.mode, Mode::HostServer)
            && self
                .server_state
                .as_ref()
                .is_some_and(|s| s.get() == &NetworkingState::Started)
        {
            Identity::HostServer
        } else {
            let client_id = self.client.as_ref().map(|c| c.client.id());
            Identity::Client(client_id)
        }
    }
    pub fn is_client(&self) -> bool {
        self.identity().is_client()
    }
    pub fn is_server(&self) -> bool {
        self.identity().is_server()
    }
}

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        // REFLECTION
        app.register_type::<Mode>()
            .register_type::<SharedConfig>()
            .register_type::<TickConfig>()
            .register_type::<PingConfig>()
            .register_type::<IoStats>()
            .register_type::<IoState>()
            .register_type::<LinkConditionerConfig>()
            .register_type::<CompressionConfig>();

        // PLUGINS
        #[cfg(feature = "avian2d")]
        app.add_plugins(crate::utils::avian2d::Avian2dPlugin);
        #[cfg(feature = "avian3d")]
        app.add_plugins(crate::utils::avian3d::Avian3dPlugin);
        #[cfg(feature = "visualizer")]
        {
            if !app.is_plugin_added::<bevy_metrics_dashboard::bevy_egui::EguiPlugin>() {
                app.add_plugins(bevy_metrics_dashboard::bevy_egui::EguiPlugin);
            }
            app.add_plugins(bevy_metrics_dashboard::RegistryPlugin::default())
                .add_plugins(bevy_metrics_dashboard::DashboardPlugin);
            app.add_systems(Startup, spawn_metrics_visualizer);
        }

        // RESOURCES
        // the SharedPlugin is called after the ClientConfig is inserted
        let input_send_interval =
            if let Some(client_config) = app.world().get_resource::<ClientConfig>() {
                // use the input_send_interval on the client
                client_config.input.send_interval
            } else {
                // on the server (when rebroadcasting inputs), send inputs every frame
                Duration::default()
            };
        app.insert_resource(ChannelRegistry::new(input_send_interval));
        app.insert_resource(ComponentRegistry::default());
        app.insert_resource(MessageRegistry::default());
        // NOTE: this tick duration must be the same as any previous existing fixed timesteps
        app.insert_resource(Time::<Fixed>::from_seconds(
            self.config.tick.tick_duration.as_secs_f64(),
        ));

        // PLUGINS
        // we always keep running the tick_manager and time_manager even the client or server are stopped
        app.add_plugins(TickManagerPlugin {
            config: self.config.tick,
        });
        app.add_plugins(TimePlugin);
    }

    fn finish(&self, app: &mut App) {
        // PROTOCOL
        // we register components here because
        // - the SharedPlugin is built only once in HostServer mode (client and server plugins in the same app)
        // (if we put this in the ReplicationPlugin, the components would get registered twice)
        // - we need to run this in `finish` so that all plugins have been built (so ClientPlugin and ServerPlugin
        // both exists)
        app.register_component::<PreSpawnedPlayerObject>(ChannelDirection::Bidirectional);
        app.register_component::<PrePredicted>(ChannelDirection::Bidirectional);
        app.register_component::<ShouldBePredicted>(ChannelDirection::ServerToClient);
        app.register_component::<ShouldBeInterpolated>(ChannelDirection::ServerToClient);
        app.register_component::<ParentSync>(ChannelDirection::Bidirectional)
            // to replicate ParentSync on the predicted/interpolated entities so that they spawn their own hierarchies
            .add_prediction(ComponentSyncMode::Simple)
            .add_interpolation(ComponentSyncMode::Simple)
            .add_map_entities();
        app.register_component::<Controlled>(ChannelDirection::ServerToClient)
            .add_prediction(ComponentSyncMode::Once)
            .add_interpolation(ComponentSyncMode::Once);

        app.register_message::<AuthorityChange>(ChannelDirection::ServerToClient)
            .add_map_entities();

        // check that the protocol was built correctly
        app.world().resource::<ComponentRegistry>().check();
    }
}

#[cfg(feature = "visualizer")]
fn spawn_metrics_visualizer(mut commands: Commands) {
    commands.spawn(bevy_metrics_dashboard::DashboardWindow::new(
        "Metrics Dashboard",
    ));
}
