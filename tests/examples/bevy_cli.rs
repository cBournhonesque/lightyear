use std::net::{Ipv4Addr, SocketAddr};

use bevy::prelude::*;
use bevy::DefaultPlugins;
use clap::Parser;
use tracing_subscriber::fmt::format::FmtSpan;

use lightyear_client::Authentication;
use lightyear_shared::netcode::{ClientId, Key};

/// Test where the user can use the cli to spawn a client or a server process

fn main() {
    tracing_subscriber::FmtSubscriber::builder()
        .with_span_events(FmtSpan::ENTER)
        .with_max_level(tracing::Level::DEBUG)
        .init();

    let cli = Cli::parse();
    dbg!(&cli);
    debug!(?cli);
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);

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
    commands.spawn(TextBundle::from_section(
        "Server",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
}

fn client_init(mut commands: Commands) {
    commands.spawn(TextBundle::from_section(
        "Client",
        TextStyle {
            font_size: 30.0,
            color: Color::WHITE,
            ..default()
        },
    ));
}

fn setup(app: &mut App, cli: Cli) {
    match cli {
        Cli::SinglePlayer => {}
        Cli::Server { port } => {
            let server_addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
            lightyear_tests::server::bevy_setup(app, server_addr, PROTOCOL_ID, KEY);
            app.add_systems(Startup, server_init);
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
            lightyear_tests::client::bevy_setup(app, auth);
            app.add_systems(Startup, client_init);
        }
    }
}
