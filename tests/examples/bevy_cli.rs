//! Simple demo where the user can use the cli to spawn a client or a server process
//! Run with
//! - `cargo run --example bevy_cli server`
//! - `cargo run --example bevy_cli client`
use std::net::{Ipv4Addr, SocketAddr};
use std::str::FromStr;

use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::DefaultPlugins;
use clap::Parser;
use serde::{Deserialize, Serialize};
use tracing::Level;

use lightyear_client::{Authentication, Client, ClientConfig};
use lightyear_server::{NetcodeConfig, Server, ServerConfig};
use lightyear_shared::channel::channel::ReliableSettings;
use lightyear_shared::netcode::{ClientId, Key};
use lightyear_shared::plugin::events::MessageEvent;
use lightyear_shared::replication::Replicate;
use lightyear_shared::{
    component_protocol, message_protocol, protocolize, Channel, ChannelDirection, ChannelMode,
    ChannelSettings, ConnectEvent, ConnectionEvents, DisconnectEvent, EntitySpawnEvent, IoConfig,
    Message, Protocol, SharedConfig, TransportConfig, UdpSocket,
};

fn main() {
    let cli = Cli::parse();
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(LogPlugin {
        level: Level::DEBUG,
        filter: "wgpu=error,bevy_render=warn,naga=error,bevy_app=info".to_string(),
    }));
    setup(&mut app, cli);

    app.run();
}

const PORT: u16 = 5000;
const PROTOCOL_ID: u64 = 0;

const KEY: Key = [0; 32];

#[derive(Parser, PartialEq, Debug)]
enum Cli {
    SinglePlayer,
    Server {
        #[arg(short, long, default_value_t = PORT)]
        port: u16,
    },
    Client {
        #[arg(short, long, default_value_t = ClientId::default())]
        client_id: ClientId,

        #[arg(short, long, default_value_t = PORT)]
        server_port: u16,
    },
}

fn server_init(mut commands: Commands) {
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

fn client_init(mut commands: Commands, mut client: ResMut<Client<MyProtocol>>) {
    commands.spawn(Camera2dBundle::default());
    commands.spawn(TextBundle::from_section(
        "Client",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
    client.connect();
}

fn setup(app: &mut App, cli: Cli) {
    match cli {
        Cli::SinglePlayer => {}
        Cli::Server { port } => {
            let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
            let netcode_config = NetcodeConfig::default()
                .with_protocol_id(PROTOCOL_ID)
                .with_key(KEY);
            let config = ServerConfig {
                netcode: netcode_config,
                io: IoConfig::from_transport(TransportConfig::UdpSocket(server_addr)),
            };
            let plugin_config = lightyear_server::PluginConfig::new(config, protocol());
            app.add_plugins(lightyear_server::Plugin::new(plugin_config));
            app.add_systems(Startup, server_init);
            app.add_systems(
                Update,
                (
                    server_handle_connections,
                    input_system,
                    draw_boxes_system,
                    send_message_system,
                ),
            );
        }
        Cli::Client {
            client_id,
            server_port,
        } => {
            let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), server_port);
            let auth = Authentication::Manual {
                server_addr,
                client_id,
                private_key: KEY,
                protocol_id: PROTOCOL_ID,
            };
            let addr = SocketAddr::from_str("127.0.0.1:0").unwrap();
            let config = ClientConfig {
                shared: SharedConfig::default(),
                netcode: Default::default(),
                io: IoConfig::from_transport(TransportConfig::UdpSocket(addr)),
            };
            let plugin_config = lightyear_client::PluginConfig::new(config, protocol(), auth);
            app.add_plugins(lightyear_client::Plugin::new(plugin_config));
            app.add_systems(Startup, client_init);
            app.add_systems(
                Update,
                (
                    draw_boxes_system,
                    receive_message1_client,
                    receive_entity_spawn,
                ),
            );
        }
    }
}

fn protocol() -> MyProtocol {
    let mut p = MyProtocol::default();
    p.add_channel::<Channel1>(ChannelSettings {
        mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
        direction: ChannelDirection::Bidirectional,
    });
    p
}

#[derive(Bundle)]
struct PlayerBundle {
    id: PlayerId,
    position: PlayerPosition,
    color: PlayerColor,
    replicate: Replicate,
}

impl PlayerBundle {
    fn new(id: ClientId, position: Vec2, color: Color) -> Self {
        Self {
            id: PlayerId(id),
            position: PlayerPosition(position),
            color: PlayerColor(color),
            replicate: Replicate::default(),
        }
    }
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerId(ClientId);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Deref, DerefMut)]
pub struct PlayerPosition(Vec2);

#[derive(Component, Deserialize, Serialize, Clone)]
pub struct PlayerColor(Color);

#[derive(Channel)]
pub struct Channel1;

#[component_protocol(protocol = "MyProtocol")]
pub enum Components {
    PlayerId(PlayerId),
    PlayerPosition(PlayerPosition),
    PlayerColor(PlayerColor),
}

#[derive(Message, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message1(usize);

#[derive(Debug)]
#[message_protocol(protocol = "MyProtocol")]
pub enum Messages {
    Message1(Message1),
}

protocolize!(MyProtocol, Messages, Components);

/// Server connection system
fn server_handle_connections(
    // TODO: give type alias to ConnectionEvents<ClientId> ? (such as ServerConnectionEvents)?
    mut connections: EventReader<ConnectEvent<ClientId>>,
    mut disconnections: EventReader<DisconnectEvent<ClientId>>,
    mut commands: Commands,
) {
    for connection in connections.iter() {
        let client_id = connection.context();
        info!("New connection from client: {:?}", client_id);
        // Generate pseudo random color from client id.
        let r = ((client_id % 23) as f32) / 23.0;
        let g = ((client_id % 27) as f32) / 27.0;
        let b = ((client_id % 39) as f32) / 39.0;
        commands.spawn(PlayerBundle::new(
            *client_id,
            Vec2::ZERO,
            Color::rgb(r, g, b),
        ));
    }
    for disconnection in disconnections.iter() {
        info!("Client disconnected: {:?}", disconnection.context());
    }
}

/// Input system: for now, lets server move the Player entity, and the components
/// should get replicated
///
fn input_system(
    mut player: Query<(Entity, &mut PlayerPosition)>,
    input: Res<Input<KeyCode>>,
    mut commands: Commands,
) {
    if let Ok((entity, mut position)) = player.get_single_mut() {
        const MOVE_SPEED: f32 = 10.0;
        if input.pressed(KeyCode::Right) {
            position.x += MOVE_SPEED;
        }
        if input.pressed(KeyCode::Left) {
            position.x -= MOVE_SPEED;
        }
        if input.pressed(KeyCode::Up) {
            position.y += MOVE_SPEED;
        }
        if input.pressed(KeyCode::Down) {
            position.y -= MOVE_SPEED;
        }
        if input.pressed(KeyCode::D) {
            commands.entity(entity).despawn();
        }
    }
}

/// Send messages from server to clients
fn send_message_system(mut server: ResMut<Server<MyProtocol>>, input: Res<Input<KeyCode>>) {
    if input.pressed(KeyCode::M) {
        // TODO: add way to send message to all
        let message = Message1(5);
        info!("Send message: {:?}", message);
        server.broadcast_send::<Channel1, Message1>(Message1(5));
    }
}

/// System that draws the boxed of the player positions.
/// The components should be replicated from the server to the client
fn draw_boxes_system(mut gizmos: Gizmos, players: Query<(&PlayerPosition, &PlayerColor)>) {
    for (position, color) in &players {
        gizmos.rect(
            Vec3::new(position.x, position.y, 0.0),
            Quat::IDENTITY,
            Vec2::ONE * 50.0,
            color.0,
        );
    }
}

/// System to receive messages on the client
fn receive_message1_client(mut reader: EventReader<MessageEvent<Message1>>) {
    for event in reader.iter() {
        info!("Received message: {:?}", event.message());
    }
}

fn receive_entity_spawn(mut reader: EventReader<EntitySpawnEvent>) {
    for event in reader.iter() {
        info!("Received entity spawn: {:?}", event.entity());
    }
}
