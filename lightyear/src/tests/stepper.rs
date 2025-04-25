use crate::client::networking::ClientCommandsExt;
use crate::connection::netcode::generate_key;
use crate::prelude::client::{
    Authentication, ClientConfig, ClientTransport, NetConfig, NetworkingState,
};
use crate::prelude::server::{NetcodeConfig, ServerCommandsExt, ServerConfig, ServerTransport};
use crate::prelude::*;
use crate::shared::time_manager::WrappedTime;
use crate::tests::protocol::*;
use crate::transport::LOCAL_SOCKET;
#[cfg(not(feature = "std"))]
use alloc::vec;
use bevy::input::InputPlugin;
use bevy::prelude::{default, App, Mut, PluginGroup, Real, State, Time, World};
use bevy::state::app::StatesPlugin;
use bevy::time::TimeUpdateStrategy;
use bevy::MinimalPlugins;
use core::time::Duration;

pub const TEST_CLIENT_ID: u64 = 111;

pub struct BevyStepper {
    pub client_app: App,
    pub server_app: App,
    pub frame_duration: Duration,
    /// fixed timestep duration
    pub tick_duration: Duration,
    pub current_time: bevy::platform::time::Instant,
}

impl Default for BevyStepper {
    fn default() -> Self {
        let mut stepper = Self::default_no_init();
        stepper.init();
        stepper
    }
}

// Do not forget to use --features mock_time when using the LinkConditioner
impl BevyStepper {
    pub fn new(
        shared_config: SharedConfig,
        mut client_config: ClientConfig,
        frame_duration: Duration,
    ) -> Self {
        // tracing_subscriber::FmtSubscriber::builder()
        //     .with_max_level(tracing::Level::INFO)
        //     .init();

        // Use local channels instead of UDP for testing
        let addr = LOCAL_SOCKET;
        // channels to receive a message from/to server
        let (from_server_send, from_server_recv) = crossbeam_channel::unbounded();
        let (to_server_send, to_server_recv) = crossbeam_channel::unbounded();
        let mut client_io = client::IoConfig::from_transport(ClientTransport::LocalChannel {
            send: to_server_send,
            recv: from_server_recv,
        });

        let mut server_io = server::IoConfig::from_transport(ServerTransport::Channels {
            channels: vec![(addr, to_server_recv, from_server_send)],
        });

        let NetConfig::Netcode { io, .. } = client_config.net else {
            panic!("Only Netcode transport is supported in tests");
        };
        if let Some(conditioner) = io.conditioner {
            server_io = server_io.with_conditioner(conditioner.clone());
            client_io = client_io.with_conditioner(conditioner.clone());
        }

        // Shared config
        let protocol_id = 0;
        let private_key = generate_key();

        // Setup server
        let mut server_app = App::new();
        server_app.add_plugins((MinimalPlugins, StatesPlugin));
        let net_config = server::NetConfig::Netcode {
            config: NetcodeConfig::default()
                .with_protocol_id(protocol_id)
                .with_key(private_key),
            io: server_io,
        };
        let config = ServerConfig {
            shared: shared_config,
            net: vec![net_config],
            ping: PingConfig {
                // send pings every tick, so that the acks are received every frame
                ping_interval: Duration::default(),
                ..default()
            },
            ..default()
        };
        let plugin = server::ServerPlugins::new(config);
        server_app.add_plugins((plugin, ProtocolPlugin));

        // Setup client
        let mut client_app = App::new();
        client_app.add_plugins((MinimalPlugins, StatesPlugin));
        let net_config = client::NetConfig::Netcode {
            auth: Authentication::Manual {
                server_addr: addr,
                protocol_id,
                private_key,
                client_id: TEST_CLIENT_ID,
            },
            config: Default::default(),
            io: client_io,
        };

        client_config.shared = shared_config;
        client_config.ping = PingConfig {
            // send pings every tick, so that the acks are received every frame
            ping_interval: Duration::default(),
            ..default()
        };
        client_config.net = net_config;

        let plugin = client::ClientPlugins::new(client_config);
        client_app.add_plugins((plugin, ProtocolPlugin));

        #[cfg(feature = "leafwing")]
        {
            client_app.add_plugins(LeafwingInputPlugin::<LeafwingInput1>::default());
            client_app.add_plugins(LeafwingInputPlugin::<LeafwingInput2>::default());
            server_app.add_plugins(LeafwingInputPlugin::<LeafwingInput1>::default());
            server_app.add_plugins(LeafwingInputPlugin::<LeafwingInput2>::default());
            client_app.add_plugins(InputPlugin);
        }

        // Initialize Real time (needed only for the first TimeSystem run)
        let now = bevy::platform::time::Instant::now();
        client_app
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);
        server_app
            .world_mut()
            .get_resource_mut::<Time<Real>>()
            .unwrap()
            .update_with_instant(now);

        Self {
            client_app,
            server_app,
            frame_duration,
            tick_duration: shared_config.tick.tick_duration,
            current_time: now,
        }
    }

    pub(crate) fn default_no_init() -> Self {
        let frame_duration = Duration::from_millis(10);
        let tick_duration = Duration::from_millis(10);
        let shared_config = SharedConfig {
            tick: TickConfig::new(tick_duration),
            ..Default::default()
        };
        let client_config = ClientConfig::default();

        let mut stepper = Self::new(shared_config, client_config, frame_duration);
        stepper.build();
        stepper
    }

    pub(crate) fn interpolation_tick(&mut self) -> Tick {
        self.client_app.world_mut().resource_scope(
            |world: &mut World, manager: Mut<client::ConnectionManager>| {
                manager
                    .sync_manager
                    .interpolation_tick(world.resource::<TickManager>())
            },
        )
    }

    pub(crate) fn set_client_tick(&mut self, tick: Tick) {
        let new_time = WrappedTime::from_duration(self.tick_duration * (tick.0 as u32));

        self.client_app
            .world_mut()
            .resource_mut::<TimeManager>()
            .set_current_time(new_time);
        self.client_app
            .world_mut()
            .resource_mut::<TickManager>()
            .set_tick_to(tick);
    }

    pub(crate) fn set_server_tick(&mut self, tick: Tick) {
        let new_time = WrappedTime::from_duration(self.tick_duration * (tick.0 as u32));

        self.server_app
            .world_mut()
            .resource_mut::<TimeManager>()
            .set_current_time(new_time);
        self.server_app
            .world_mut()
            .resource_mut::<TickManager>()
            .set_tick_to(tick);
    }

    pub(crate) fn client_tick(&self) -> Tick {
        self.client_app.world().resource::<TickManager>().tick()
    }
    pub(crate) fn server_tick(&self) -> Tick {
        self.server_app.world().resource::<TickManager>().tick()
    }

    pub(crate) fn build(&mut self) {
        self.client_app.finish();
        self.client_app.cleanup();
        self.server_app.finish();
        self.server_app.cleanup();
    }
    pub(crate) fn init(&mut self) {
        self.server_app.world_mut().start_server();
        self.client_app.world_mut().connect_client();
        self.wait_for_connection();
        self.wait_for_sync();
    }

    // Advance the world until client is connected
    pub(crate) fn wait_for_connection(&mut self) {
        for _ in 0..100 {
            if matches!(
                self.client_app
                    .world()
                    .resource::<State<NetworkingState>>()
                    .get(),
                NetworkingState::Connected
            ) {
                break;
            }
            self.frame_step();
        }
    }

    // Advance the world until the client is synced
    pub(crate) fn wait_for_sync(&mut self) {
        for _ in 0..100 {
            if self
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .is_synced()
            {
                break;
            }
            self.frame_step();
        }
    }

    pub(crate) fn start(&mut self) {
        self.server_app.world_mut().start_server();
        self.client_app.world_mut().connect_client();

        // Advance the world to let the connection process complete
        for _ in 0..100 {
            if self
                .client_app
                .world()
                .resource::<client::ConnectionManager>()
                .is_synced()
            {
                break;
            }
            self.frame_step();
        }
    }

    pub(crate) fn stop(&mut self) {
        self.server_app.world_mut().stop_server();
        self.client_app.world_mut().disconnect_client();

        // Advance the world to let the disconnection process complete
        for _ in 0..100 {
            self.frame_step();
        }
    }

    pub(crate) fn advance_time(&mut self, duration: Duration) {
        self.current_time += duration;
        self.client_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        self.server_app
            .insert_resource(TimeUpdateStrategy::ManualInstant(self.current_time));
        mock_instant::global::MockClock::advance(duration);
    }

    pub(crate) fn flush(&mut self) {
        self.client_app.world_mut().flush();
        self.server_app.world_mut().flush();
    }

    /// Advance the world by one frame duration
    pub(crate) fn frame_step(&mut self) {
        self.advance_time(self.frame_duration);
        self.client_app.update();
        self.server_app.update();
    }

    pub(crate) fn tick_step(&mut self) {
        self.advance_time(self.tick_duration);
        self.client_app.update();
        self.server_app.update();
    }
}
