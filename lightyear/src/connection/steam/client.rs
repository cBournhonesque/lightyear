use crate::connection::client::{ConnectionError, ConnectionState, NetClient};
use crate::connection::id::ClientId;
use crate::packet::packet_builder::RecvPayload;
use crate::prelude::client::Io;
use crate::prelude::LinkConditionerConfig;
use crate::transport::LOCAL_SOCKET;
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use steamworks::networking_sockets::{InvalidHandle, NetConnection};
use steamworks::networking_types::{
    NetConnectionEnd, NetConnectionInfo, NetworkingConnectionState, NetworkingIdentity, SendFlags,
};
use steamworks::{ClientManager, SteamError, SteamId};
use tracing::info;

use super::steamworks_client::SteamworksClient;

const MAX_MESSAGE_BATCH_SIZE: usize = 512;

#[derive(Debug, Clone)]
pub struct SteamConfig {
    pub socket_config: SocketConfig,
    pub app_id: u32,
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            socket_config: Default::default(),
            app_id: 480,
        }
    }
}

/// Steam socket configuration for clients
#[derive(Debug, Clone)]
pub enum SocketConfig {
    /// Connect to a server by IP address. Suitable for dedicated servers.
    Ip { server_addr: SocketAddr },
    /// Connect to another Steam user hosting a server. Suitable for
    /// peer-to-peer games.
    P2P { virtual_port: i32, steam_id: u64 },
}

impl Default for SocketConfig {
    fn default() -> Self {
        SocketConfig::Ip {
            server_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 27015)),
        }
    }
}

pub struct Client {
    steamworks_client: Arc<RwLock<SteamworksClient>>,
    config: SteamConfig,
    connection: Option<NetConnection<ClientManager>>,
    packet_queue: VecDeque<RecvPayload>,
    conditioner: Option<LinkConditionerConfig>,
}

impl Client {
    pub fn new(
        steamworks_client: Arc<RwLock<SteamworksClient>>,
        config: SteamConfig,
        conditioner: Option<LinkConditionerConfig>,
    ) -> Self {
        Self {
            steamworks_client,
            config,
            connection: None,
            packet_queue: VecDeque::new(),
            conditioner,
        }
    }

    fn connection_info(&self) -> Option<Result<NetConnectionInfo, ConnectionError>> {
        self.connection.as_ref().map(|connection| {
            self.steamworks_client
                .try_read()
                .expect("could not get steamworks client")
                .get_client()
                .networking_sockets()
                .get_connection_info(connection)
                .map_err(|err| ConnectionError::SteamInvalidHandle(InvalidHandle))
        })
    }

    fn connection_state(&self) -> Result<NetworkingConnectionState, ConnectionError> {
        self.connection_info()
            .unwrap_or(Err(SteamError::NoConnection.into()))
            .map_or(Ok(NetworkingConnectionState::None), |info| {
                info.state()
                    .map_err(|err| ConnectionError::SteamInvalidState(err))
            })
    }
}

impl NetClient for Client {
    fn connect(&mut self) -> Result<(), ConnectionError> {
        // TODO: using the NetworkingConfigEntry options seems to cause an issue. See: https://github.com/Noxime/steamworks-rs/issues/169
        // let options = get_networking_options(&self.conditioner);

        match self.config.socket_config {
            SocketConfig::Ip { server_addr } => {
                self.connection = Some(
                    self.steamworks_client
                        .try_read()
                        .expect("could not get steamworks client")
                        .get_client()
                        .networking_sockets()
                        .connect_by_ip_address(server_addr, vec![])?,
                );
                info!(
                    "Opened steam connection to server at address: {}",
                    server_addr
                );
            }
            SocketConfig::P2P {
                virtual_port,
                steam_id,
            } => {
                self.connection = Some(
                    self.steamworks_client
                        .try_read()
                        .expect("could not get steamworks client")
                        .get_client()
                        .networking_sockets()
                        .connect_p2p(
                            NetworkingIdentity::new_steam_id(SteamId::from_raw(steam_id)),
                            virtual_port,
                            vec![],
                        )?,
                );
            }
        }
        Ok(())
    }

    fn disconnect(&mut self) -> Result<(), ConnectionError> {
        if let Some(connection) = core::mem::take(&mut self.connection) {
            connection.close(NetConnectionEnd::AppGeneric, None, false);
        }
        Ok(())
    }

    fn state(&self) -> ConnectionState {
        match self
            .connection_state()
            .unwrap_or(NetworkingConnectionState::None)
        {
            NetworkingConnectionState::Connecting | NetworkingConnectionState::FindingRoute => {
                ConnectionState::Connecting
            }
            NetworkingConnectionState::Connected => ConnectionState::Connected,
            _ => {
                let reason = self
                    .connection_info()
                    .map_or(None, |info| info.ok().map(|i| i.end_reason()))
                    .flatten()
                    .map(|r| ConnectionError::SteamDisconnection(r));
                ConnectionState::Disconnected { reason }
            }
        }
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<(), ConnectionError> {
        self.steamworks_client
            .try_write()
            .expect("could not get steamworks single client")
            .get_single()
            .run_callbacks();

        // TODO: should I maintain an internal state for the connection? or just rely on `connection_state()` ?
        // update connection state
        return match self.connection_state()? {
            NetworkingConnectionState::None => Err(SteamError::NoConnection.into()),
            NetworkingConnectionState::Connecting | NetworkingConnectionState::FindingRoute => {
                Ok(())
            }
            NetworkingConnectionState::ClosedByPeer
            | NetworkingConnectionState::ProblemDetectedLocally => {
                Err(SteamError::IOFailure.into())
            }
            NetworkingConnectionState::Connected => {
                // receive packet
                let connection = self.connection.as_mut().unwrap();
                for message in connection.receive_messages(MAX_MESSAGE_BATCH_SIZE)? {
                    // // get a buffer from the pool to avoid new allocations
                    // let mut reader = self.buffer_pool.start_read(message.data());
                    // let packet = Packet::decode(&mut reader).context("could not decode packet")?;
                    // // return the buffer to the pool
                    // self.buffer_pool.attach(reader);
                    let payload = RecvPayload::copy_from_slice(message.data());
                    self.packet_queue.push_back(payload);
                }
                Ok(())
            }
        };
    }

    fn recv(&mut self) -> Option<RecvPayload> {
        self.packet_queue.pop_front()
    }

    fn send(&mut self, buf: &[u8]) -> Result<(), ConnectionError> {
        self.connection
            .as_ref()
            .ok_or(ConnectionError::NotConnected)?
            .send_message(buf, SendFlags::UNRELIABLE_NO_NAGLE)?;
        Ok(())
    }

    fn id(&self) -> ClientId {
        ClientId::Steam(
            self.steamworks_client
                .try_read()
                .expect("could not get steamworks client")
                .get_client()
                .user()
                .steam_id()
                .raw(),
        )
    }

    fn local_addr(&self) -> SocketAddr {
        LOCAL_SOCKET
    }

    fn io(&self) -> Option<&Io> {
        None
    }

    fn io_mut(&mut self) -> Option<&mut Io> {
        None
    }
}
