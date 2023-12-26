use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use bevy::prelude::{default, App, Mut, PluginGroup, Real, Time};
use bevy::time::TimeUpdateStrategy;
use bevy::utils::HashMap;
use bevy::MinimalPlugins;

use lightyear::client as lightyear_client;
use lightyear::netcode::generate_key;
use lightyear::prelude::client::{
    Authentication, ClientConfig, InputConfig, InterpolationConfig, PredictionConfig, SyncConfig,
};
use lightyear::prelude::server::{NetcodeConfig, ServerConfig};
use lightyear::prelude::*;
use lightyear::server as lightyear_server;

use crate::protocol::*;

// Sometimes it takes time for socket to receive all data.
const SOCKET_WAIT: Duration = Duration::from_millis(5);

/// Helpers to setup a bevy app where I can just step the world easily

pub trait Step {
    /// Advance both apps by one frame duration
    fn frame_step(&mut self);

    /// Advance both apps by on fixed timestep duration
    fn tick_step(&mut self);
}

pub struct BevyStepper {
    pub client_apps: HashMap<ClientId, App>,
    pub server_app: App,
    pub frame_duration: Duration,
    /// fixed timestep duration
    pub tick_duration: Duration,
    pub current_time: std::time::Instant,
}

// Do not forget to use --features mock_time when using the LinkConditioner
impl BevyStepper {
    pub fn new(
        num_clients: usize,
        shared_config: SharedConfig,
        sync_config: SyncConfig,
        prediction_config: PredictionConfig,
        interpolation_config: InterpolationConfig,
        frame_duration: Duration,
    ) -> Self {
        let now = std::time::Instant::now();
        let local_addr = SocketAddr::from_str("127.0.0.1:0").unwrap();

        // Shared config
        let protocol_id = 0;
        let private_key = generate_key();

        // Setup server
        let mut server_app = App::new();
        server_app.add_plugins(MinimalPlugins.build());
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(protocol_id)
            .with_key(private_key);
        let io = Io::from_config(&IoConfig::from_transport(TransportConfig::UdpSocket(
            local_addr,
        )));
        let server_addr = io.local_addr();
        let config = ServerConfig {
            shared: shared_config.clone(),
            netcode: netcode_config,
            ..default()
        };
        let plugin_config = server::PluginConfig::new(config, io, protocol());
        let plugin = server::ServerPlugin::new(plugin_config);
        server_app.add_plugins(plugin);

        // Setup client
        let mut client_apps = HashMap::new();
        for i in 0..num_clients {
            let client_id = i as ClientId;
            let mut client_app = App::new();
            client_app.add_plugins(MinimalPlugins.build());
            let auth = Authentication::Manual {
                server_addr,
                protocol_id,
                private_key,
                client_id,
            };
            // let addr = SocketAddr::from_str(&format!("127.0.0.1:{}", i)).unwrap();
            let io = Io::from_config(&IoConfig::from_transport(TransportConfig::UdpSocket(
                local_addr,
            )));
            let config = ClientConfig {
                shared: shared_config.clone(),
                sync: sync_config.clone(),
                prediction: prediction_config,
                interpolation: interpolation_config.clone(),
                ..default()
            };
            let plugin_config = client::PluginConfig::new(config, io, protocol(), auth);
            let plugin = client::ClientPlugin::new(plugin_config);
            client_app.add_plugins(plugin);
            // Initialize Real time (needed only for the first TimeSystem run)
            client_app
                .world
                .get_resource_mut::<Time<Real>>()
                .unwrap()
                .update_with_instant(now);
            client_apps.insert(client_id, client_app);
        }

        server_app
            .world
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);

        Self {
            client_apps,
            server_app,
            frame_duration,
            tick_duration: shared_config.tick.tick_duration,
            current_time: now,
        }
    }

    pub fn client(&self, client_id: ClientId) -> &Client {
        self.client_apps
            .get(&client_id)
            .unwrap()
            .world
            .resource::<Client>()
    }

    pub fn client_mut(&mut self, client_id: ClientId) -> Mut<Client> {
        self.client_apps
            .get_mut(&client_id)
            .unwrap()
            .world
            .resource_mut::<Client>()
    }

    fn server(&self) -> &Server {
        self.server_app.world.resource::<Server>()
    }

    pub fn init(&mut self) {
        self.client_apps.values_mut().for_each(|client_app| {
            client_app.world.resource_mut::<Client>().connect();
        });

        // Advance the world to let the connection process complete
        for _ in 0..50 {
            self.frame_step();
        }
    }
}

impl Step for BevyStepper {
    // TODO: maybe for testing use a local io via channels?
    /// Advance the world by one frame duration
    fn frame_step(&mut self) {
        self.current_time += self.frame_duration;

        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app.update();
        // sleep a bit to make sure that local io receives the packets
        std::thread::sleep(SOCKET_WAIT);
        for client_app in self.client_apps.values_mut() {
            client_app.insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
            client_app.update();
        }
    }

    fn tick_step(&mut self) {
        self.current_time += self.tick_duration;
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app.update();
        // sleep a bit to make sure that local io receives the packets
        std::thread::sleep(SOCKET_WAIT);
        for client_app in self.client_apps.values_mut() {
            client_app.insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
            client_app.update();
        }
    }
}
