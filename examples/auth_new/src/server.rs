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
use bevy::utils::HashSet; // Use bevy's HashSet
use core::time::Duration;
use tokio::io::AsyncWriteExt;

use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common_new::shared::{SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS}; // Import common settings

use crate::client::AuthSettings; // Assuming AuthSettings is defined in client.rs
use crate::protocol::*;
use crate::shared;

pub struct ExampleServerPlugin;
// { // Removed fields, use resources/constants
//     pub protocol_id: u64,
//     pub private_key: Key,
//     pub game_server_addr: SocketAddr,
//     pub auth_backend_addr: SocketAddr,
// }

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        // Initialize ClientIds resource and start the auth task at Startup
        app.add_systems(Startup, setup_auth_task);
        app.add_observer(handle_disconnect_event);
        app.add_observer(handle_connect_event);
    }
}

// // Removed: Server start is handled in main.rs
// fn start_server(mut commands: Commands) {
//     commands.start_server();
// }

/// This resource will track the list of Netcode client-ids currently in use, so that
/// we don't have multiple clients with the same id
#[derive(Resource, Default)]
struct ClientIds(Arc<RwLock<HashSet<u64>>>);

/// Update the list of connected client ids when a client disconnects
fn handle_disconnect_event(trigger: Trigger<DisconnectEvent>, client_ids: Res<ClientIds>) {
    // Use PeerId and check for Netcode variant
    if let PeerId::Netcode(client_id) = trigger.event().peer_id {
        info!("Client disconnected: {}. Removing from ClientIds.", client_id);
        client_ids.0.write().unwrap().remove(&client_id);
    }
}

/// Update the list of connected client ids when a client connects
fn handle_connect_event(trigger: Trigger<ConnectEvent>, client_ids: Res<ClientIds>) {
    // Use PeerId and check for Netcode variant
    if let PeerId::Netcode(client_id) = trigger.event().peer_id {
        info!("Client connected: {}. Adding to ClientIds.", client_id);
        client_ids.0.write().unwrap().insert(client_id);
    }
}

/// Startup system to initialize ClientIds resource and start the auth task
fn setup_auth_task(
    mut commands: Commands,
    auth_settings: Res<AuthSettings>, // Get auth settings
) {
    let client_ids_arc = Arc::new(RwLock::new(HashSet::default()));
    commands.insert_resource(ClientIds(client_ids_arc.clone()));

    // Use constants for game server address, protocol id, and private key
    let game_server_addr = SocketAddr::new(SERVER_ADDR, SERVER_PORT);
    let protocol_id = SHARED_SETTINGS.protocol_id;
    let private_key = SHARED_SETTINGS.private_key;

    start_netcode_authentication_task(
        game_server_addr,
        auth_settings.backend_addr, // Use address from resource
        protocol_id,
        private_key,
        client_ids_arc,
    );
}


/// Start a detached task that listens for incoming TCP connections and sends `ConnectToken`s to clients
// (Function signature remains the same, implementation uses the passed args)
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
