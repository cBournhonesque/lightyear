use crate::protocol::*;
use crate::shared::{shared_config, shared_movement_behaviour};
use crate::{shared, KEY, PROTOCOL_ID};
use bevy::prelude::*;
use lightyear_shared::prelude::*;
use lightyear_shared::server::config::PacketConfig;
use lightyear_shared::server::events::{ConnectEvent, DisconnectEvent, InputEvent};
use lightyear_shared::server::{NetcodeConfig, PingConfig, Server, ServerConfig};
use lightyear_shared::shared::sets::FixedUpdateSet;
use lightyear_shared::{IoConfig, LinkConditionerConfig, TransportConfig};
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

pub struct ServerPlugin {
    pub(crate) port: u16,
}

impl Plugin for ServerPlugin {
    fn build(&self, app: &mut App) {
        let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), self.port);
        let netcode_config = NetcodeConfig::default()
            .with_protocol_id(PROTOCOL_ID)
            .with_key(KEY);
        let link_conditioner = LinkConditionerConfig {
            incoming_latency: Duration::from_millis(100),
            incoming_jitter: Duration::from_millis(0),
            incoming_loss: 0.00,
        };
        let config = ServerConfig {
            shared: shared_config().clone(),
            netcode: netcode_config,
            io: IoConfig::from_transport(TransportConfig::UdpSocket(server_addr))
                .with_conditioner(link_conditioner),
            ping: PingConfig::default(),
        };
        let plugin_config =
            lightyear_shared::server::PluginConfig::new(config, MyProtocol::default());
        app.add_plugins(lightyear_shared::server::Plugin::new(plugin_config));
        app.add_plugins(shared::SharedPlugin);
        app.init_resource::<Global>();
        app.add_systems(Startup, init);
        // the physics/FixedUpdates systems that consume inputs should be run in this set
        app.add_systems(FixedUpdate, movement.in_set(FixedUpdateSet::Main));
        app.add_systems(Update, (handle_connections, send_message, log_confirmed));
    }
}

#[derive(Resource, Default)]
struct Global {
    pub client_id_to_entity_id: HashMap<ClientId, Entity>,
}

pub(crate) fn init(mut commands: Commands, mut server: ResMut<Server<MyProtocol>>) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Server",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
    // server.set_base_relative_speed(0.001);
}

/// Server connection system, create a player upon connection
pub(crate) fn handle_connections(
    // TODO: give type alias to ConnectionEvents<ClientId> ? (such as ServerConnectionEvents)?
    mut connections: EventReader<ConnectEvent>,
    mut disconnections: EventReader<DisconnectEvent>,
    mut global: ResMut<Global>,
    mut commands: Commands,
) {
    for connection in connections.read() {
        let client_id = connection.context();
        info!("New connection from client: {:?}", client_id);
        // Generate pseudo random color from client id.
        let h = (((client_id * 30) % 360) as f32) / 360.0;
        let s = 0.8;
        let l = 0.5;
        let entity = commands.spawn(PlayerBundle::new(
            *client_id,
            Vec2::ZERO,
            Color::hsl(h, s, l),
        ));
        // Add a mapping from client id to entity id
        global
            .client_id_to_entity_id
            .insert(*client_id, entity.id());
    }
    for disconnection in disconnections.read() {
        let client_id = disconnection.context();
        info!("Client {:?} disconnected", client_id);
        if let Some(entity) = global.client_id_to_entity_id.remove(client_id) {
            commands.entity(entity).despawn();
        }
    }
}

pub(crate) fn log_confirmed(server: Res<Server<MyProtocol>>, confirmed: Query<(&PlayerPosition)>) {
    for pos in confirmed.iter() {
        // info!(
        //     "interpolated pos: {:?}, server tick: {:?}",
        //     pos,
        //     server.tick()
        // );
    }
}

/// Read client inputs and move players
pub(crate) fn movement(
    mut position_query: Query<&mut PlayerPosition>,
    mut input_reader: EventReader<InputEvent<Inputs>>,
    global: Res<Global>,
    server: Res<Server<MyProtocol>>,
) {
    for input in input_reader.read() {
        let client_id = input.context();
        if let Some(input) = input.input() {
            info!(
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

/// Send messages from server to clients
pub(crate) fn send_message(mut server: ResMut<Server<MyProtocol>>, input: Res<Input<KeyCode>>) {
    if input.pressed(KeyCode::M) {
        // TODO: add way to send message to all
        let message = Message1(5);
        info!("Send message: {:?}", message);
        server.broadcast_send::<Channel1, Message1>(Message1(5));
    }
}
