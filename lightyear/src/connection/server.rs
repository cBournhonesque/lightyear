use anyhow::Result;
use bevy::ecs::entity::EntityHash;
use bevy::prelude::{Entity, Resource};

use crate::_reexport::ReadWordBuffer;
use crate::connection::client::ClientConnection;
use crate::connection::netcode::ClientId;

#[cfg(all(feature = "steam", not(target_family = "wasm")))]
use crate::connection::steam::server::SteamConfig;
use crate::packet::packet::Packet;

use crate::prelude::{Io, IoConfig, LinkConditionerConfig};
use crate::server::config::NetcodeConfig;
use crate::utils::free_list::FreeList;

pub trait NetServer: Send + Sync {
    /// Start the server
    fn start(&mut self) -> Result<()>;

    /// Return the list of connected clients
    fn connected_client_ids(&self) -> Vec<ClientId>;

    /// Update the connection states + internal bookkeeping (keep-alives, etc.)
    fn try_update(&mut self, delta_ms: f64) -> Result<()>;

    /// Receive a packet from one of the connected clients
    fn recv(&mut self) -> Option<(Packet, ClientId)>;

    /// Send a packet to one of the connected clients
    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<()>;

    fn new_connections(&self) -> Vec<ClientId>;

    fn new_disconnections(&self) -> Vec<ClientId>;

    fn io(&self) -> &Io;
}

/// A wrapper around a `Box<dyn NetServer>`
#[derive(Resource)]
pub struct ServerConnection {
    server: Box<dyn NetServer>,
}

/// Configuration for the server connection
#[derive(Clone, Debug)]
pub enum NetConfig {
    Netcode {
        config: NetcodeConfig,
        io: IoConfig,
    },
    #[cfg(all(feature = "steam", not(target_family = "wasm")))]
    Steam {
        config: SteamConfig,
        conditioner: Option<LinkConditionerConfig>,
    },
}

impl Default for NetConfig {
    fn default() -> Self {
        NetConfig::Netcode {
            config: NetcodeConfig::default(),
            io: IoConfig::default(),
        }
    }
}

impl NetConfig {
    pub fn build_server(self) -> ServerConnection {
        match self {
            NetConfig::Netcode { config, io } => {
                let io = io.get_io();
                let server = super::netcode::Server::new(config, io);
                ServerConnection {
                    server: Box::new(server),
                }
            }
            // TODO: might want to distinguish between steam with direct ip connections
            //  vs steam with p2p connections
            #[cfg(all(feature = "steam", not(target_family = "wasm")))]
            NetConfig::Steam {
                config,
                conditioner,
            } => {
                // TODO: handle errors
                let server = super::steam::server::Server::new(config, conditioner)
                    .expect("could not create steam server");
                ServerConnection {
                    server: Box::new(server),
                }
            }
        }
    }
}

impl NetServer for ServerConnection {
    fn start(&mut self) -> Result<()> {
        self.server.start()
    }

    fn connected_client_ids(&self) -> Vec<ClientId> {
        self.server.connected_client_ids()
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<()> {
        self.server.try_update(delta_ms)
    }

    fn recv(&mut self) -> Option<(Packet, ClientId)> {
        self.server.recv()
    }

    fn send(&mut self, buf: &[u8], client_id: ClientId) -> Result<()> {
        self.server.send(buf, client_id)
    }

    fn new_connections(&self) -> Vec<ClientId> {
        self.server.new_connections()
    }

    fn new_disconnections(&self) -> Vec<ClientId> {
        self.server.new_disconnections()
    }

    fn io(&self) -> &Io {
        self.server.io()
    }
}

type EntityHashMap<K, V> = hashbrown::HashMap<K, V, EntityHash>;

// TODO: add a way to get the server of a given type?
/// On the server we allow the use of multiple types of ServerConnection at the same time
/// This resource holds the list of all the [`ServerConnection`]s, and maps client ids to the index of the server connection in the list
#[derive(Resource)]
pub struct ServerConnections {
    /// list of the various `ServerConnection`s available. Will be static after first insertion.
    pub servers: Vec<ServerConnection>,
    /// mapping from the connection's [`ClientId`] into a global [`ClientId`]-space (in case multiple transports use the same id)
    pub(crate) global_id_map: client_map::GlobalClientIdMap,
}

impl ServerConnections {
    pub fn new(config: Vec<NetConfig>) -> Self {
        let mut servers = vec![];
        for config in config {
            let server = config.build_server();
            servers.push(server);
        }
        ServerConnections {
            servers,
            global_id_map: client_map::GlobalClientIdMap::new(),
        }
    }
}

/// Since we use multiple independent [`ServerConnection`]s and each of them have their own id space, there might be collisions
/// between the [`ClientId`]s of different [`ServerConnection`]s. To solve this we will map the client ids from each [`ServerConnection`]
/// into a global id space
pub(crate) mod client_map {
    use super::EntityHashMap;
    use crate::prelude::ClientId;
    use bevy::prelude::Entity;
    use bevy::utils::HashMap;
    use tracing::info;

    pub(crate) type ServerConnectionIdx = usize;
    pub(crate) type ServerConnectionClientId = ClientId;
    pub(crate) type GlobalClientId = ClientId;

    pub(crate) struct GlobalClientIdMap {
        connection_to_global:
            HashMap<(ServerConnectionIdx, ServerConnectionClientId), GlobalClientId>,
        global_to_connection:
            EntityHashMap<GlobalClientId, (ServerConnectionIdx, ServerConnectionClientId)>,
    }

    impl GlobalClientIdMap {
        pub(crate) fn new() -> Self {
            GlobalClientIdMap {
                connection_to_global: HashMap::default(),
                global_to_connection: EntityHashMap::default(),
            }
        }

        pub(crate) fn insert(
            &mut self,
            server_idx: ServerConnectionIdx,
            client_id: ServerConnectionClientId,
        ) -> GlobalClientId {
            // generate a new global id for the newly-connected client

            // by default, try to reuse the same id as the connection's id
            let global_id = if !self.global_to_connection.contains_key(&client_id) {
                client_id as GlobalClientId
            } else {
                // there is a conflict! we need to find a new id
                // we will just try randomly until we find one that isn't in use
                let mut client_id = rand::random::<u64>();
                while self
                    .global_to_connection
                    .contains_key(&(client_id as GlobalClientId))
                {
                    client_id = rand::random::<u64>();
                }
                info!("ClientId already used (presumably by another ServerConnection)! Generating a new ClientId: {:?}", client_id);
                client_id as GlobalClientId
            };
            self.connection_to_global
                .insert((server_idx, client_id), global_id);
            self.global_to_connection
                .insert(global_id, (server_idx, client_id));
            global_id
        }

        #[inline]
        pub(crate) fn get_global(
            &self,
            server_idx: ServerConnectionIdx,
            client_id: ServerConnectionClientId,
        ) -> Option<GlobalClientId> {
            self.connection_to_global
                .get(&(server_idx, client_id))
                .copied()
        }

        #[inline]
        pub(crate) fn get_local(
            &self,
            client_id: GlobalClientId,
        ) -> Option<(ServerConnectionIdx, ServerConnectionClientId)> {
            self.global_to_connection.get(&client_id).copied()
        }

        #[inline]
        pub(crate) fn remove_by_local(
            &mut self,
            server_idx: ServerConnectionIdx,
            client_id: ServerConnectionClientId,
        ) -> Option<GlobalClientId> {
            let global_id = self.connection_to_global.remove(&(server_idx, client_id));
            if let Some(global_id) = global_id {
                self.global_to_connection.remove(&global_id);
            }
            global_id
        }
    }
}
