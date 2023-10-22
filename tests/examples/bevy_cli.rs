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
use lightyear_server::{NetcodeConfig, ServerConfig};
use lightyear_shared::channel::channel::ReliableSettings;
use lightyear_shared::netcode::{ClientId, Key};
use lightyear_shared::replication::Replicate;
use lightyear_shared::{
    component_protocol, message_protocol, protocolize, Channel, ChannelDirection, ChannelMode,
    ChannelSettings, IoConfig, Protocol, SharedConfig,
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
    commands.spawn(PlayerBundle::new(0, Vec2::ZERO, Color::GREEN));
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
                io: IoConfig::UdpSocket(server_addr),
            };
            let plugin_config = lightyear_server::PluginConfig::new(config, protocol());
            app.add_plugins(lightyear_server::Plugin::new(plugin_config));
            app.add_systems(Startup, server_init);
            app.add_systems(Update, (input_system, draw_boxes_system));
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
                io: IoConfig::UdpSocket(addr),
            };
            let plugin_config = lightyear_client::PluginConfig::new(config, protocol(), auth);
            app.add_plugins(lightyear_client::Plugin::new(plugin_config));
            app.add_systems(Startup, client_init);
            app.add_systems(Update, draw_boxes_system_client);
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

#[message_protocol]
pub enum Messages {}

protocolize!(MyProtocol, Messages, Components);

/// Input system: for now, lets server move the Player entity, and the components
/// should get replicated
///
fn input_system(mut player: Query<&mut PlayerPosition>, input: Res<Input<KeyCode>>) {
    if let Ok(mut position) = player.get_single_mut() {
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
    }
}

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

fn draw_boxes_system_client(mut gizmos: Gizmos, players: Query<&PlayerPosition>) {
    for (position) in &players {
        gizmos.rect(
            Vec3::new(position.x, position.y, 0.0),
            Quat::IDENTITY,
            Vec2::ONE * 50.0,
            Color::GREEN,
        );
    }
}
