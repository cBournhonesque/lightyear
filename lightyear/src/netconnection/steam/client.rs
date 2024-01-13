use crate::_reexport::{ReadBuffer, ReadWordBuffer};
use crate::netconnection::client::NetClient;
use crate::netconnection::steam::MAX_PACKET_SIZE;
use crate::netconnection::ClientId;
use crate::prelude::Io;
use anyhow::anyhow;
use std::collections::VecDeque;
use steamworks::{
    networking_sockets::{InvalidHandle, NetConnection, NetworkingSockets},
    networking_types::{
        NetConnectionEnd, NetworkingConnectionState, NetworkingIdentity, SendFlags,
    },
    ClientManager, SteamError, SteamId,
};

enum ConnectionState {
    NotStarted,
    Connected {
        connection: NetConnection<ClientManager>,
    },
    Disconnected {
        end_reason: NetConnectionEnd,
    },
}

#[cfg_attr(feature = "bevy", derive(bevy_ecs::system::Resource))]
pub struct Client {
    client: steamworks::Client<ClientManager>,
    state: ConnectionState,
    packet_queue: VecDeque<ReadWordBuffer>,
}

impl Client {
    pub fn new(client: steamworks::Client<ClientManager>) -> Result<Self, InvalidHandle> {
        let networking_sockets = client.networking_sockets();
        Ok(Self {
            client,
            state: ConnectionState::NotStarted,
            packet_queue: VecDeque::new(),
        })
    }

    fn steam_id(&self) -> SteamId {
        self.client.user().steam_id()
    }

    pub fn connect(&mut self) {
        let options = Vec::new();
        let connection = self
            .client
            .networking_sockets()
            .connect_p2p(NetworkingIdentity::new_steam_id(self.steam_id), 0, options)
            .unwrap();
    }

    pub fn is_connected(&self) -> bool {
        let status = self.connection_state();

        status == NetworkingConnectionState::Connected
    }

    pub fn is_disconnected(&self) -> bool {
        let status = self.connection_state();
        status == NetworkingConnectionState::ClosedByPeer
            || status == NetworkingConnectionState::ProblemDetectedLocally
            || status == NetworkingConnectionState::None
    }

    pub fn is_connecting(&self) -> bool {
        let status = self.connection_state();
        status == NetworkingConnectionState::Connecting
            || status == NetworkingConnectionState::FindingRoute
    }

    pub fn connection_state(&self) -> NetworkingConnectionState {
        let connection = match &self.state {
            ConnectionState::Connected { connection } => connection,
            ConnectionState::NotStarted | ConnectionState::Disconnected { .. } => {
                return NetworkingConnectionState::None;
            }
        };

        let Ok(info) = self
            .client
            .networking_sockets()
            .get_connection_info(connection)
        else {
            return NetworkingConnectionState::None;
        };

        match info.state() {
            Ok(state) => state,
            Err(_) => NetworkingConnectionState::None,
        }
    }

    pub fn disconnect_reason(&self) -> Option<NetConnectionEnd> {
        let connection = match &self.state {
            ConnectionState::NotStarted => return None,
            ConnectionState::Connected { connection } => connection,
            ConnectionState::Disconnected { end_reason, .. } => {
                return Some(*end_reason);
            }
        };

        if let Ok(info) = self
            .client
            .networking_sockets()
            .get_connection_info(connection)
        {
            return info.end_reason();
        }

        None
    }

    pub fn client_id(&self, steam_client: &steamworks::Client<ClientManager>) -> ClientId {
        steam_client.user().steam_id().raw()
    }

    pub fn disconnect(&mut self) {
        if matches!(self.state, ConnectionState::Disconnected { .. }) {
            return;
        }

        let disconnect_state = ConnectionState::Disconnected {
            end_reason: NetConnectionEnd::AppGeneric,
        };
        let old_state = std::mem::replace(&mut self.state, disconnect_state);
        if let ConnectionState::Connected { connection } = old_state {
            connection.close(
                NetConnectionEnd::AppGeneric,
                Some("Client disconnected"),
                false,
            );
        }
    }

    /// Bookkeeping + receive packets + send packets
    pub fn update(&mut self) {
        if self.is_disconnected() {
            if let ConnectionState::Connected { connection } = &self.state {
                let end_reason = self
                    .client
                    .networking_sockets()
                    .get_connection_info(connection)
                    .map(|info| info.end_reason())
                    .unwrap_or_default()
                    .unwrap_or(NetConnectionEnd::AppGeneric);

                self.state = ConnectionState::Disconnected { end_reason };
            }

            return;
        };

        let ConnectionState::Connected { connection } = &mut self.state else {
            unreachable!()
        };

        let messages = connection.receive_messages(MAX_PACKET_SIZE);
        messages.iter().for_each(|message| {
            let reader = ReadWordBuffer::start_read(message.data());
            self.packet_queue.push_back(reader);
        });
    }

    pub fn recv(&mut self) -> Option<ReadWordBuffer> {
        self.packet_queue.pop_front()
    }

    pub fn send(&mut self, buf: &[u8], io: &mut Io) -> Result<(), SteamError> {
        if self.is_disconnected() {
            return Err(SteamError::NoConnection);
        }

        if self.is_connecting() {
            return Ok(());
        }
        let ConnectionState::Connected { connection } = &mut self.state else {
            unreachable!()
        };
        connection.send_message(&buf, SendFlags::UNRELIABLE)?;

        connection.flush_messages()
    }
}

impl NetClient for Client {
    fn connect(&mut self) {
        self.connect()
    }

    fn is_connected(&self) -> bool {
        self.is_connected()
    }
    fn try_update(&mut self, delta_ms: f64, io: &mut Io) -> anyhow::Result<()> {
        self.try_update(delta_ms, io).map_err(|e| anyhow!(e))
    }

    fn recv(&mut self) -> Option<ReadWordBuffer> {
        self.recv()
    }

    fn send(&mut self, buf: &[u8], io: &mut Io) -> anyhow::Result<()> {
        self.send(buf, io).map_err(|e| anyhow!(e))
    }
}
