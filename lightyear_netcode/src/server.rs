use alloc::collections::VecDeque;
use alloc::sync::Arc;
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, format, string::ToString, vec, vec::Vec};
use bevy::platform_support::collections::HashMap;
use bevy::prelude::Resource;
use bevy::reflect::List;
use core::net::SocketAddr;
use no_std_io2::io;
use no_std_io2::io::Seek;
use tracing::{debug, error, trace, warn};

use super::{
    bytes::Bytes,
    crypto::{self, Key},
    error::{Error, Result},
    packet::{
        ChallengePacket, DeniedPacket, DisconnectPacket, KeepAlivePacket, Packet, PayloadPacket,
        RequestPacket, ResponsePacket,
    },
    replay::ReplayProtection,
    token::{ChallengeToken, ConnectToken, ConnectTokenBuilder, ConnectTokenPrivate},
    MAC_BYTES, MAX_PACKET_SIZE, MAX_PKT_BUF_SIZE, PACKET_SEND_RATE_SEC,
};
use crate::token::TOKEN_EXPIRE_SEC;
use lightyear_connection::id;
use lightyear_connection::server::{ConnectionError, ConnectionRequestHandler, DefaultConnectionRequestHandler, DeniedReason};
use lightyear_link::{Link, LinkReceiver, LinkSender, RecvPayload, SendPayload};
use lightyear_serde::reader::ReadInteger;
use lightyear_serde::writer::Writer;
#[cfg(feature = "trace")]
use tracing::{instrument, Level};

pub const MAX_CLIENTS: usize = 256;

const CLIENT_TIMEOUT_SECS: i32 = 10;

#[derive(Clone, Copy)]
struct TokenEntry {
    time: f64,
    mac: [u8; 16],
    addr: SocketAddr,
}

struct TokenEntries {
    inner: Vec<TokenEntry>,
}


impl TokenEntries {
    fn new() -> Self {
        Self { inner: Vec::new() }
    }
    fn find_or_insert(&mut self, entry: TokenEntry) -> bool {
        let (mut oldest, mut matching) = (None, None);
        let mut oldest_time = f64::INFINITY;
        // Perform a linear search for the oldest and matching entries at the same time
        for (idx, saved_entry) in self.inner.iter().enumerate() {
            if entry.time < oldest_time {
                oldest_time = saved_entry.time;
                oldest = Some(idx);
            }
            if entry.mac == saved_entry.mac {
                matching = Some(idx);
            }
        }
        let Some(oldest) = oldest else {
            // If there is no oldest entry then the list is empty, so just insert the entry
            self.inner.push(entry);
            return true;
        };
        if let Some(matching) = matching {
            // Allow reusing tokens only if the address matches
            self.inner[matching].addr == entry.addr
        } else {
            // If there is no matching entry, replace the oldest one
            self.inner[oldest] = entry;
            true
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Connection {
    confirmed: bool,
    connected: bool,
    client_id: ClientId,
    addr: SocketAddr,
    timeout: i32,
    last_access_time: f64,
    last_send_time: f64,
    last_receive_time: f64,
    send_key: Key,
    receive_key: Key,
    sequence: u64,
}


impl Connection {
    fn confirm(&mut self) {
        self.confirmed = true;
    }
    fn connect(&mut self) {
        self.connected = true;
    }
    fn is_confirmed(&self) -> bool {
        self.confirmed
    }
    fn is_connected(&self) -> bool {
        self.connected
    }
}

/// The client id from a connect token, must be unique for each client.
pub type ClientId = u64;

struct ConnectionCache {
    // this somewhat mimics the original C implementation,
    // the main difference being that `Connection` includes the encryption mapping as well.
    clients: HashMap<ClientId, Connection>,

    // map from client address to client id
    client_id_map: HashMap<SocketAddr, ClientId>,

    // we are not using a free-list here to not allocate memory up-front, since `ReplayProtection` is biggish (~2kb)
    replay_protection: HashMap<ClientId, ReplayProtection>,

    // packets that are buffered and ready to be sent
    send_queue: Vec<(SendPayload, ClientId)>,

    // packet queue for all clients
    packet_queue: Vec<(RecvPayload, ClientId)>,

    // corresponds to the server time
    time: f64,
}

impl ConnectionCache {
    fn new(server_time: f64) -> Self {
        Self {
            clients: HashMap::default(),
            client_id_map: HashMap::default(),
            replay_protection: HashMap::default(),
            send_queue: Vec::with_capacity(MAX_CLIENTS * 2),
            packet_queue: Vec::with_capacity(MAX_CLIENTS * 2),
            time: server_time,
        }
    }
    fn add(
        &mut self,
        client_id: ClientId,
        addr: SocketAddr,
        timeout: i32,
        send_key: Key,
        receive_key: Key,
    ) {
        if let Some((_, ref mut existing)) = self.find_by_addr(&addr) {
            existing.client_id = client_id;
            existing.timeout = timeout;
            existing.send_key = send_key;
            existing.receive_key = receive_key;
            existing.last_access_time = self.time;
            return;
        }
        let conn = Connection {
            confirmed: false,
            connected: false,
            client_id,
            addr,
            timeout,
            last_access_time: self.time,
            last_send_time: f64::NEG_INFINITY,
            last_receive_time: f64::NEG_INFINITY,
            send_key,
            receive_key,
            sequence: 0,
        };
        self.clients.insert(client_id, conn);
        self.replay_protection
            .insert(client_id, ReplayProtection::new());

        self.client_id_map.insert(addr, client_id);
    }
    fn remove(&mut self, client_id: ClientId) {
        let Some(conn) = self.clients.get(&client_id) else {
            return;
        };
        if !conn.is_connected() {
            return;
        }
        self.client_id_map.remove(&conn.addr);
        self.replay_protection.remove(&client_id);
        self.clients.remove(&client_id);
    }

    fn ids(&self) -> Vec<ClientId> {
        self.clients.keys().cloned().collect()
    }

    fn find_by_addr(&self, addr: &SocketAddr) -> Option<(ClientId, Connection)> {
        self.client_id_map
            .get(addr)
            .and_then(|id| self.clients.get(id).map(|conn| (*id, *conn)))
    }
    fn find_by_id(&self, client_id: ClientId) -> Option<Connection> {
        self.clients.get(&client_id).cloned()
    }
    fn update(&mut self, delta_ms: f64) {
        self.time += delta_ms;
    }

    /// Get a new client id that is not already in use.
    fn new_id(&self) -> ClientId {
        let mut id = rand::random::<u64>();
        while self.clients.contains_key(&id) {
            id = rand::random::<u64>();
        }
        id
    }
}

pub type Callback<Ctx> = Box<dyn FnMut(ClientId, SocketAddr, &mut Ctx) + Send + Sync + 'static>;

/// Configuration for a server.
///
/// * `num_disconnect_packets` - The number of redundant disconnect packets that will be sent to a client when the server is disconnecting it.
/// * `keep_alive_send_rate` - The rate at which keep-alive packets will be sent to clients.
/// * `on_connect` - A callback that will be called when a client is connected to the server.
/// * `on_disconnect` - A callback that will be called when a client is disconnected from the server.
///
/// # Example
/// ```
/// # let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 40005));
/// # let protocol_id = 0x123456789ABCDEF0;
/// # let private_key = [42u8; 32];
/// use std::sync::{Arc, Mutex};
/// use lightyear_netcode::{NetcodeServer, ServerConfig};
///
/// let thread_safe_counter = Arc::new(Mutex::new(0));
/// let cfg = ServerConfig::with_context(thread_safe_counter).on_connect(|idx, _, ctx| {
///     let mut counter = ctx.lock().unwrap();
///     *counter += 1;
///     println!("client {} connected, counter: {idx}", counter);
/// });
/// let server = NetcodeServer::with_config(protocol_id, private_key, cfg).unwrap();
/// ```
pub struct ServerConfig<Ctx> {
    num_disconnect_packets: usize,
    keep_alive_send_rate: f64,
    token_expire_secs: i32,
    client_timeout_secs: i32,
    connection_request_handler: Arc<dyn ConnectionRequestHandler>,
    server_addr: SocketAddr,
    pub(crate) context: Ctx,
    on_connect: Option<Callback<Ctx>>,
    on_disconnect: Option<Callback<Ctx>>,
}

impl Default for ServerConfig<()> {
    fn default() -> Self {
        Self {
            num_disconnect_packets: 10,
            keep_alive_send_rate: PACKET_SEND_RATE_SEC,
            token_expire_secs: TOKEN_EXPIRE_SEC,
            client_timeout_secs: CLIENT_TIMEOUT_SECS,
            connection_request_handler: Arc::new(DefaultConnectionRequestHandler),
            server_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            context: (),
            on_connect: None,
            on_disconnect: None,
        }
    }
}

impl<Ctx> ServerConfig<Ctx> {
    /// Create a new, default server configuration with no context.
    pub fn new() -> ServerConfig<()> {
        ServerConfig::<()>::default()
    }
    /// Create a new server configuration with context that will be passed to the callbacks.
    pub fn with_context(ctx: Ctx) -> Self {
        Self {
            num_disconnect_packets: 10,
            keep_alive_send_rate: PACKET_SEND_RATE_SEC,
            token_expire_secs: TOKEN_EXPIRE_SEC,
            client_timeout_secs: CLIENT_TIMEOUT_SECS,
            connection_request_handler: Arc::new(DefaultConnectionRequestHandler),
            server_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            context: ctx,
            on_connect: None,
            on_disconnect: None,
        }
    }
    /// Set the number of redundant disconnect packets that will be sent to a client when the server is disconnecting it. <br>
    /// The default is 10 packets.
    pub fn num_disconnect_packets(mut self, num: usize) -> Self {
        self.num_disconnect_packets = num;
        self
    }
    /// Set the rate (in seconds) at which keep-alive packets will be sent to clients. <br>
    /// The default is 10 packets per second. (`0.1` seconds)
    pub fn keep_alive_send_rate(mut self, rate_seconds: f64) -> Self {
        self.keep_alive_send_rate = rate_seconds;
        self
    }
    /// Set the duration (in seconds) after which the server disconnects a client if they don't hear from them.
    /// The default is 10 seconds.
    pub fn client_timeout_secs(mut self, client_timeout_secs: i32) -> Self {
        self.client_timeout_secs = client_timeout_secs;
        self
    }
    /// Set the duration (in seconds) after which ConnectTokens generated by the server will expire
    /// The default is 30 seconds.
    pub fn token_expire_secs(mut self, expire_secs: i32) -> Self {
        self.token_expire_secs = expire_secs;
        self
    }
    /// Set the socket address of the server.
    // TODO: This actually NEEDS to be set, change the API to force this
    pub fn server_addr(mut self, server_addr: SocketAddr) -> Self {
        self.server_addr = server_addr;
        self
    }
    /// Provide a callback that will be called when a client is connected to the server. <br>
    /// The callback will be called with the client index and the context that was provided (provide a `None` context if you don't need one).
    ///
    /// See [`ServerConfig`] for an example.
    pub fn on_connect<F>(mut self, cb: F) -> Self
    where
        F: FnMut(ClientId, SocketAddr, &mut Ctx) + Send + Sync + 'static,
    {
        self.on_connect = Some(Box::new(cb));
        self
    }
    /// Provide a callback that will be called when a client is disconnected from the server. <br>
    /// The callback will be called with the client index and the context that was provided (provide a `None` context if you don't need one).
    ///
    /// See [`ServerConfig`] for an example.
    pub fn on_disconnect<F>(mut self, cb: F) -> Self
    where
        F: FnMut(ClientId, SocketAddr, &mut Ctx) + Send + Sync + 'static,
    {
        self.on_disconnect = Some(Box::new(cb));
        self
    }
}

/// The `netcode` server.
///
/// Responsible for accepting connections from clients and communicating with them using the netcode protocol. <br>
/// The server should be run in a loop to process incoming packets, send updates to clients, and maintain stable connections.
///
/// # Example
///
/// ```
/// # use std::net::{SocketAddr, Ipv4Addr};
/// # use core::time::Duration;
/// # use std::thread;
/// # use lightyear_link::Link;
/// # use lightyear_netcode::{generate_key, NetcodeServer};
/// let mut link = Link::new(SocketAddr::from(([127, 0, 0, 1], 12345)));
/// let private_key = generate_key();
/// let protocol_id = 0x123456789ABCDEF0;
/// let mut server = NetcodeServer::new(protocol_id, private_key).unwrap();
///
/// let tick_rate = Duration::from_secs_f64(1.0 / 60.0);
///
/// loop {
///     server.update(tick_rate.as_secs_f64() / 1000.0, &mut link);
///     if let Some((received, from)) = server.recv() {
///         // ...
///     }
///     thread::sleep(tick_rate);
///     # break;
/// }
/// ```
///
pub struct NetcodeServer<Ctx = ()> {
    time: f64,
    private_key: Key,
    sequence: u64,
    token_sequence: u64,
    challenge_sequence: u64,
    challenge_key: Key,
    protocol_id: u64,
    conn_cache: ConnectionCache,
    token_entries: TokenEntries,
    pub(crate) cfg: ServerConfig<Ctx>,
    // We use a Writer (wrapper around BytesMut) here because we will keep re-using the
    // same allocation for the bytes we send.
    // 1. We create an array on the stack of size MAX_PACKET_SIZE
    // 2. We copy the serialized array in the writer via `extend_from_size`
    // 3. We split the bytes off, to recover the allocation
    writer: Writer,
    client_errors: Vec<Error>,
}

impl NetcodeServer {
    /// Create a new server with a default configuration.
    ///
    /// For a custom configuration, use [`Server::with_config`](NetcodeServer::with_config) instead.
    pub fn new(protocol_id: u64, private_key: Key) -> Result<Self> {
        let server: NetcodeServer<()> = NetcodeServer {
            time: 0.0,
            private_key,
            protocol_id,
            sequence: 1 << 63,
            token_sequence: 0,
            challenge_sequence: 0,
            challenge_key: crypto::generate_key(),
            conn_cache: ConnectionCache::new(0.0),
            token_entries: TokenEntries::new(),
            cfg: ServerConfig::default(),
            writer: Writer::with_capacity(MAX_PKT_BUF_SIZE),
            client_errors: vec![],
        };
        // info!("server started on {}", server.io.local_addr());
        Ok(server)
    }
}

impl<Ctx> NetcodeServer<Ctx> {
    /// Create a new server with a custom configuration. <br>
    /// Callbacks with context can be registered with the server to be notified when the server changes states. <br>
    /// See [`ServerConfig`] for more details.
    ///
    /// # Example
    /// ```
    /// # use lightyear_netcode::{generate_key, NetcodeServer, ServerConfig};
    ///
    /// let private_key = generate_key();
    /// let protocol_id = 0x123456789ABCDEF0;
    /// let cfg = ServerConfig::with_context(42).on_connect(|idx, _, ctx| {
    ///     assert_eq!(ctx, &42);
    /// });
    /// let server = NetcodeServer::with_config(protocol_id, private_key, cfg).unwrap();
    /// ```
    pub fn with_config(protocol_id: u64, private_key: Key, cfg: ServerConfig<Ctx>) -> Result<Self> {
        let server = NetcodeServer {
            time: 0.0,
            private_key,
            protocol_id,
            sequence: 1 << 63,
            token_sequence: 0,
            challenge_sequence: 0,
            challenge_key: crypto::generate_key(),
            conn_cache: ConnectionCache::new(0.0),
            token_entries: TokenEntries::new(),
            cfg,
            writer: Writer::with_capacity(MAX_PKT_BUF_SIZE),
            client_errors: vec![],
        };
        // info!("server started on {}", server.addr());
        Ok(server)
    }
}

pub(crate) enum ConnectionUpdate {
    /// A new connection was established
    Connected(ClientId, SocketAddr),
    /// A connection was closed
    Disconnected(ClientId, SocketAddr),
}

impl<Ctx> NetcodeServer<Ctx> {
    const ALLOWED_PACKETS: u8 = 1 << Packet::REQUEST
        | 1 << Packet::RESPONSE
        | 1 << Packet::KEEP_ALIVE
        | 1 << Packet::PAYLOAD
        | 1 << Packet::DISCONNECT;
    fn on_connect(&mut self, client_id: ClientId, addr: SocketAddr) {
        if let Some(cb) = self.cfg.on_connect.as_mut() {
            cb(client_id, addr, &mut self.cfg.context)
        }
    }
    fn on_disconnect(&mut self, client_id: ClientId, addr: SocketAddr) {
        if let Some(cb) = self.cfg.on_disconnect.as_mut() {
            cb(client_id, addr, &mut self.cfg.context)
        }
    }
    fn handle_client_error(&mut self, error: Error) {
        self.client_errors.push(error);
    }
    fn touch_client(&mut self, client_id: Option<ClientId>) {
        let Some(id) = client_id else {
            return;
        };
        let Some(conn) = self.conn_cache.clients.get_mut(&id) else {
            return;
        };
        conn.last_receive_time = self.time;
        if !conn.is_confirmed() {
            debug!("server confirmed connection with client {id}");
            conn.confirm();
        }
    }
    fn process_packet(
        &mut self,
        addr: SocketAddr,
        packet: Packet,
        sender: &mut LinkSender,
    ) -> Result<Option<ConnectionUpdate>> {
        let client_id = self.conn_cache.find_by_addr(&addr).map(|(id, _)| id);
        trace!(
            "server received {} from {}",
            packet.to_string(),
            client_id
                .map(|idx| format!("client {idx}"))
                .unwrap_or_else(|| addr.to_string())
        );
        match packet {
            Packet::Request(packet) => {
                self.process_connection_request(addr, packet, sender)?;
                Ok(None)
            },
            Packet::Response(packet) => self.process_connection_response(addr, packet, sender),
            Packet::KeepAlive(_) => {
                self.touch_client(client_id);
                Ok(None)
            }
            Packet::Payload(packet) => {
                self.touch_client(client_id);
                if let Some(idx) = client_id {
                    self.conn_cache.packet_queue.push((packet.buf, idx));
                }
                Ok(None)
            }
            Packet::Disconnect(_) => {
                if let Some(idx) = client_id {
                    debug!("server disconnected client {idx}");
                    self.on_disconnect(idx, addr);
                    self.conn_cache.remove(idx);
                    return Ok(Some(ConnectionUpdate::Disconnected(idx, addr)))
                }
                Ok(None)
            }
            _ => unreachable!("packet should have been filtered out by `ALLOWED_PACKETS`"),
        }
    }
    fn send_to_addr(
        &mut self,
        packet: Packet,
        key: Key,
        sender: &mut LinkSender,
    ) -> Result<()> {
        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet.write(self.writer.as_mut(), self.sequence, &key, self.protocol_id)?;
        self.writer.extend_from_slice(&buf[..size]);
        sender.push(self.writer.split());
        self.sequence += 1;
        Ok(())
    }
    fn send_to_client(
        &mut self,
        packet: Packet,
        id: ClientId,
        sender: &mut LinkSender,
    ) -> Result<()> {
        let conn = &mut self
            .conn_cache
            .clients
            .get_mut(&id)
            .ok_or(Error::ClientNotFound(id::PeerId::Netcode(id)))?;

        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet.write(&mut buf, conn.sequence, &conn.send_key, self.protocol_id)?;

        self.writer.extend_from_slice(&buf[..size]);
        sender.push(self.writer.split());

        conn.last_access_time = self.time;
        conn.last_send_time = self.time;
        conn.sequence += 1;
        Ok(())
    }

    fn process_connection_request(
        &mut self,
        from_addr: SocketAddr,
        mut packet: RequestPacket,
        sender: &mut LinkSender,
    ) -> Result<()> {
        let mut reader = io::Cursor::new(&mut packet.token_data[..]);
        let token = ConnectTokenPrivate::read_from(&mut reader)?;

        // TODO: this doesn't work with local hosts because the local bind_addr is often 0.0.0.0, even though
        //  the tokens contain 127.0.0.1
        // if !token
        //     .server_addresses
        //     .iter()
        //     .any(|(_, addr)| addr == self.local_addr())
        // {
        //     info!(
        //         token_addr = ?token.server_addresses,
        //         server_addr = ?self.local_addr(),
        //         "server ignored connection request. server address not in connect token whitelist"
        //     );
        //     return Ok(());
        // };
        if self
            .conn_cache
            .find_by_addr(&from_addr)
            .is_some_and(|(_, conn)| conn.is_connected())
        {
            return Err(Error::ClientAddressInUse(from_addr));
        };
        if self
            .conn_cache
            .find_by_id(token.client_id)
            .is_some_and(|conn| conn.is_connected())
        {
            return Err(Error::ClientIdInUse(id::PeerId::Netcode(token.client_id)));
        };
        let entry = TokenEntry {
            time: self.time,
            addr: from_addr,
            mac: packet.token_data
                [ConnectTokenPrivate::SIZE - MAC_BYTES..ConnectTokenPrivate::SIZE]
                .try_into()?,
        };
        if !self.token_entries.find_or_insert(entry) {
            return Err(Error::ConnectTokenInUse(id::PeerId::Netcode(
                token.client_id,
            )));
        };
        if self.num_connected_clients() >= MAX_CLIENTS {
            self.send_to_addr(
                DeniedPacket::create(DeniedReason::ServerFull),
                token.server_to_client_key,
                sender,
            )?;
            return Err(Error::ServerIsFull(id::PeerId::Netcode(token.client_id)));
        };
        if let Some(denied_reason) = self
            .cfg
            .connection_request_handler
            .handle_request(id::PeerId::Netcode(token.client_id))
        {
            self.send_to_addr(
                DeniedPacket::create(denied_reason),
                token.server_to_client_key,
                sender,
            )?;
            return Err(Error::Denied(id::PeerId::Netcode(token.client_id)));
        }

        let Ok(challenge_token_encrypted) = ChallengeToken {
            client_id: token.client_id,
            user_data: token.user_data,
        }
        .encrypt(self.challenge_sequence, &self.challenge_key) else {
            return Err(Error::ConnectTokenEncryptionFailure(id::PeerId::Netcode(
                token.client_id,
            )));
        };

        self.send_to_addr(
            ChallengePacket::create(self.challenge_sequence, challenge_token_encrypted),
            token.server_to_client_key,
            sender,
        )?;

        self.conn_cache.add(
            token.client_id,
            from_addr,
            token.timeout_seconds,
            token.server_to_client_key,
            token.client_to_server_key,
        );

        debug!("server sent connection challenge packet");
        self.challenge_sequence += 1;
        Ok(())
    }
    fn process_connection_response(
        &mut self,
        from_addr: SocketAddr,
        mut packet: ResponsePacket,
        sender: &mut LinkSender,
    ) -> Result<Option<ConnectionUpdate>> {
        let Ok(challenge_token) =
            ChallengeToken::decrypt(&mut packet.token, packet.sequence, &self.challenge_key)
        else {
            return Err(Error::ConnectTokenDecryptionFailure);
        };

        let id: ClientId = challenge_token.client_id;
        let Some(conn) = self.conn_cache.find_by_id(id) else {
            return Err(Error::UnknownClient(id::PeerId::Netcode(id)));
        };
        if conn.is_connected() {
            // TODO: most of the time this error can happen because we receive older 'ConnectionResponse' messages
            // even though the client is already connected. Should we just ignore this error?
            // return Err(Error::ClientIdInUse(id::ClientId::Netcode(id)));
            return Ok(None);
        };

        if self.num_connected_clients() >= MAX_CLIENTS {
            let send_key = self
                .conn_cache
                .clients
                .get(&id)
                .ok_or(Error::ClientNotFound(id::PeerId::Netcode(id)))?
                .send_key;

            self.send_to_addr(
                DeniedPacket::create(DeniedReason::ServerFull),
                send_key,
                sender,
            )?;
            return Err(Error::ServerIsFull(id::PeerId::Netcode(id)));
        }

        let client = self
            .conn_cache
            .clients
            .get_mut(&id)
            .ok_or(Error::ClientNotFound(id::PeerId::Netcode(id)))?;

        client.connect();
        client.last_send_time = self.time;
        client.last_receive_time = self.time;
        debug!(
            "server accepted client {} with id {}",
            id, challenge_token.client_id
        );
        self.send_to_client(KeepAlivePacket::create(id), id, sender)?;
        self.on_connect(id, from_addr);
        Ok(Some(ConnectionUpdate::Connected(id, from_addr)))
    }
    fn check_for_timeouts(&mut self) {
        for id in self.conn_cache.ids() {
            let Some(client) = self.conn_cache.clients.get_mut(&id) else {
                continue;
            };
            if !client.is_connected() {
                continue;
            }
            let addr = client.addr;
            if client.timeout.is_positive()
                && client.last_receive_time + (client.timeout as f64) < self.time
            {
                debug!("server timed out client {id}");
                self.on_disconnect(id, addr);
                self.conn_cache.remove(id);
            }
        }
    }
    fn send_keepalives(&mut self, sender: &mut LinkSender) -> Result<()> {
        for id in self.conn_cache.ids() {
            let Some(client) = self.conn_cache.clients.get_mut(&id) else {
                self.handle_client_error(Error::ClientNotFound(id::PeerId::Netcode(id)));
                continue;
            };
            if !client.is_connected() {
                continue;
            }
            if client.last_send_time + self.cfg.keep_alive_send_rate >= self.time {
                continue;
            }

            self.send_to_client(KeepAlivePacket::create(id), id, sender)?;
            trace!("server sent connection keep-alive packet to client {id}");
        }
        Ok(())
    }
    fn recv_packet(
        &mut self,
        buf: RecvPayload,
        now: u64,
        addr: SocketAddr,
        sender: &mut LinkSender,
    ) -> Result<Option<ConnectionUpdate>> {
        if buf.len() <= 1 {
            // Too small to be a packet
            return Ok(None);
        }
        let mut reader = io::Cursor::new(buf);
        let first_byte = reader.read_u8()?;
        reader.seek(io::SeekFrom::Current(-1))?;
        let (key, replay_protection) = match self.conn_cache.find_by_addr(&addr) {
            // Regardless of whether an entry in the connection cache exists for the client or not,
            // if the packet is a connection request we need to use the server's private key to decrypt it.
            _ if first_byte == Packet::REQUEST => (self.private_key, None),
            Some((client_id, _)) => (
                // If the packet is not a connection request, use the receive key to decrypt it.
                self.conn_cache
                    .clients
                    .get(&client_id)
                    .ok_or(Error::ClientNotFound(id::PeerId::Netcode(client_id)))?
                    .receive_key,
                self.conn_cache.replay_protection.get_mut(&client_id),
            ),
            None => {
                // Not a connection request packet, and not a known client, so ignore
                return Err(Error::Ignored(addr));
            }
        };

        let packet = Packet::read(
            reader.into_inner(),
            self.protocol_id,
            now,
            key,
            replay_protection,
            Self::ALLOWED_PACKETS,
        )?;

        self.process_packet(addr, packet, sender)
    }

    fn recv_packets(
        &mut self,
        remote_addr: SocketAddr,
        sender: &mut LinkSender,
        receiver: &mut LinkReceiver,
    ) -> Result<()> {
        let now = super::utils::now()?;

        // process every packet regardless of success/failure
        receiver.drain(..).for_each(|payload| {
            if let Err(e) = self.recv_packet(payload, now, remote_addr, sender) {
                self.handle_client_error(e);
            }
        });

        Ok(())
    }
    /// Updates the server.
    ///
    /// * Updates the server's elapsed time.
    /// * Receives and processes packets from clients, any received payload packets will be queued.
    /// * Sends keep-alive packets to connected clients.
    /// * Checks for timed out clients and disconnects them.
    ///
    /// This method should be called regularly, probably at a fixed rate (e.g., 60Hz).
    ///
    /// # Panics
    /// Panics if the server can't send or receive packets.
    /// For a non-panicking version, use [`try_update`](NetcodeServer::try_update).
    pub fn update(&mut self, delta_ms: f64, link: &mut Link) {
        self.try_update(delta_ms, link)
            .expect("send/recv error while updating server");
    }
    /// The fallible version of [`update`](NetcodeServer::update).
    ///
    /// Returns an error if the server can't send or receive packets.
    pub fn try_update(&mut self, delta_ms: f64, link: &mut Link) -> Result<Vec<Error>> {
        self.update_state(delta_ms);
        self.receive(link)
    }

    /// Updates the server state without receiving packets.
    pub fn update_state(&mut self, delta_ms: f64) {
        self.time += delta_ms;
        self.conn_cache.update(delta_ms);
        self.check_for_timeouts();
    }

    /// Receive packets from the links, process them.
    /// We might buffer some packets to the link as well (for Timeouts or ConnectionRequests, etc.)
    pub fn receive(&mut self, link: &mut Link) -> Result<Vec<Error>> {
        let remote_addr = link.remote_addr.expect("Netcode is only compatible\
        with links that have a remote address");
        let (sender, receiver) = (&mut link.send, &mut link.recv);
        self.recv_packets(remote_addr, sender, receiver)?;
        self.send_keepalives(sender)?;
        Ok(self.client_errors.drain(..).collect())
    }

    /// Receives a packet from a client, if one is available in the queue.
    ///
    /// The packet will be returned as a `Vec<u8>` along with the client index of the sender.
    ///
    /// If no packet is available, `None` will be returned.
    ///
    /// # Example
    /// ```
    /// # use std::net::{SocketAddr, Ipv4Addr};
    /// # use bevy::platform_support::time::Instant;
    /// # use lightyear_link::Link;
    /// # use lightyear_netcode::{NetcodeServer, MAX_PACKET_SIZE};
    /// # let protocol_id = 0x123456789ABCDEF0;
    /// # let private_key = [42u8; 32];
    /// # let mut server = NetcodeServer::new(protocol_id, private_key).unwrap();
    /// # let mut link = Link::new(SocketAddr::from(([127, 0, 0, 1], 12345)));
    /// #
    /// let start = Instant::now();
    /// loop {
    ///    let now = start.elapsed().as_secs_f64();
    ///    server.update(now, &mut link);
    ///    let mut packet_buf = [0u8; MAX_PACKET_SIZE];
    ///    while let Some((packet, from)) = server.recv() {
    ///        // ...
    ///    }
    ///    # break;
    /// }
    pub fn recv(&mut self) -> impl Iterator<Item=(RecvPayload, ClientId)> {
        self.conn_cache.packet_queue.drain(..)
    }

    pub(crate) fn buffer_send(&mut self, buf: SendPayload, client_id: ClientId) -> Result<()> {
        self.conn_cache.send_queue.push((buf, client_id));
        Ok(())
    }

    pub(crate) fn send_buffered(&mut self, sender: &mut LinkSender) -> Result<()> {
        self.conn_cache.send_queue.drain(..).try_for_each(|(buf, client_id)| {
            self.send(buf, client_id, sender)
        })
    }

    /// Sends a packet to a client.
    ///
    /// The provided buffer must be smaller than [`MAX_PACKET_SIZE`].
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub fn send(&mut self, buf: SendPayload, client_id: ClientId, sender: &mut LinkSender) -> Result<()> {
        if buf.len() > MAX_PACKET_SIZE {
            return Err(Error::SizeMismatch(MAX_PACKET_SIZE, buf.len()));
        }
        let Some(conn) = self.conn_cache.clients.get_mut(&client_id) else {
            return Err(Error::ClientNotFound(id::PeerId::Netcode(client_id)));
        };
        if !conn.is_connected() {
            // since there is no way to obtain a client index of clients that are not connected,
            // there is no straight-forward way for a user to send a packet to a non-connected client.
            // still, in case a user somehow manages to obtain such index, we'll return an error.
            return Err(Error::ClientNotConnected(id::PeerId::Netcode(client_id)));
        }
        if !conn.is_confirmed() {
            // send a keep-alive packet to the client to confirm the connection
            self.send_to_client(KeepAlivePacket::create(client_id), client_id, sender)?;
        }
        let packet = PayloadPacket::create(buf);
        self.send_to_client(packet, client_id, sender)
    }

    /// Sends a packet to all connected clients.
    ///
    /// The provided buffer must be smaller than [`MAX_PACKET_SIZE`].
    pub fn send_all(&mut self, buf: SendPayload, sender: &mut LinkSender) -> Result<()> {
        for id in self.conn_cache.ids() {
            match self.send(buf.clone(), id, sender) {
                Ok(_) | Err(Error::ClientNotConnected(_)) | Err(Error::ClientNotFound(_)) => {
                    continue
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// Creates a connect token builder for a given client ID.
    /// The builder can be used to configure the token with additional data before generating the final token.
    /// The `generate` method must be called on the builder to generate the final token.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use std::net::{SocketAddr, Ipv4Addr};
    /// # use std::str::FromStr;
    /// # use lightyear_netcode::{generate_key, NetcodeServer};
    ///  
    /// let private_key = generate_key();
    /// let protocol_id = 0x123456789ABCDEF0;
    /// let bind_addr = "0.0.0.0:0";
    /// let mut server = NetcodeServer::new(protocol_id, private_key).unwrap();
    ///
    /// let client_id = 123u64;
    /// let token = server.token(client_id, SocketAddr::from_str(bind_addr).unwrap())
    ///     .expire_seconds(60)  // defaults to 30 seconds, negative for no expiry
    ///     .timeout_seconds(-1) // defaults to 15 seconds, negative for no timeout
    ///     .generate()
    ///     .unwrap();
    /// ```
    ///
    /// See [`ConnectTokenBuilder`] for more options.
    pub fn token(
        &mut self,
        client_id: ClientId,
        server_addr: SocketAddr,
    ) -> ConnectTokenBuilder<SocketAddr> {
        let token_builder =
            ConnectToken::build(server_addr, self.protocol_id, client_id, self.private_key);
        self.token_sequence += 1;
        token_builder
    }

    /// Disconnects a client.
    ///
    /// The server will send a number of redundant disconnect packets to the client, and then remove its connection info.
    pub fn disconnect(&mut self, client_id: ClientId, sender: &mut LinkSender) -> Result<()> {
        let Some(conn) = self.conn_cache.clients.get_mut(&client_id) else {
            return Ok(());
        };
        if !conn.is_connected() {
            return Ok(());
        }
        let addr = conn.addr;
        debug!("server disconnecting client {client_id}");
        self.on_disconnect(client_id, addr);
        for _ in 0..self.cfg.num_disconnect_packets {
            // we do not use ? here because we want to continue even if the send fails
            let _ = self
                .send_to_client(DisconnectPacket::create(), client_id, sender)
                .inspect_err(|e| {
                    error!("server failed to send disconnect packet: {e}");
                });
        }
        self.conn_cache.remove(client_id);
        Ok(())
    }

    /// Disconnects a client.
    ///
    /// The server will send a number of redundant disconnect packets to the client, and then remove its connection info.
    pub(crate) fn disconnect_by_addr(&mut self, addr: SocketAddr, sender: &mut LinkSender) -> Result<()> {
        let Some(client_id) = self.conn_cache.client_id_map.get(&addr) else {
            return Err(Error::AddressNotFound(addr));
        };
        self.disconnect(*client_id, sender)
    }

    /// Disconnects all clients.
    pub fn disconnect_all(&mut self, sender: &mut LinkSender) -> Result<()> {
        debug!("Server preparing to disconnect all clients");
        for id in self.conn_cache.ids() {
            let Some(conn) = self.conn_cache.clients.get_mut(&id) else {
                warn!("Could not disconnect client {id:?} because the connection was not found");
                continue;
            };
            if conn.is_connected() {
                debug!("Server preparing to disconnect client {id:?}");
                self.disconnect(id, sender)?;
            }
        }
        Ok(())
    }

    pub fn connected_client_ids(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.conn_cache
            .clients
            .iter()
            .filter_map(|(id, c)| c.is_connected().then_some(id).copied())
    }

    pub fn client_ids(&self) -> impl Iterator<Item = ClientId> + '_ {
        self.conn_cache.clients.keys().copied()
    }

    /// Gets the number of connected clients.
    pub fn num_connected_clients(&self) -> usize {
        self.conn_cache
            .clients
            .iter()
            .filter(|(_, c)| c.is_connected())
            .count()
    }

    /// Gets the address of a client.
    pub fn client_addr(&self, client_id: ClientId) -> Option<SocketAddr> {
        self.conn_cache.clients.get(&client_id).map(|c| c.addr)
    }

    /// Gets the address of the server
    pub fn local_addr(&self) -> SocketAddr {
        self.cfg.server_addr
    }
}
