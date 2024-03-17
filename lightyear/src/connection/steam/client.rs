use crate::_reexport::{ReadBuffer, ReadWordBuffer};
use crate::connection::client::NetClient;
use crate::packet::packet::Packet;
use crate::prelude::{ClientId, Io, LinkConditionerConfig};
use crate::serialize::wordbuffer::reader::BufferPool;
use crate::transport::LOCAL_SOCKET;
use anyhow::{anyhow, Context, Result};
use std::collections::VecDeque;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::{Arc, RwLock};
use steamworks::networking_sockets::{NetConnection, NetworkingSockets};
use steamworks::networking_types::{
    NetConnectionEnd, NetConnectionInfo, NetworkingConfigEntry, NetworkingConfigValue,
    NetworkingConnectionState, SendFlags,
};
use steamworks::{ClientManager, SingleClient};
use tracing::{info, warn};

use super::{get_networking_options, SingleClientThreadSafe};

const MAX_MESSAGE_BATCH_SIZE: usize = 512;

#[derive(Debug, Clone)]
pub struct SteamConfig {
    pub server_addr: SocketAddr,
    pub app_id: u32,
}
impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            server_addr: SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), 27015)),
            // app id of the public Space Wars demo app
            app_id: 480,
        }
    }
}

pub struct Client {
    client: steamworks::Client<ClientManager>,
    single_client: SingleClientThreadSafe,
    config: SteamConfig,
    connection: Option<NetConnection<ClientManager>>,
    packet_queue: VecDeque<Packet>,
    buffer_pool: BufferPool,
    conditioner: Option<LinkConditionerConfig>,
}

impl Client {
    pub fn new(config: SteamConfig, conditioner: Option<LinkConditionerConfig>) -> Result<Self> {
        let (client, single) = steamworks::Client::init_app(config.app_id)
            .context("could not initialize steam client")?;

        Ok(Self {
            client,
            single_client: SingleClientThreadSafe(single),
            config,
            connection: None,
            packet_queue: VecDeque::new(),
            buffer_pool: BufferPool::default(),
            conditioner,
        })
    }

    fn connection_info(&self) -> Option<Result<NetConnectionInfo>> {
        self.connection.as_ref().map(|connection| {
            self.client
                .networking_sockets()
                .get_connection_info(connection)
                .map_err(|err| anyhow!("could not get connection info"))
        })
    }

    fn connection_state(&self) -> Result<NetworkingConnectionState> {
        self.connection_info()
            .unwrap_or(Err(anyhow!("no connection")))
            .map_or(Ok(NetworkingConnectionState::None), |info| info.state())
            .context("could not get connection state")
    }
}

impl NetClient for Client {
    fn connect(&mut self) -> Result<()> {
        let options = get_networking_options(&self.conditioner);
        self.connection = Some(
            self.client
                .networking_sockets()
                .connect_by_ip_address(self.config.server_addr, options)
                .context("failed to create connection")?,
        );
        info!(
            "Opened steam connection to server at address: {}",
            self.config.server_addr
        );
        Ok(())
    }

    fn is_connected(&self) -> bool {
        matches!(
            self.connection_state(),
            Ok(NetworkingConnectionState::Connected)
        )
    }

    fn try_update(&mut self, delta_ms: f64) -> Result<()> {
        self.single_client.0.run_callbacks();

        // TODO: should I maintain an internal state for the connection? or just rely on `connection_state()` ?
        // update connection state
        return match self.connection_state()? {
            NetworkingConnectionState::None => Err(anyhow!("no connection")),
            NetworkingConnectionState::Connecting | NetworkingConnectionState::FindingRoute => {
                Ok(())
            }
            NetworkingConnectionState::ClosedByPeer
            | NetworkingConnectionState::ProblemDetectedLocally => {
                Err(anyhow!("connection closed"))
            }
            NetworkingConnectionState::Connected => {
                // receive packet
                let connection = self.connection.as_mut().unwrap();
                for message in connection
                    .receive_messages(MAX_MESSAGE_BATCH_SIZE)
                    .context("failed to receive messages")?
                {
                    // get a buffer from the pool to avoid new allocations
                    let mut reader = self.buffer_pool.start_read(message.data());
                    let packet = Packet::decode(&mut reader).context("could not decode packet")?;
                    // return the buffer to the pool
                    self.buffer_pool.attach(reader);
                    self.packet_queue.push_back(packet);
                }
                Ok(())
            }
        };
    }

    fn recv(&mut self) -> Option<Packet> {
        self.packet_queue.pop_front()
    }

    fn send(&mut self, buf: &[u8]) -> Result<()> {
        self.connection
            .as_ref()
            .ok_or(anyhow!("client not connected"))?
            .send_message(buf, SendFlags::UNRELIABLE_NO_NAGLE)
            .context("could not send message")?;
        Ok(())
    }

    fn id(&self) -> ClientId {
        self.client.user().steam_id().raw()
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
