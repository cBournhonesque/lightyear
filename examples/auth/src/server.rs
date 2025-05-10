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

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::tasks::IoTaskPool;
use core::time::Duration;
use lightyear::netcode::ConnectToken;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use lightyear_examples_common::shared::{SERVER_ADDR, SERVER_PORT, SHARED_SETTINGS};
use tokio::io::AsyncWriteExt;

use crate::shared;

pub struct ExampleServerPlugin {
    pub game_server_addr: SocketAddr,
    pub auth_backend_addr: SocketAddr,
}

impl Plugin for ExampleServerPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(handle_disconnect_event);
        app.add_observer(handle_connect_event);

        let client_ids = Arc::new(RwLock::new(HashSet::default()));
        start_netcode_authentication_task(
            self.game_server_addr,
            self.auth_backend_addr,
            client_ids.clone(),
        );
        app.insert_resource(ClientIds(client_ids));
    }
}


/// This resource will track the list of Netcode client-ids currently in use, so that
/// we don't have multiple clients with the same id
#[derive(Resource, Default)]
struct ClientIds(Arc<RwLock<HashSet<u64>>>);

/// Update the list of connected client ids when a client disconnects
fn handle_disconnect_event(
    trigger: Trigger<OnAdd, Disconnected>,
    query: Query<&Connected, With<ClientOf>>,
    client_ids: Res<ClientIds>) {
    let Ok(connected) = query.get(trigger.target()) else {
        return
    };
    if let PeerId::Netcode(client_id) = connected.remote_peer_id {
        info!("Client disconnected: {}. Removing from ClientIds.", client_id);
        client_ids.0.write().unwrap().remove(&client_id);
    }
}

/// Update the list of connected client ids when a client connects
fn handle_connect_event(
    trigger: Trigger<OnAdd, Connected>,
    query: Query<&Connected, With<ClientOf>>,
    client_ids: Res<ClientIds>
) {

    let Ok(connected) = query.get(trigger.target()) else {
        return
    };
    if let PeerId::Netcode(client_id) = connected.remote_peer_id {
        info!("Client connected: {}. Adding to ClientIds.", client_id);
        client_ids.0.write().unwrap().insert(client_id);
    }
}


/// Start a detached task that listens for incoming TCP connections and sends `ConnectToken`s to clients
fn start_netcode_authentication_task(
    game_server_addr: SocketAddr,
    auth_backend_addr: SocketAddr,
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
                    ConnectToken::build(game_server_addr, SHARED_SETTINGS.protocol_id, client_id, SHARED_SETTINGS.private_key)
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
