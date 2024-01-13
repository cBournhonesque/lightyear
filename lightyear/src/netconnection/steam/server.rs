use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet, VecDeque};

use crate::_reexport::{ReadBuffer, ReadWordBuffer};
use crate::netconnection::server::{ConnectionEvents, NetServer};
use crate::netconnection::steam::MAX_PACKET_SIZE;
use crate::netconnection::ClientId;
use crate::prelude::Io;
use steamworks::{
    networking_sockets::{InvalidHandle, ListenSocket, NetConnection},
    networking_types::{ListenSocketEvent, NetConnectionEnd, NetworkingConfigEntry, SendFlags},
    Client, ClientManager, FriendFlags, Friends, LobbyId, Manager, Matchmaking, ServerManager,
    SteamError, SteamId,
};
use tracing::error;

pub enum AccessPermission {
    /// Everyone can connect
    Public,
    /// No one can connect
    Private,
    /// Only friends from the host can connect
    FriendsOnly,
    /// Only user from this list can connect
    InList(HashSet<SteamId>),
    /// Users that are in the lobby can connect
    InLobby(LobbyId),
}

pub struct SteamServerConfig {
    pub max_clients: usize,
    pub access_permission: AccessPermission,
}

#[cfg_attr(feature = "bevy", derive(bevy_ecs::system::Resource))]
pub struct SteamNetServer {
    server: steamworks::Server,
    listen_socket: ListenSocket<ServerManager>,
    max_clients: usize,
    access_permission: AccessPermission,
    connections: HashMap<ClientId, NetConnection<ServerManager>>,
    packet_queue: VecDeque<(ReadWordBuffer, ClientId)>,
}

impl SteamNetServer {
    pub fn new(
        server: steamworks::Server,
        config: SteamServerConfig,
    ) -> Result<Self, InvalidHandle> {
        Ok(Self {
            server,
            listen_socket,
            max_clients: config.max_clients,
            access_permission: config.access_permission,
            connections: HashMap::new(),
            packet_queue: Default::default(),
        })
    }

    fn num_connected_clients(&self) -> usize {
        self.connections.len()
    }

    fn max_clients(&self) -> usize {
        self.max_clients
    }

    /// Update the access permission to the server,
    /// this change only applies to new connections.
    fn set_access_permissions(&mut self, access_permission: AccessPermission) {
        self.access_permission = access_permission;
    }

    /// Disconnects a client from the server.
    fn disconnect_client(&mut self, client_id: ClientId, flush_last_packets: bool) {
        if let Some((_key, value)) = self.connections.remove_entry(&client_id) {
            let _ = value.close(
                NetConnectionEnd::AppGeneric,
                Some("Client was kicked"),
                flush_last_packets,
            );
        }
    }

    /// Disconnects all active clients including the host client from the server.
    fn disconnect_all(&mut self, flush_last_packets: bool) {
        self.connections.keys().for_each(|client_id| {
            self.disconnect_client(*client_id, flush_last_packets);
        });
    }
}

impl NetServer for SteamNetServer {
    /// Update server connections, and receive packets from the network.
    fn try_update(&mut self, delta_ms: f64, io: &mut Io) -> Result<ConnectionEvents> {
        let mut events = ConnectionEvents {
            connected: Vec::new(),
            disconnected: Vec::new(),
        };
        // listen to connection requests
        while let Some(event) = self.listen_socket.try_receive_event() {
            match event {
                ListenSocketEvent::Connected(event) => {
                    if let Some(steam_id) = event.remote().steam_id() {
                        let client_id = steam_id.raw() as ClientId;
                        self.connections.insert(client_id, event.take_connection());
                        events.connected.push(client_id);
                    }
                }
                ListenSocketEvent::Disconnected(event) => {
                    if let Some(steam_id) = event.remote().steam_id() {
                        let client_id = steam_id.raw() as ClientId;
                        self.connections.remove(&client_id);
                        events.disconnected.push(client_id);
                    }
                }
                ListenSocketEvent::Connecting(event) => {
                    if self.num_connected_clients() >= self.max_clients {
                        event.reject(NetConnectionEnd::AppGeneric, Some("Too many clients"));
                        continue;
                    }

                    let Some(steam_id) = event.remote().steam_id() else {
                        event.reject(NetConnectionEnd::AppGeneric, Some("Invalid steam id"));
                        continue;
                    };

                    // let permitted = match &self.access_permission {
                    //     AccessPermission::Public => true,
                    //     AccessPermission::Private => false,
                    //     AccessPermission::FriendsOnly => {
                    //         let friend = self.friends.get_friend(steam_id);
                    //         friend.has_friend(FriendFlags::IMMEDIATE)
                    //     }
                    //     AccessPermission::InList(list) => list.contains(&steam_id),
                    //     AccessPermission::InLobby(lobby) => {
                    //         let users_in_lobby = self.matchmaking.lobby_members(*lobby);
                    //         users_in_lobby.contains(&steam_id)
                    //     }
                    // };
                    let permitted = true;
                    if permitted {
                        if let Err(e) = event.accept() {
                            error!("Failed to accept connection from {steam_id:?}: {e}");
                        }
                    } else {
                        event.reject(NetConnectionEnd::AppGeneric, Some("Not allowed"));
                    }
                }
            }
        }

        for (client_id, connection) in self.connections.iter_mut() {
            // TODO this allocates on the side of steamworks.rs and should be avoided, PR needed
            for message in connection.receive_messages(MAX_PACKET_SIZE) {
                self.packet_queue
                    .push_back((ReadWordBuffer::start_read(message.data()), *client_id));
            }
        }
        self.connections.values_mut().try_for_each(|connection| {
            connection
                .flush_messages()
                .context("Failed to flush messages")
        })?;
        Ok(events)
    }

    fn send(&mut self, buf: &[u8], client_id: ClientId, io: &mut Io) -> Result<()> {
        // self.connections.get_mut(&client_id).context()
        let Some(connection) = self.connections.get_mut(&client_id) else {
            return Err(SteamError::NoConnection.into());
        };
        connection
            .send_message(buf, SendFlags::UNRELIABLE)
            .context("Failed to send message")?;
        Ok(())
    }

    fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)> {
        self.packet_queue.pop_front()
    }

    fn connected_client_ids(&self) -> Vec<ClientId> {
        self.connections.keys().collect()
    }
}
