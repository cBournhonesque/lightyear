use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour, shared_tail_behaviour};
use crate::{shared, Transports, KEY, PROTOCOL_ID};
use bevy::prelude::*;
use lightyear::inputs::input_buffer::InputBuffer;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

pub struct MyServerPlugin {
    pub(crate) port: u16,
    pub(crate) transport: Transports,
}

impl Plugin for MyServerPlugin {
    fn build(&self, app: &mut App) {
        let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), self.port);
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(PROTOCOL_ID)
            .with_key(KEY);
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(200),
            incoming_jitter: Duration::from_millis(40),
            incoming_loss: 0.05,
        };
        let transport = match self.transport {
            Transports::Udp => TransportConfig::UdpSocket(server_addr),
            Transports::Webtransport => TransportConfig::WebTransportServer {
                server_addr,
                certificate: Certificate::self_signed(&["localhost"]),
            },
        };
        let io = Io::from_config(
            &IoConfig::from_transport(transport).with_conditioner(link_conditioner),
        );
        let config = ServerConfig {
            shared: shared_config().clone(),
            netcode: netcode_config,
            ping: PingConfig::default(),
        };
        let plugin_config = PluginConfig::new(config, io, protocol());
        app.add_plugins(server::ServerPlugin::new(plugin_config));
        app.add_plugins(shared::SharedPlugin);
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
    pub client_id_to_entity_id: HashMap<ClientId, Entity>,
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
        let h = (((client_id * 30) % 360) as f32) / 360.0;
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
        let tail_entity = commands.spawn(TailBundle::new(
            *client_id,
            player_entity,
            player_position,
            tail_length,
        ));
        // Add a mapping from client id to entity id
        global
            .client_id_to_entity_id
            .insert(*client_id, player_entity);
    }
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        if let Some(entity) = global.client_id_to_entity_id.remove(client_id) {
            // TODO: also despawn tail, maybe by emitting an event?
            commands.entity(entity).despawn();
        }
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<&mut PlayerPosition>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    global: Res<Global>,
    server: Res<Server>,
) {
    for input in input_reader.read() {
        let client_id = input.context();
        if let Some(input) = input.input() {
            debug!(
                "Receiving input: {:?} from client: {:?} on tick: {:?}",
                input,
                client_id,
                server.tick()
            );
            if let Some(player_entity) = global.client_id_to_entity_id.get(client_id) {
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
