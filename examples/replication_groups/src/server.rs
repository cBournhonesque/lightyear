use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour, shared_tail_behaviour};
use crate::{shared, Transports, KEY, PROTOCOL_ID};
use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::utils::Duration;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

// Plugin group to add all server-related plugins
pub struct ServerPluginGroup {
    pub(crate) lightyear: ServerPlugin<MyProtocol>,
}

impl ServerPluginGroup {
    pub(crate) async fn new(port: u16, transport: Transports) -> ServerPluginGroup {
        // Step 1: create the io (transport + link conditioner)
        let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port);
        let transport_config = match transport {
            Transports::Udp => TransportConfig::UdpSocket(server_addr),
            // if using webtransport, we load the certificate keys
            Transports::WebTransport => {
                let certificate =
                    Certificate::load("../certificates/cert.pem", "../certificates/key.pem")
                        .await
                        .unwrap();
                let digest = certificate.hashes()[0].fmt_as_dotted_hex();
                dbg!(
                    "Generated self-signed certificate with digest: {:?}",
                    digest
                );
                TransportConfig::WebTransportServer {
                    server_addr,
                    certificate,
                }
            }
        };
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(200),
            incoming_jitter: Duration::from_millis(40),
            incoming_loss: 0.05,
        };
        let io = Io::from_config(
            IoConfig::from_transport(transport_config).with_conditioner(link_conditioner),
        );

        // Step 2: define the server configuration
        let config = ServerConfig {
            shared: shared_config().clone(),
            net: NetConfig::Netcode {
                config: NetcodeConfig::default()
                    .with_protocol_id(PROTOCOL_ID)
                    .with_key(KEY),
            },
            ..default()
        };

        // Step 3: create the plugin
        let plugin_config = PluginConfig::new(config, io, protocol());
        ServerPluginGroup {
            lightyear: ServerPlugin::new(plugin_config),
        }
    }
}

impl PluginGroup for ServerPluginGroup {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(self.lightyear)
            .add(ExampleServerPlugin)
            .add(shared::SharedPlugin)
    }
}

// Plugin for server-specific logic
pub struct ExampleServerPlugin;

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Global>();
        app.add_systems(Startup, init);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(
            FixedUpdate,
            (movement, shared_tail_behaviour)
                .chain()
                .in_set(FixedUpdateSet::Main),
        );
        app.add_systems(Update, handle_connections);
        // app.add_systems(Update, debug_inputs);
    }
}

#[derive(Resource, Default)]
pub(crate) struct Global {
    pub client_id_to_entity_id: HashMap<ClientId, (Entity, Entity)>,
}

pub(crate) fn init(mut commands: Commands) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Server",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
}

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    mut connections: EventReader<ConnectEvent>,
    mut disconnections: EventReader<DisconnectEvent>,
    mut global: ResMut<Global>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.context();
        // Generate pseudo random color from client id.
        let h = (((client_id.wrapping_mul(30)) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let player_position = Vec2::ZERO;
        let player_entity = commands
            .spawn(PlayerBundle::new(
                *client_id,
                player_position,
                Color::hsl(h, s, l),
            ))
            .id();
        let tail_length = 300.0;
        let tail_entity = commands
            .spawn(TailBundle::new(
                *client_id,
                player_entity,
                player_position,
                tail_length,
            ))
            .id();
        // Add a mapping from client id to entity id
        global
            .client_id_to_entity_id
            .insert(*client_id, (player_entity, tail_entity));
    }
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        if let Some((player_entity, tail_entity)) = global.client_id_to_entity_id.remove(client_id)
        {
            commands.entity(player_entity).despawn();
            commands.entity(tail_entity).despawn();
        }
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<&mut PlayerPosition>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    global: Res<Global>,
    tick_manager: Res<TickManager>,
) {
    for input in input_reader.read() {
        let client_id = input.context();
        if let Some(input) = input.input() {
            debug!(
                "Receiving input: {:?} from client: {:?} on tick: {:?}",
                input,
                client_id,
                tick_manager.tick()
            );
            if let Some((player_entity, _)) = global.client_id_to_entity_id.get(client_id) {
                if let Ok(mut position) = position_query.get_mut(*player_entity) {
                    shared_movement_behaviour(&mut position, input);
                }
            }
        }
    }
}

// pub(crate) fn debug_inputs(server: Res<Server>) {
//     info!(tick = ?server.tick(), inputs = ?server.get_input_buffer(1), "debug");
// }
