//! The server side of the example.
//! It is possible (and recommended) to run the server in headless mode (without any rendering plugins).
//!
//! The server will:
//! - spawn a new player entity for each client that connects
//! - read inputs from the clients and move the player entities accordingly
//!
//! Lightyear will handle the replication of entities automatically if you add a `Replicate` component to them.
use anyhow::Context;
use async_compat::Compat;
use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use bevy::utils::{Duration, HashSet};
use tokio::io::AsyncWriteExt;

use lightyear::prelude::server::*;
use lightyear::prelude::ClientId::Netcode;
use lightyear::prelude::*;

use crate::protocol::*;
use crate::shared;

pub struct ExampleServerPlugin {
    pub protocol_id: u64,
    pub private_key: Key,
    pub game_server_addr: SocketAddr,
    pub auth_backend_addr: SocketAddr,
}

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        let client_ids = Arc::new(RwLock::new(HashSet::default()));
        app.add_systems(Startup, (init, start_server));

        start_netcode_authentication_task(
            self.game_server_addr,
            self.auth_backend_addr,
            self.protocol_id,
            self.private_key,
            client_ids.clone(),
        );

        app.insert_resource(ClientIds(client_ids));
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

/// This resource will track the list of Netcode client-ids currently in use, so that
/// we don't have multiple clients with the same id
#[derive(Resource)]
struct ClientIds(Arc<RwLock<HashSet<u64>>>);

/// Update the list of connected client ids when a client connects or disconnects
fn handle_connect_events(
    client_ids: Res<ClientIds>,
    mut connect_events: EventReader<ConnectEvent>,
    mut disconnect_events: EventReader<DisconnectEvent>,
) {
    for event in connect_events.read() {
        if let Netcode(client_id) = event.client_id {
            client_ids.0.write().unwrap().insert(client_id);
        }
    }
    for event in disconnect_events.read() {
        if let Netcode(client_id) = event.client_id {
            client_ids.0.write().unwrap().remove(&client_id);
        }
    }
}

/// Start a detached task that listens for incoming TCP connections and sends `ConnectToken`s to clients
fn start_netcode_authentication_task(
    game_server_addr: SocketAddr,
    auth_backend_addr: SocketAddr,
    protocol_id: u64,
    private_key: Key,
    client_ids: Arc<RwLock<HashSet<u64>>>,
) {
    IoTaskPool::get()
        .spawn(Compat::new(async move {
            info!(
                "Listening for ConnectToken requests on {}",
                auth_backend_addr
            );
            let listener = tokio::net::TcpListener::bind(auth_backend_addr)
                .await
                .unwrap();
            loop {
                // received a new connection
                let (mut stream, _) = listener.accept().await.unwrap();

                // assign a new client_id
                let client_id = loop {
                    let client_id = rand::random();
                    if !client_ids.read().unwrap().contains(&client_id) {
                        break client_id;
                    }
                };

                let token =
                    ConnectToken::build(game_server_addr, protocol_id, client_id, private_key)
                        .generate()
                        .expect("Failed to generate token");

                let serialized_token = token.try_into_bytes().expect("Failed to serialize token");
                trace!(
                    "Sending token {:?} to client {}. Token len: {}",
                    serialized_token,
                    client_id,
                    serialized_token.len()
                );
                stream
                    .write_all(&serialized_token)
                    .await
                    .expect("Failed to send token to client");
            }
        }))
        .detach();
}
