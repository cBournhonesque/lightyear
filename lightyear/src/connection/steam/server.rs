use crate::_reexport::{ReadBuffer, ReadWordBuffer};
use crate::connection::netcode::MAX_PACKET_SIZE;
use crate::connection::server::NetServer;
use crate::prelude::{ClientId, Io};
use anyhow::{Context, Result};
use bevy::utils::HashMap;
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr};
use steamworks::networking_sockets::{ListenSocket, NetConnection};
use steamworks::networking_types::{
    ListenSocketEvent, NetConnectionEnd, NetworkingConfigEntry, SendFlags,
};
use steamworks::{ClientManager, Manager, ServerManager, ServerMode, SteamError};
use tracing::error;

#[derive(Debug, Clone)]
pub struct SteamConfig {
    pub app_id: u32,
    pub server_ip: Ipv4Addr,
    pub game_port: u16,
    pub query_port: u16,
    pub max_clients: usize,
    // pub mode: ServerMode,
    // TODO: name this protocol to match netcode?
    pub version: String,
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            // app id of the public Space Wars demo app
            app_id: 480,
            server_ip: Ipv4Addr::new(127, 0, 0, 1),
            game_port: 27015,
            query_port: 27016,
            max_clients: 16,
            // mode: ServerMode::NoAuthentication,
            version: "1.0".to_string(),
        }
    }
}

// TODO: enable p2p by replacing ServerManager with ClientManager?
pub struct Server {
    // TODO: update to use ServerManager...
    client: steamworks::Client<ClientManager>,
    server: steamworks::Server,
    config: SteamConfig,
    listen_socket: ListenSocket<ServerManager>,
    connections: HashMap<ClientId, NetConnection<ServerManager>>,
    packet_queue: VecDeque<(ReadWordBuffer, ClientId)>,
    new_connections: Vec<ClientId>,
    new_disconnections: Vec<ClientId>,
}

impl Server {
    pub fn new(config: SteamConfig) -> Result<Self> {
        let (client, _) = steamworks::Client::init_app(config.app_id)
            .context("could not initialize steam client")?;
        let (mut server, _) = steamworks::Server::init(
            config.server_ip,
            config.game_port,
            config.query_port,
            ServerMode::NoAuthentication,
            // config.mode.clone(),
            &config.version.clone(),
        )
        .context("could not initialize steam server")?;

        Ok(Self {
            client,
            server,
            config,
            listen_socket,
            connections: HashMap::new(),
            packet_queue: VecDeque::new(),
            new_connections: Vec::new(),
            new_disconnections: Vec::new(),
        })
    }
}

impl NetServer for Server {
    fn start(&mut self) -> Result<()> {
        let options: Vec<NetworkingConfigEntry> = Vec::new();
        let server_addr = SocketAddr::new(self.config.server_ip.into(), self.config.game_port);
        let listen_socket = self
            .client
            .networking_sockets()
            .create_listen_socket_ip(server_addr, options)
            .context("could not create server listen socket")?;
        Ok(())
    }

    fn connected_client_ids(&self) -> Vec<ClientId> {
        self.connections.keys().cloned().collect()
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<()> {
        // reset connection events
        self.new_connections.clear();
        self.new_disconnections.clear();

        // process connection events
        while let Some(event) = self.listen_socket.try_receive_event() {
            match event {
                ListenSocketEvent::Connected(event) => {
                    if let Some(steam_id) = event.remote().steam_id() {
                        let client_id = steam_id.raw() as ClientId;
                        self.new_connections.push(client_id);
                        self.connections.insert(client_id, event.take_connection());
                    } else {
                        error!("Received connection attempt from invalid steam id");
                    }
                }
                ListenSocketEvent::Disconnected(event) => {
                    if let Some(steam_id) = event.remote().steam_id() {
                        let client_id = steam_id.raw() as ClientId;
                        self.new_disconnections.push(client_id);
                        self.connections.remove(&client_id);
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
                    // TODO: improve permission check
                    let permitted = true;
                    if permitted {
                        if let Err(e) = event.accept() {
                            error!("Failed to accept connection from {steam_id:?}: {e}");
                        }
                    } else {
                        event.reject(NetConnectionEnd::AppGeneric, Some("Not allowed"));
                        continue;
                    }
                }
            }
        }

        // buffer incoming packets
        for (client_id, connection) in self.connections.iter_mut() {
            // TODO: avoid allocating messages into a separate buffer, instead provide our own buffer?
            for message in connection
                .receive_messages(MAX_PACKET_SIZE)
                .context("Failed to receive messages")?
            {
                self.packet_queue
                    .push_back((ReadWordBuffer::start_read(message.data()), *client_id));
            }
            // TODO: is this necessary since I disabled nagle?
            connection
                .flush_messages()
                .context("Failed to flush messages")?;
        }

        // send any keep-alives or connection-related packets
        Ok(())
    }

    fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)> {
        self.packet_queue.pop_front()
    }

    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<()> {
        let Some(connection) = self.connections.get_mut(&client_id) else {
            return Err(SteamError::NoConnection.into());
        };
        // TODO: compare this with self.listen_socket.send_messages()
        connection
            .send_message(buf, SendFlags::UNRELIABLE_NO_NAGLE)
            .context("Failed to send message")?;
        Ok(())
    }

    fn new_connections(&self) -> Vec<ClientId> {
        self.new_connections.clone()
    }

    fn new_disconnections(&self) -> Vec<ClientId> {
        self.new_disconnections.clone()
    }

    fn io(&self) -> &Io {
        todo!()
    }
}
