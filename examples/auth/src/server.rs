//! The server side of the example.
//! It is possible (and recommended) to run the server in headless mode (without any rendering plugins).
//!
//! The server will:
//! - spawn a new player entity for each client that connects
//! - read inputs from the clients and move the player entities accordingly
//!
//! Lightyear will handle the replication of entities automatically if you add a `Replicate` component to them.
use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};

use bevy::app::PluginGroupBuilder;
use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use bevy::utils::Duration;

pub use lightyear::prelude::server::*;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared::shared_config;
use crate::{shared, ServerTransports, SharedSettings};

pub struct ExampleServerPlugin {
    pub auth_backend_address: SocketAddr,
}

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (init, start_server));
    }
}

/// Start the server
fn start_server(mut commands: Commands) {
    commands.start_server();
}

/// Add some debugging text to the screen
fn init(mut commands: Commands) {
    commands.spawn(
        TextBundle::from_section(
            "Server",
            TextStyle {
                font_size: 30.0,
                color: Color::WHITE,
                ..default()
            },
        )
        .with_style(Style {
            align_self: AlignSelf::End,
            ..default()
        }),
    );
}

fn start_netcode_authentication_task(auth_backend_address: SocketAddr) {
    IoTaskPool::get().spawn(async {
        let mut listener = tokio::net::TcpListener::bind(auth_backend_address)
            .await
            .unwrap();
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = [0; CONNECT_TOKEN_BYTES];
            stream.read_exact(&mut buffer).await.unwrap();
            println!("Received connect token: {:?}", buffer);
        }
    });
}

fn handle_incoming_connection(
    game_server_addr: SocketAddr,
    client_id_to_entity_id: SharedClientIdToEntityIdHashMap,
    mut stream: tokio::net::TcpStream,
) -> anyhow::Result<()> {
    let client_id_to_entity_id = client_id_to_entity_id.lock().unwrap();
    let client_id = loop {
        let client_id = rand::random();
        if !client_id_to_entity_id.contains_key(&ClientId::Local(client_id)) {
            break client_id;
        }
    };

    let token = ConnectToken::build(
        game_server_addr,
        DEFAULT_PROTOCOL_ID,
        client_id,
        DEFAULT_PRIVATE_KEY,
    )
    .generate()
    .context("Failed to generate token")?;

    let serialized_token = token
        .try_into_bytes()
        .context("Failed to serialize token")?;

    stream
        .write_all(&serialized_token)
        .context("Failed to send token to client")?;

    Ok(())
}
