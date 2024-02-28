use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use bevy::prelude::Resource;
use tracing::{debug, error, trace};

use crate::connection::netcode::token::TOKEN_EXPIRE_SEC;
use crate::connection::server::NetServer;
use crate::serialize::reader::ReadBuffer;
use crate::serialize::wordbuffer::reader::ReadWordBuffer;
use crate::server::config::NetcodeConfig;
use crate::transport::io::Io;
use crate::transport::{PacketReceiver, PacketSender};

use super::{
    bytes::Bytes,
    crypto::{self, Key},
    error::{Error, Result},
    generate_key,
    packet::{
        ChallengePacket, DeniedPacket, DisconnectPacket, KeepAlivePacket, Packet, PayloadPacket,
        RequestPacket, ResponsePacket,
    },
    replay::ReplayProtection,
    token::{ChallengeToken, ConnectToken, ConnectTokenBuilder, ConnectTokenPrivate},
    MAC_BYTES, MAX_PACKET_SIZE, MAX_PKT_BUF_SIZE, PACKET_SEND_RATE_SEC,
};

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
///
/// Note that this is not the same as the [`ClientId`], which is used by the server to identify clients.
pub type ClientId = u64;

struct ConnectionCache {
    // this somewhat mimics the original C implementation,
    // the main difference being that `Connection` includes the encryption mapping as well.
    clients: HashMap<ClientId, Connection>,

    // map from client address to client id
    client_id_map: HashMap<SocketAddr, ClientId>,

    // we are not using a free-list here to not allocate memory up-front, since `ReplayProtection` is biggish (~2kb)
    replay_protection: HashMap<ClientId, ReplayProtection>,

    // packet queue for all clients
    packet_queue: VecDeque<(ReadWordBuffer, ClientId)>,

    // corresponds to the server time
    time: f64,
}

impl ConnectionCache {
    fn new(server_time: f64) -> Self {
        Self {
            clients: HashMap::with_capacity(MAX_CLIENTS),
            client_id_map: HashMap::with_capacity(MAX_CLIENTS),
            replay_protection: HashMap::with_capacity(MAX_CLIENTS),
            packet_queue: VecDeque::with_capacity(MAX_CLIENTS * 2),
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

pub type Callback<Ctx> = Box<dyn FnMut(ClientId, &mut Ctx) + Send + Sync + 'static>;

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
/// use crate::lightyear::connection::netcode::{NetcodeServer, ServerConfig};
///
/// let thread_safe_counter = Arc::new(Mutex::new(0));
/// let cfg = ServerConfig::with_context(thread_safe_counter).on_connect(|idx, ctx| {
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
    server_addr: SocketAddr,
    context: Ctx,
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
        F: FnMut(ClientId, &mut Ctx) + Send + Sync + 'static,
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
        F: FnMut(ClientId, &mut Ctx) + Send + Sync + 'static,
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
/// # use crate::lightyear::connection::netcode::{generate_key, NetcodeServer};
/// # use std::net::{SocketAddr, Ipv4Addr};
/// # use bevy::utils::{Instant, Duration};
/// # use std::thread;
/// # use lightyear::prelude::{Io, IoConfig, TransportConfig};
/// let mut io = Io::from_config(IoConfig::from_transport(TransportConfig::UdpSocket(
///    SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0)))
/// );
/// let private_key = generate_key();
/// let protocol_id = 0x123456789ABCDEF0;
/// let mut server = NetcodeServer::new(protocol_id, private_key).unwrap();
///
/// let start = Instant::now();
/// let tick_rate = Duration::from_secs_f64(1.0 / 60.0);
///
/// loop {
///     server.update(start.elapsed().as_secs_f64(), &mut io);
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
    cfg: ServerConfig<Ctx>,
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
    /// use crate::lightyear::connection::netcode::{generate_key, NetcodeServer, ServerConfig};
    ///
    /// let private_key = generate_key();
    /// let protocol_id = 0x123456789ABCDEF0;
    /// let cfg = ServerConfig::with_context(42).on_connect(|idx, ctx| {
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
        };
        // info!("server started on {}", server.addr());
        Ok(server)
    }
}

impl<Ctx> NetcodeServer<Ctx> {
    const ALLOWED_PACKETS: u8 = 1 << Packet::REQUEST
        | 1 << Packet::RESPONSE
        | 1 << Packet::KEEP_ALIVE
        | 1 << Packet::PAYLOAD
        | 1 << Packet::DISCONNECT;
    fn on_connect(&mut self, client_id: ClientId) {
        if let Some(cb) = self.cfg.on_connect.as_mut() {
            cb(client_id, &mut self.cfg.context)
        }
    }
    fn on_disconnect(&mut self, client_id: ClientId) {
        if let Some(cb) = self.cfg.on_disconnect.as_mut() {
            cb(client_id, &mut self.cfg.context)
        }
    }
    fn touch_client(&mut self, client_id: Option<ClientId>) -> Result<()> {
        let Some(id) = client_id else {
            return Ok(());
        };
        let Some(conn) = self.conn_cache.clients.get_mut(&id) else {
            return Ok(());
        };
        conn.last_receive_time = self.time;
        if !conn.is_confirmed() {
            debug!("server confirmed connection with client {id}");
            conn.confirm();
        }
        Ok(())
    }
    fn process_packet(
        &mut self,
        addr: SocketAddr,
        packet: Packet,
        sender: &mut impl PacketSender,
    ) -> Result<()> {
        let client_id = self.conn_cache.find_by_addr(&addr).map(|(id, _)| id);
        trace!(
            "server received {} from {}",
            packet.to_string(),
            client_id
                .map(|idx| format!("client {idx}"))
                .unwrap_or_else(|| addr.to_string())
        );
        match packet {
            Packet::Request(packet) => self.process_connection_request(addr, packet, sender),
            Packet::Response(packet) => self.process_connection_response(addr, packet, sender),
            Packet::KeepAlive(_) => self.touch_client(client_id),
            Packet::Payload(packet) => {
                self.touch_client(client_id)?;
                if let Some(idx) = client_id {
                    self.conn_cache
                        .packet_queue
                        .push_back((ReadWordBuffer::start_read(packet.buf), idx));
                }
                Ok(())
            }
            Packet::Disconnect(_) => {
                if let Some(idx) = client_id {
                    debug!("server disconnected client {idx}");
                    self.on_disconnect(idx);
                    self.conn_cache.remove(idx);
                }
                Ok(())
            }
            _ => unreachable!("packet should have been filtered out by `ALLOWED_PACKETS`"),
        }
    }

    fn remove_client(&mut self, addr: SocketAddr) {
        let client_id = self.conn_cache.find_by_addr(&addr).map(|(id, _)| id);
        if let Some(idx) = client_id {
            debug!("server disconnected client {idx}");
            self.on_disconnect(idx);
            self.conn_cache.remove(idx);
        }
    }

    fn send_to_addr(
        &mut self,
        packet: Packet,
        addr: SocketAddr,
        key: Key,
        sender: &mut impl PacketSender,
    ) -> Result<()> {
        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet.write(&mut buf, self.sequence, &key, self.protocol_id)?;
        sender.send(&buf[..size], &addr).map_err(Error::from)?;
        self.sequence += 1;
        Ok(())
    }
    fn send_to_client(
        &mut self,
        packet: Packet,
        id: ClientId,
        sender: &mut impl PacketSender,
    ) -> Result<()> {
        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let conn = &mut self
            .conn_cache
            .clients
            .get_mut(&id)
            .expect("invalid client id");
        let size = packet.write(&mut buf, conn.sequence, &conn.send_key, self.protocol_id)?;
        sender.send(&buf[..size], &conn.addr).map_err(Error::from)?;
        conn.last_access_time = self.time;
        conn.last_send_time = self.time;
        conn.sequence += 1;
        Ok(())
    }

    // /// A client is requesting a connection to the server via a secure io. Provide them with a ConnectToken
    // fn process_token_request(
    //     &mut self,
    //     from_addr: SocketAddr,
    //     sender: &mut impl PacketSender,
    // ) -> Result<()> {
    //     info!("Received token request from {}", from_addr);
    //     let client_id = self.conn_cache.new_id();
    //     let token = self
    //         .token(client_id, self.local_addr())
    //         .expire_seconds(self.cfg.token_expire_secs) // defaults to 30 seconds, negative for no expiry
    //         .timeout_seconds(self.cfg.client_timeout_secs) // defaults to 15 seconds, negative for no timeout
    //         .generate()
    //         .unwrap();
    //     let token_bytes = token.try_into_bytes()?;
    //
    //     // TODO: this is complete garbage, fix this.
    //
    //     // we can't send packets bigger than the MTU, so we will send one packet with 1000 bytes
    //     // and another packet with 1048 bytes
    //     let packet_1 = &token_bytes[..1000];
    //     let packet_2 = &token_bytes[1000..];
    //     sender.send(&packet_1, &from_addr).map_err(Error::from)?;
    //     sender.send(&packet_2, &from_addr).map_err(Error::from)?;
    //     info!("Generated a ConnectToken.");
    //     Ok(())
    // }

    fn process_connection_request(
        &mut self,
        from_addr: SocketAddr,
        mut packet: RequestPacket,
        sender: &mut impl PacketSender,
    ) -> Result<()> {
        let mut reader = std::io::Cursor::new(&mut packet.token_data[..]);
        let Ok(token) = ConnectTokenPrivate::read_from(&mut reader) else {
            debug!("server ignored connection request. failed to read connect token");
            return Ok(());
        };
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
            debug!("server ignored connection request. a client with this address is already connected");
            return Ok(());
        };
        if self
            .conn_cache
            .find_by_id(token.client_id)
            .is_some_and(|conn| conn.is_connected())
        {
            debug!("server ignored connection request. a client with this id is already connected");
            return Ok(());
        };
        let entry = TokenEntry {
            time: self.time,
            addr: from_addr,
            mac: packet.token_data
                [ConnectTokenPrivate::SIZE - MAC_BYTES..ConnectTokenPrivate::SIZE]
                .try_into()
                .expect("valid MAC size"),
        };
        if !self.token_entries.find_or_insert(entry) {
            debug!("server ignored connection request. connect token has already been used");
            return Ok(());
        };
        if self.num_connected_clients() >= MAX_CLIENTS {
            debug!("server denied connection request. server is full");
            self.send_to_addr(
                DeniedPacket::create(),
                from_addr,
                token.server_to_client_key,
                sender,
            )?;
            return Ok(());
        };
        self.conn_cache.add(
            token.client_id,
            from_addr,
            token.timeout_seconds,
            token.server_to_client_key,
            token.client_to_server_key,
        );
        let Ok(challenge_token_encrypted) = ChallengeToken {
            client_id: token.client_id,
            user_data: token.user_data,
        }
        .encrypt(self.challenge_sequence, &self.challenge_key) else {
            debug!("server ignored connection request. failed to encrypt challenge token");
            return Ok(());
        };
        self.send_to_addr(
            ChallengePacket::create(self.challenge_sequence, challenge_token_encrypted),
            from_addr,
            token.server_to_client_key,
            sender,
        )?;
        debug!("server sent connection challenge packet");
        self.challenge_sequence += 1;
        Ok(())
    }
    fn process_connection_response(
        &mut self,
        from_addr: SocketAddr,
        mut packet: ResponsePacket,
        sender: &mut impl PacketSender,
    ) -> Result<()> {
        let Ok(challenge_token) =
            ChallengeToken::decrypt(&mut packet.token, packet.sequence, &self.challenge_key)
        else {
            debug!("server ignored connection response. failed to decrypt challenge token");
            return Ok(());
        };
        let id: ClientId = challenge_token.client_id;
        let Some(conn) = self.conn_cache.find_by_id(id) else {
            debug!("server ignored connection response. no packet send key");
            return Ok(());
        };
        if conn.is_connected() {
            debug!("server ignored connection request. a client with this id is already connected");
            return Ok(());
        };

        if self.num_connected_clients() >= MAX_CLIENTS {
            debug!("server denied connection response. server is full");
            self.send_to_addr(
                DeniedPacket::create(),
                from_addr,
                self.conn_cache
                    .clients
                    .get(&id)
                    .expect("invalid client id")
                    .send_key,
                sender,
            )?;
            return Ok(());
        };
        let client = self
            .conn_cache
            .clients
            .get_mut(&id)
            .expect("invalid client id");
        client.connect();
        client.last_send_time = self.time;
        client.last_receive_time = self.time;
        debug!(
            "server accepted client {} with id {}",
            id, challenge_token.client_id
        );
        self.send_to_client(KeepAlivePacket::create(id), id, sender)?;
        self.on_connect(id);
        Ok(())
    }
    fn check_for_timeouts(&mut self) {
        for id in self.conn_cache.ids() {
            let Some(client) = self.conn_cache.clients.get_mut(&id) else {
                continue;
            };
            if !client.is_connected() {
                continue;
            }
            if client.timeout.is_positive()
                && client.last_receive_time + (client.timeout as f64) < self.time
            {
                debug!("server timed out client {id}");
                self.on_disconnect(id);
                self.conn_cache.remove(id);
            }
        }
    }
    fn send_packets(&mut self, io: &mut Io) -> Result<()> {
        for id in self.conn_cache.ids() {
            let Some(client) = self.conn_cache.clients.get_mut(&id) else {
                continue;
            };
            if !client.is_connected() {
                continue;
            }
            if client.last_send_time + self.cfg.keep_alive_send_rate >= self.time {
                continue;
            }

            self.send_to_client(KeepAlivePacket::create(id), id, io)?;
            trace!("server sent connection keep-alive packet to client {id}");
        }
        Ok(())
    }
    fn recv_packet(
        &mut self,
        buf: &mut [u8],
        now: u64,
        addr: SocketAddr,
        sender: &mut impl PacketSender,
    ) -> Result<()> {
        if buf.len() <= 1 {
            // TODO: make token request something else than this?
            // if buf == [u8::MAX].as_slice() {
            //     self.process_token_request(addr, sender)?;
            // }
            // Too small to be a packet
            return Ok(());
        }
        let (key, replay_protection) = match self.conn_cache.find_by_addr(&addr) {
            // Regardless of whether an entry in the connection cache exists for the client or not,
            // if the packet is a connection request we need to use the server's private key to decrypt it.
            _ if buf[0] == Packet::REQUEST => (self.private_key, None),
            Some((client_id, _)) => (
                // If the packet is not a connection request, use the receive key to decrypt it.
                self.conn_cache
                    .clients
                    .get(&client_id)
                    .expect("client id not found")
                    .receive_key,
                self.conn_cache.replay_protection.get_mut(&client_id),
            ),
            None => {
                // Not a connection request packet, and not a known client, so ignore
                debug!("server ignored non-connection-request packet from unknown address {addr}");
                return Ok(());
            }
        };
        let packet = match Packet::read(
            buf,
            self.protocol_id,
            now,
            key,
            replay_protection,
            Self::ALLOWED_PACKETS,
        ) {
            Ok(packet) => packet,
            Err(Error::Crypto(e)) => {
                debug!(error = ?e, "server ignored packet because it failed to decrypt.");
                return Ok(());
            }
            Err(e) => {
                error!("server ignored packet: {e}");
                return Ok(());
            }
        };
        self.process_packet(addr, packet, sender)
    }

    fn recv_packets(
        &mut self,
        sender: &mut impl PacketSender,
        receiver: &mut impl PacketReceiver,
    ) -> Result<()> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        loop {
            match receiver.recv() {
                Ok(Some((buf, addr))) => {
                    self.recv_packet(buf, now, addr, sender)?;
                }
                Err(e) => {
                    if e.kind() == std::io::ErrorKind::Other {
                        if let Some(e) = e.get_ref() {
                            if let Some(err) = e.downcast_ref::<super::DisConnectionError>() {
                                self.remove_client(err.addr);
                            }
                        }
                    }
                    break;
                }
                _ => {
                    break;
                }
            }
        }
        // while let Some((buf, addr)) = receiver.recv().map_err(Error::from)? {
        //     self.recv_packet(buf, now, addr, sender)?;
        // }
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
    pub fn update(&mut self, delta_ms: f64, io: &mut Io) {
        self.try_update(delta_ms, io)
            .expect("send/recv error while updating server")
    }
    /// The fallible version of [`update`](NetcodeServer::update).
    ///
    /// Returns an error if the server can't send or receive packets.
    pub fn try_update(&mut self, delta_ms: f64, io: &mut Io) -> Result<()> {
        self.time += delta_ms;
        self.conn_cache.update(delta_ms);
        let (sender, receiver) = io.split();
        self.check_for_timeouts();
        self.recv_packets(sender, receiver)?;
        self.send_packets(io)?;
        Ok(())
    }
    /// Receives a packet from a client, if one is available in the queue.
    ///
    /// The packet will be returned as a `Vec<u8>` along with the client index of the sender.
    ///
    /// If no packet is available, `None` will be returned.
    ///
    /// # Example
    /// ```
    /// # use crate::lightyear::connection::netcode::{NetcodeServer, ServerConfig, MAX_PACKET_SIZE};
    /// # use lightyear::prelude::{Io, IoConfig, TransportConfig};
    /// # use std::net::{SocketAddr, Ipv4Addr};
    /// # use bevy::utils::Instant;
    /// # let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    /// # let protocol_id = 0x123456789ABCDEF0;
    /// # let private_key = [42u8; 32];
    /// # let mut server = NetcodeServer::new(protocol_id, private_key).unwrap();
    /// # let mut io = Io::from_config(
    /// #     IoConfig::from_transport(TransportConfig::UdpSocket(addr))
    /// # );
    /// let start = Instant::now();
    /// loop {
    ///    let now = start.elapsed().as_secs_f64();
    ///    server.update(now, &mut io);
    ///    let mut packet_buf = [0u8; MAX_PACKET_SIZE];
    ///    while let Some((packet, from)) = server.recv() {
    ///        // ...
    ///    }
    ///    # break;
    /// }
    pub fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)> {
        self.conn_cache.packet_queue.pop_front()
    }
    /// Sends a packet to a client.
    ///
    /// The provided buffer must be smaller than [`MAX_PACKET_SIZE`].
    pub fn send(&mut self, buf: &[u8], client_id: ClientId, io: &mut Io) -> Result<()> {
        if buf.len() > MAX_PACKET_SIZE {
            return Err(Error::SizeMismatch(MAX_PACKET_SIZE, buf.len()));
        }
        let Some(conn) = self.conn_cache.clients.get_mut(&client_id) else {
            return Err(Error::ClientNotFound);
        };
        if !conn.is_connected() {
            // since there is no way to obtain a client index of clients that are not connected,
            // there is no straight-forward way for a user to send a packet to a non-connected client.
            // still, in case a user somehow manages to obtain such index, we'll return an error.
            return Err(Error::ClientNotConnected);
        }
        if !conn.is_confirmed() {
            // send a keep-alive packet to the client to confirm the connection
            self.send_to_client(KeepAlivePacket::create(client_id), client_id, io)?;
        }
        let packet = PayloadPacket::create(buf);
        self.send_to_client(packet, client_id, io)
    }

    /// Sends a packet to all connected clients.
    ///
    /// The provided buffer must be smaller than [`MAX_PACKET_SIZE`].
    pub fn send_all(&mut self, buf: &[u8], io: &mut Io) -> Result<()> {
        for id in self.conn_cache.ids() {
            match self.send(buf, id, io) {
                Ok(_) | Err(Error::ClientNotConnected) | Err(Error::ClientNotFound) => continue,
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
    /// ```
    /// # use crate::lightyear::connection::netcode::{generate_key, NetcodeServer, ServerConfig};
    /// # use std::net::{SocketAddr, Ipv4Addr};
    /// # use std::str::FromStr;
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
    pub fn disconnect(&mut self, client_id: ClientId, io: &mut Io) -> Result<()> {
        let Some(conn) = self.conn_cache.clients.get_mut(&client_id) else {
            return Ok(());
        };
        if !conn.is_connected() {
            return Ok(());
        }
        debug!("server disconnecting client {client_id}");
        for _ in 0..self.cfg.num_disconnect_packets {
            self.send_to_client(DisconnectPacket::create(), client_id, io)?;
        }
        self.on_disconnect(client_id);
        self.conn_cache.remove(client_id);
        Ok(())
    }
    /// Disconnects all clients.
    pub fn disconnect_all(&mut self, io: &mut Io) -> Result<()> {
        debug!("server disconnecting all clients");
        for id in self.conn_cache.ids() {
            let Some(conn) = self.conn_cache.clients.get_mut(&id) else {
                continue;
            };
            if conn.is_connected() {
                self.disconnect(id, io)?;
            }
        }
        Ok(())
    }

    pub fn connected_client_ids(&self) -> Vec<ClientId> {
        self.conn_cache
            .clients
            .iter()
            .filter_map(|(id, c)| c.is_connected().then_some(id))
            .cloned()
            .collect()
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

#[derive(Default)]
pub(crate) struct NetcodeServerContext {
    pub(crate) connections: Vec<ClientId>,
    pub(crate) disconnections: Vec<ClientId>,
}

#[derive(Resource)]
pub struct Server {
    server: NetcodeServer<NetcodeServerContext>,
    io: Io,
}

impl NetServer for Server {
    fn start(&mut self) {}

    fn connected_client_ids(&self) -> Vec<ClientId> {
        self.server.connected_client_ids()
    }

    fn try_update(&mut self, delta_ms: f64) -> anyhow::Result<()> {
        // reset the new connections/disconnections
        self.server.cfg.context.connections.clear();
        self.server.cfg.context.disconnections.clear();

        self.server
            .try_update(delta_ms, &mut self.io)
            .context("could not update server")
    }

    fn recv(&mut self) -> Option<(ReadWordBuffer, ClientId)> {
        self.server.recv()
    }

    fn send(&mut self, buf: &[u8], client_id: ClientId) -> anyhow::Result<()> {
        self.server
            .send(buf, client_id, &mut self.io)
            .context("could not send packet")
    }

    fn new_connections(&self) -> Vec<ClientId> {
        self.server.cfg.context.connections.clone()
    }

    fn new_disconnections(&self) -> Vec<ClientId> {
        self.server.cfg.context.disconnections.clone()
    }

    fn io(&self) -> &Io {
        &self.io
    }
}

impl Server {
    pub(crate) fn new(config: NetcodeConfig, io: Io) -> Self {
        let private_key = config.private_key.unwrap_or(generate_key());
        // create context
        let context = NetcodeServerContext::default();
        let mut cfg = ServerConfig::with_context(context)
            .on_connect(|id, ctx| {
                ctx.connections.push(id);
            })
            .on_disconnect(|id, ctx| {
                ctx.disconnections.push(id);
            });
        cfg = cfg.keep_alive_send_rate(config.keep_alive_send_rate);
        cfg = cfg.num_disconnect_packets(config.num_disconnect_packets);
        let server = NetcodeServer::with_config(config.protocol_id, private_key, cfg)
            .expect("Could not create server netcode");

        Self { server, io }
    }
}
