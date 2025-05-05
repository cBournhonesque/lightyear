use crate::connection::id::ClientId;
use crate::connection::netcode::MAX_PACKET_SIZE;
use crate::connection::server::{
    ConnectionError, ConnectionRequestHandler, DefaultConnectionRequestHandler, NetServer,
};
use crate::packet::packet_builder::RecvPayload;
use crate::prelude::LinkConditionerConfig;
use crate::server::io::Io;
use bevy::platform::collections::HashMap;
use alloc::collections::VecDeque;
use alloc::sync::Arc;
#[cfg(not(feature = "std"))]
use alloc::{string::String, vec, vec::Vec};
use parking_lot::RwLock;
use std::net::{Ipv4Addr, SocketAddr};
use steamworks::networking_sockets::{ListenSocket, NetConnection};
use steamworks::networking_types::{ListenSocketEvent, NetConnectionEnd, SendFlags};
use steamworks::{ClientManager, ServerMode, SteamError};
use tracing::{error, info};

use super::steamworks_client::SteamworksClient;

#[derive(Debug, Clone)]
pub struct SteamConfig {
    pub app_id: u32,
    pub socket_config: SocketConfig,
    pub max_clients: usize,
    /// A closure that will be used to accept or reject incoming connections
    pub connection_request_handler: Arc<dyn ConnectionRequestHandler>,
    // pub mode: ServerMode,
    // TODO: name this protocol to match netcode?
    pub version: String,
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            // app id of the public Space Wars demo app
            app_id: 480,
            socket_config: Default::default(),
            max_clients: 16,
            connection_request_handler: Arc::new(DefaultConnectionRequestHandler),
            // mode: ServerMode::NoAuthentication,
            version: "1.0".to_string(),
        }
    }
}

/// Steam socket configuration for servers
#[derive(Debug, Clone)]
pub enum SocketConfig {
    /// This server accepts connections via IP address. Suitable for dedicated servers.
    Ip {
        server_ip: Ipv4Addr,
        game_port: u16,
        query_port: u16,
    },
    /// This server accepts Steam P2P connections. Suitable for peer-to-peer games.
    P2P { virtual_port: i32 },
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self::Ip {
            server_ip: Ipv4Addr::new(127, 0, 0, 1),
            game_port: 27015,
            query_port: 27016,
        }
    }
}

// TODO: enable p2p by replacing ServerManager with ClientManager?
pub struct Server {
    steamworks_client: Arc<RwLock<SteamworksClient>>,
    server: Option<steamworks::Server>,
    config: SteamConfig,
    listen_socket: Option<ListenSocket<ClientManager>>,
    connections: HashMap<ClientId, NetConnection<ClientManager>>,
    packet_queue: VecDeque<(RecvPayload, ClientId)>,
    new_connections: Vec<ClientId>,
    new_disconnections: Vec<ClientId>,
    conditioner: Option<LinkConditionerConfig>,
    client_errors: Vec<ConnectionError>,
}

impl Server {
    pub fn new(
        steamworks_client: Arc<RwLock<SteamworksClient>>,
        config: SteamConfig,
        conditioner: Option<LinkConditionerConfig>,
    ) -> Result<Self, ConnectionError> {
        let server = match &config.socket_config {
            SocketConfig::Ip {
                server_ip,
                game_port,
                query_port,
            } => {
                let (server, _) = steamworks::Server::init(
                    *server_ip,
                    *game_port,
                    *query_port,
                    ServerMode::NoAuthentication,
                    // config.mode.clone(),
                    &config.version.clone(),
                )?;
                Some(server)
            }
            SocketConfig::P2P { .. } => None,
        };
        Ok(Self {
            steamworks_client,
            server,
            config,
            listen_socket: None,
            connections: HashMap::default(),
            packet_queue: VecDeque::new(),
            new_connections: Vec::new(),
            new_disconnections: Vec::new(),
            conditioner,
            client_errors: vec![],
        })
    }
}

impl NetServer for Server {
    fn start(&mut self) -> Result<(), ConnectionError> {
        // TODO: using the NetworkingConfigEntry options seems to cause an issue. See: https://github.com/Noxime/steamworks-rs/issues/169
        // let options = get_networking_options(&self.conditioner);

        match self.config.socket_config {
            SocketConfig::Ip {
                server_ip,
                game_port,
                ..
            } => {
                let server_addr = SocketAddr::new(server_ip.into(), game_port);
                self.listen_socket = Some(
                    self.steamworks_client
                        .try_read()
                        .expect("could not get steamworks client")
                        .get_client()
                        .networking_sockets()
                        .create_listen_socket_ip(server_addr, vec![])?,
                );
                info!("Steam socket started on {:?}", server_addr);
            }
            SocketConfig::P2P { virtual_port } => {
                self.listen_socket = Some(
                    self.steamworks_client
                        .try_read()
                        .expect("could not get steamworks client")
                        .get_client()
                        .networking_sockets()
                        .create_listen_socket_p2p(virtual_port, vec![])?,
                );
                info!(
                    "Steam P2P socket started on virtual port: {:?}",
                    virtual_port
                );
            }
        };
        Ok(())
    }

    fn stop(&mut self) -> Result<(), ConnectionError> {
        self.listen_socket = None;
        for (client_id, connection) in self.connections.drain() {
            let _ = connection.close(NetConnectionEnd::AppGeneric, None, true);
            self.new_disconnections.push(client_id);
        }
        info!("Steam socket has been closed.");
        Ok(())
    }

    fn disconnect(&mut self, client_id: ClientId) -> Result<(), ConnectionError> {
        match client_id {
            ClientId::Steam(id) => {
                if let Some(connection) = self.connections.remove(&client_id) {
                    let _ = connection.close(NetConnectionEnd::AppGeneric, None, true);
                    self.new_disconnections.push(client_id);
                }
                Ok(())
            }
            _ => Err(ConnectionError::InvalidConnectionType),
        }
    }

    fn connected_client_ids(&self) -> Vec<ClientId> {
        self.connections.keys().cloned().collect()
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<Vec<ConnectionError>, ConnectionError> {
        self.steamworks_client
            .try_write()
            .expect("could not get steamworks client")
            .get_single()
            .run_callbacks();

        // reset connection events
        self.new_connections.clear();
        self.new_disconnections.clear();

        // process connection events
        let Some(listen_socket) = self.listen_socket.as_mut() else {
            return Err(SteamError::NoConnection.into());
        };
        while let Some(event) = listen_socket.try_receive_event() {
            match event {
                ListenSocketEvent::Connected(event) => {
                    if let Some(steam_id) = event.remote().steam_id() {
                        let client_id = ClientId::Steam(steam_id.raw());
                        info!("Client with id: {:?} connected!", client_id);
                        self.new_connections.push(client_id);
                        self.connections.insert(client_id, event.take_connection());
                    } else {
                        error!("Received connection attempt from invalid steam id");
                    }
                }
                ListenSocketEvent::Disconnected(event) => {
                    if let Some(steam_id) = event.remote().steam_id() {
                        let client_id = ClientId::Steam(steam_id.raw());
                        info!(
                            "Client with id: {:?} disconnected! Reason: {:?}",
                            client_id,
                            event.end_reason()
                        );
                        if let Some(connection) = self.connections.remove(&client_id) {
                            let _ = connection.close(NetConnectionEnd::AppGeneric, None, true);
                            self.new_disconnections.push(client_id);
                        }
                    } else {
                        error!("Received disconnection attempt from invalid steam id");
                    }
                }
                ListenSocketEvent::Connecting(event) => {
                    if self.connections.len() >= self.config.max_clients {
                        event.reject(NetConnectionEnd::AppGeneric, Some("Too many clients"));
                        continue;
                    }
                    let Some(steam_id) = event.remote().steam_id() else {
                        event.reject(NetConnectionEnd::AppGeneric, Some("Invalid steam id"));
                        continue;
                    };
                    info!("Client with id: {:?} requesting connection!", steam_id);
                    if let Some(denied_reason) = self
                        .config
                        .connection_request_handler
                        .handle_request(ClientId::Steam(steam_id.raw()))
                    {
                        event.reject(NetConnectionEnd::AppGeneric, Some("{denied_reason:?}"));
                        continue;
                    } else {
                        if let Err(e) = event.accept() {
                            error!("Failed to accept connection from {steam_id:?}: {e}");
                        }
                        info!("Accepted connection from client {:?}", steam_id);
                    }
                }
            }
        }

        // buffer incoming packets
        for (client_id, connection) in self.connections.iter_mut() {
            // TODO: avoid allocating messages into a separate buffer, instead provide our own buffer?
            for message in connection.receive_messages(MAX_PACKET_SIZE)? {
                // // get a buffer from the pool to avoid new allocations
                // let mut reader = self.buffer_pool.start_read(message.data());
                // let packet = Packet::decode(&mut reader).context("could not decode packet")?;
                // // return the buffer to the pool
                // self.buffer_pool.attach(reader);
                let payload = RecvPayload::copy_from_slice(message.data());
                self.packet_queue.push_back((payload, *client_id));
            }
            // TODO: is this necessary since I disabled nagle?
            connection.flush_messages()?
        }

        // send any keep-alives or connection-related packets
        Ok(self.client_errors.drain(..).collect())
    }

    fn recv(&mut self) -> Option<(RecvPayload, ClientId)> {
        self.packet_queue.pop_front()
    }

    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<(), ConnectionError> {
        let Some(connection) = self.connections.get_mut(&client_id) else {
            return Err(ConnectionError::ConnectionNotFound);
        };
        // TODO: compare this with self.listen_socket.send_messages()
        connection.send_message(buf, SendFlags::UNRELIABLE_NO_NAGLE)?;
        Ok(())
    }

    fn new_connections(&self) -> Vec<ClientId> {
        self.new_connections.clone()
    }

    fn new_disconnections(&self) -> Vec<ClientId> {
        self.new_disconnections.clone()
    }

    fn client_addr(&self, client_id: ClientId) -> Option<SocketAddr> {
        None
    }

    fn io(&self) -> Option<&Io> {
        None
    }

    fn io_mut(&mut self) -> Option<&mut Io> {
        None
    }
}
