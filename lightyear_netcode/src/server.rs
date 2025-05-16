use alloc::sync::Arc;
#[cfg(not(feature = "std"))]
use alloc::{boxed::Box, vec, vec::Vec};
use bevy::platform::collections::HashMap;
use bevy::prelude::{Entity, EntityCommands};
use core::net::SocketAddr;
use no_std_io2::io;
use tracing::{debug, error, trace, warn};

use super::{
    bytes::Bytes, crypto::{self, Key}, error::{Error, Result}, packet::{
        ChallengePacket, DeniedPacket, DisconnectPacket, KeepAlivePacket, Packet, PayloadPacket,
        RequestPacket, ResponsePacket,
    }, replay::ReplayProtection,
    token::{ChallengeToken, ConnectToken, ConnectTokenBuilder, ConnectTokenPrivate},
    ClientId,
    MAC_BYTES,
    MAX_PACKET_SIZE,
    MAX_PKT_BUF_SIZE,
    PACKET_SEND_RATE_SEC,
};
use crate::token::TOKEN_EXPIRE_SEC;
use lightyear_connection::prelude::Connecting;
use lightyear_connection::shared::{
    ConnectionRequestHandler, DefaultConnectionRequestHandler, DeniedReason,
};
use lightyear_core::id;
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
    entity: Entity,
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
            // TODO: how do we do this if we don't have access to the remote addr?
            // // Allow reusing tokens only if the entity matches
            // self.inner[matching].entity == entry.entity
            true
        } else {
            // If there is no matching entry, replace the oldest one
            self.inner[oldest] = entry;
            true
        }
    }
}

#[derive(Debug, Clone)]
struct Connection {
    confirmed: bool,
    connected: bool,
    client_id: ClientId,
    entity: Entity,
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

struct ConnectionCache {
    // this somewhat mimics the original C implementation,
    // the main difference being that `Connection` includes the encryption mapping as well.
    clients: HashMap<ClientId, Connection>,

    // map from client entity to client id
    client_id_map: HashMap<Entity, ClientId>,

    // we are not using a free-list here to not allocate memory up-front, since `ReplayProtection` is biggish (~2kb)
    replay_protection: HashMap<ClientId, ReplayProtection>,

    // corresponds to the server time
    time: f64,
}

impl ConnectionCache {
    fn new(server_time: f64) -> Self {
        Self {
            clients: HashMap::default(),
            client_id_map: HashMap::default(),
            replay_protection: HashMap::default(),
            time: server_time,
        }
    }
    fn add(
        &mut self,
        client_id: ClientId,
        entity: Entity,
        timeout: i32,
        send_key: Key,
        receive_key: Key,
    ) {
        let time = self.time;
        if let Some(existing) = self.mut_by_entity(&entity) {
            existing.client_id = client_id;
            existing.timeout = timeout;
            existing.send_key = send_key;
            existing.receive_key = receive_key;
            existing.last_access_time = time;
            return;
        }
        let conn = Connection {
            confirmed: false,
            connected: false,
            client_id,
            entity,
            timeout,
            last_access_time: time,
            last_send_time: f64::NEG_INFINITY,
            last_receive_time: f64::NEG_INFINITY,
            send_key,
            receive_key,
            sequence: 1 << 62,
        };
        self.clients.insert(client_id, conn);
        self.replay_protection
            .insert(client_id, ReplayProtection::new());

        self.client_id_map.insert(entity, client_id);
    }
    fn remove(&mut self, client_id: ClientId) {
        let Some(conn) = self.clients.get(&client_id) else {
            return;
        };
        if !conn.is_connected() {
            return;
        }
        self.client_id_map.remove(&conn.entity);
        self.replay_protection.remove(&client_id);
        self.clients.remove(&client_id);
    }

    fn ids(&self) -> Vec<ClientId> {
        self.clients.keys().cloned().collect()
    }

    fn find_by_entity(&self, entity: &Entity) -> Option<&Connection> {
        self.client_id_map
            .get(entity)
            .and_then(|id| self.clients.get(id))
    }
    fn mut_by_entity(&mut self, entity: &Entity) -> Option<&mut Connection> {
        self.client_id_map
            .get(entity)
            .and_then(|id| self.clients.get_mut(id))
    }

    fn find_by_id(&self, client_id: ClientId) -> Option<&Connection> {
        self.clients.get(&client_id)
    }
    fn mut_by_id(&mut self, client_id: ClientId) -> Option<&mut Connection> {
        self.clients.get_mut(&client_id)
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

pub type Callback<Ctx> = Box<dyn FnMut(ClientId, Entity, &mut Ctx) + Send + Sync + 'static>;

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
/// use lightyear_netcode::{Server, ServerConfig};
///
/// let thread_safe_counter = Arc::new(Mutex::new(0));
/// let cfg = ServerConfig::with_context(thread_safe_counter).on_connect(|idx, _, ctx| {
///     let mut counter = ctx.lock().unwrap();
///     *counter += 1;
///     println!("client {} connected, counter: {idx}", counter);
/// });
/// let server = Server::with_config(protocol_id, private_key, cfg).unwrap();
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
        F: FnMut(ClientId, Entity, &mut Ctx) + Send + Sync + 'static,
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
        F: FnMut(ClientId, Entity, &mut Ctx) + Send + Sync + 'static,
    {
        self.on_disconnect = Some(Box::new(cb));
        self
    }
}

/// The `netcode` server.
///
/// Responsible for accepting connections from clients and communicating with them using the netcode protocol.
/// The server should be run in a loop to process incoming packets, send updates to clients, and maintain stable connections.
pub struct Server<Ctx = ()> {
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
    // We cannot mix the netcode packets and the user's payload packets to send, so
    // we will temporarily buffer them here
    send_queue: HashMap<Entity, Vec<SendPayload>>,
    // We use a Writer (wrapper around BytesMut) here because we will keep re-using the
    // same allocation for the bytes we send.
    // 1. We create an array on the stack of size MAX_PACKET_SIZE
    // 2. We copy the serialized array in the writer via `extend_from_size`
    // 3. We split the bytes off, to recover the allocation
    writer: Writer,
    client_errors: Vec<Error>,
}

impl Server {
    /// Create a new server with a default configuration.
    ///
    /// For a custom configuration, use [`Server::with_config`](Server::with_config) instead.
    pub fn new(protocol_id: u64, private_key: Key) -> Result<Self> {
        let server: Server<()> = Server {
            time: 0.0,
            private_key,
            protocol_id,
            sequence: 1 << 23,
            token_sequence: 0,
            challenge_sequence: 0,
            challenge_key: crypto::generate_key(),
            conn_cache: ConnectionCache::new(0.0),
            token_entries: TokenEntries::new(),
            cfg: ServerConfig::default(),
            send_queue: HashMap::default(),
            writer: Writer::with_capacity(MAX_PKT_BUF_SIZE),
            client_errors: vec![],
        };
        // info!("server started on {}", server.io.local_addr());
        Ok(server)
    }
}

impl<Ctx> Server<Ctx> {
    /// Create a new server with a custom configuration. <br>
    /// Callbacks with context can be registered with the server to be notified when the server changes states. <br>
    /// See [`ServerConfig`] for more details.
    ///
    /// # Example
    /// ```
    /// # use lightyear_netcode::{generate_key, Server, ServerConfig};
    ///
    /// let private_key = generate_key();
    /// let protocol_id = 0x123456789ABCDEF0;
    /// let cfg = ServerConfig::with_context(42).on_connect(|idx, _, ctx| {
    ///     assert_eq!(ctx, &42);
    /// });
    /// let server = Server::with_config(protocol_id, private_key, cfg).unwrap();
    /// ```
    pub fn with_config(protocol_id: u64, private_key: Key, cfg: ServerConfig<Ctx>) -> Result<Self> {
        let server = Server {
            time: 0.0,
            private_key,
            protocol_id,
            sequence: 1 << 23,
            token_sequence: 0,
            challenge_sequence: 0,
            challenge_key: crypto::generate_key(),
            conn_cache: ConnectionCache::new(0.0),
            token_entries: TokenEntries::new(),
            cfg,
            send_queue: HashMap::default(),
            writer: Writer::with_capacity(MAX_PKT_BUF_SIZE),
            client_errors: vec![],
        };
        // info!("server started on {}", server.addr());
        Ok(server)
    }
}

impl<Ctx> Server<Ctx> {
    const ALLOWED_PACKETS: u8 = 1 << Packet::REQUEST
        | 1 << Packet::RESPONSE
        | 1 << Packet::KEEP_ALIVE
        | 1 << Packet::PAYLOAD
        | 1 << Packet::DISCONNECT;
    fn on_connect(&mut self, client_id: ClientId, entity: Entity) {
        if let Some(cb) = self.cfg.on_connect.as_mut() {
            cb(client_id, entity, &mut self.cfg.context)
        }
    }
    fn on_disconnect(&mut self, client_id: ClientId, entity: Entity) {
        if let Some(cb) = self.cfg.on_disconnect.as_mut() {
            cb(client_id, entity, &mut self.cfg.context)
        }
    }
    fn handle_client_error(&mut self, error: Error) {
        self.client_errors.push(error);
    }
    fn touch_client(&mut self, client_id: ClientId) {
        if let Some(conn) = self.conn_cache.mut_by_id(client_id) {
            conn.last_receive_time = self.time;
            if !conn.is_confirmed() {
                debug!("server confirmed connection with client {client_id}");
                conn.confirm();
            }
        }
    }
    fn process_packet(
        &mut self,
        packet: Packet,
        entity_mut: &mut EntityCommands,
    ) -> Result<Option<RecvPayload>> {
        let entity = entity_mut.id();
        match packet {
            Packet::Request(packet) => {
                self.process_connection_request(packet, entity_mut)?;
                Ok(None)
            }
            Packet::Response(packet) => {
                self.process_connection_response(packet, entity)?;
                Ok(None)
            }
            Packet::KeepAlive(_) => {
                if let Some(client_id) =
                    self.conn_cache.find_by_entity(&entity).map(|c| c.client_id)
                {
                    self.touch_client(client_id);
                }
                Ok(None)
            }
            Packet::Payload(packet) => {
                if let Some(client_id) =
                    self.conn_cache.find_by_entity(&entity).map(|c| c.client_id)
                {
                    self.touch_client(client_id);
                    Ok(Some(packet.buf))
                } else {
                    Ok(None)
                }
            }
            Packet::Disconnect(_) => {
                if let Some(idx) = self.conn_cache.find_by_entity(&entity).map(|c| c.client_id) {
                    debug!("server disconnected client {idx}");
                    self.on_disconnect(idx, entity);
                    self.conn_cache.remove(idx);
                }
                Ok(None)
            }
            _ => unreachable!("packet should have been filtered out by `ALLOWED_PACKETS`"),
        }
    }
    fn send_netcode_packet(&mut self, packet: Packet, key: Key, entity: Entity) -> Result<()> {
        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet.write(&mut buf, self.sequence, &key, self.protocol_id)?;
        self.writer.extend_from_slice(&buf[..size]);
        self.send_queue
            .entry(entity)
            .or_default()
            .push(self.writer.split());
        self.sequence += 1;
        Ok(())
    }
    fn send_to_addr(&mut self, packet: Packet, key: Key, sender: &mut LinkSender) -> Result<()> {
        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet.write(&mut buf, self.sequence, &key, self.protocol_id)?;
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

    fn send_netcode_to_client(
        &mut self,
        packet: Packet,
        id: ClientId,
        entity: Entity,
    ) -> Result<()> {
        let conn = &mut self
            .conn_cache
            .clients
            .get_mut(&id)
            .ok_or(Error::ClientNotFound(id::PeerId::Netcode(id)))?;

        let mut buf = [0u8; MAX_PKT_BUF_SIZE];
        let size = packet.write(&mut buf, conn.sequence, &conn.send_key, self.protocol_id)?;
        self.writer.extend_from_slice(&buf[..size]);
        self.send_queue
            .entry(entity)
            .or_default()
            .push(self.writer.split());

        conn.last_access_time = self.time;
        conn.last_send_time = self.time;
        conn.sequence += 1;
        Ok(())
    }

    fn process_connection_request(
        &mut self,
        mut packet: RequestPacket,
        entity_mut: &mut EntityCommands,
    ) -> Result<()> {
        trace!("Server received connection request packet");
        let mut reader = io::Cursor::new(&mut packet.token_data[..]);
        let token = ConnectTokenPrivate::read_from(&mut reader)?;
        let entity = entity_mut.id();

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
            .find_by_entity(&entity)
            .is_some_and(|conn| conn.is_connected())
        {
            return Err(Error::ClientEntityInUse(entity));
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
            entity,
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
            self.send_netcode_packet(
                DeniedPacket::create(DeniedReason::ServerFull),
                token.server_to_client_key,
                entity,
            )?;
            return Err(Error::ServerIsFull(id::PeerId::Netcode(token.client_id)));
        };
        if let Some(denied_reason) = self
            .cfg
            .connection_request_handler
            .handle_request(id::PeerId::Netcode(token.client_id))
        {
            self.send_netcode_packet(
                DeniedPacket::create(denied_reason),
                token.server_to_client_key,
                entity,
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

        self.send_netcode_packet(
            ChallengePacket::create(self.challenge_sequence, challenge_token_encrypted),
            token.server_to_client_key,
            entity,
        )?;

        self.conn_cache.add(
            token.client_id,
            entity,
            token.timeout_seconds,
            token.server_to_client_key,
            token.client_to_server_key,
        );

        entity_mut.insert(Connecting);

        debug!("server sent connection challenge packet");
        self.challenge_sequence += 1;
        Ok(())
    }

    fn process_connection_response(
        &mut self,
        mut packet: ResponsePacket,
        entity: Entity,
    ) -> Result<()> {
        let Ok(challenge_token) =
            ChallengeToken::decrypt(&mut packet.token, packet.sequence, &self.challenge_key)
        else {
            return Err(Error::ConnectTokenDecryptionFailure);
        };

        let id: ClientId = challenge_token.client_id;
        // avoid borrow-checker by directly using `conn_cache.clients`
        let Some(client) = self.conn_cache.clients.get(&id) else {
            return Err(Error::UnknownClient(id::PeerId::Netcode(id)));
        };
        if client.is_connected() {
            // TODO: most of the time this error can happen because we receive older 'ConnectionResponse' messages
            // even though the client is already connected. Should we just ignore this error?
            // return Err(Error::ClientIdInUse(id::ClientId::Netcode(id)));
            return Ok(());
        };

        if self.num_connected_clients() >= MAX_CLIENTS {
            self.send_netcode_packet(
                DeniedPacket::create(DeniedReason::ServerFull),
                client.send_key,
                entity,
            )?;
            return Err(Error::ServerIsFull(id::PeerId::Netcode(id)));
        }

        let client = self.conn_cache.clients.get_mut(&id).unwrap();

        client.connect();
        client.last_send_time = self.time;
        client.last_receive_time = self.time;
        debug!(
            "server accepted client {} with id {}",
            id, challenge_token.client_id
        );
        self.send_netcode_to_client(KeepAlivePacket::create(id), id, entity)?;
        self.on_connect(id, entity);
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
            let entity = client.entity;
            if client.timeout.is_positive()
                && client.last_receive_time + (client.timeout as f64) < self.time
            {
                debug!("server timed out client {id}");
                self.on_disconnect(id, entity);
                self.conn_cache.remove(id);
            }
        }
    }

    // fn send_keepalives(&mut self, sender: &mut LinkSender) -> Result<()> {
    //     for id in self.conn_cache.ids() {
    //         let Some(client) = self.conn_cache.clients.get_mut(&id) else {
    //             self.handle_client_error(Error::ClientNotFound(id::PeerId::Netcode(id)));
    //             continue;
    //         };
    //         if !client.is_connected() {
    //             continue;
    //         }
    //         if client.last_send_time + self.cfg.keep_alive_send_rate >= self.time {
    //             continue;
    //         }
    //
    //         self.send_to_client(KeepAlivePacket::create(id), id, sender)?;
    //         trace!("server sent connection keep-alive packet to client {id}");
    //     }
    //     Ok(())
    // }

    /// Send keep-alives to a given client
    pub(crate) fn send_keepalives(&mut self, id: ClientId, sender: &mut LinkSender) -> Result<()> {
        let Some(client) = self.conn_cache.clients.get_mut(&id) else {
            return Err(Error::ClientNotFound(id::PeerId::Netcode(id)));
        };
        if !client.is_connected() {
            return Ok(());
        }
        if client.last_send_time + self.cfg.keep_alive_send_rate >= self.time {
            return Ok(());
        }
        self.send_to_client(KeepAlivePacket::create(id), id, sender)?;
        trace!("server sent connection keep-alive packet to client {id}");
        Ok(())
    }

    pub(crate) fn send_netcode_packets(&mut self, entity: Entity, sender: &mut LinkSender) {
        if let Some(queue) = self.send_queue.get_mut(&entity) { queue.drain(..).for_each(|send_payload| {
                trace!("server sending netcode packet");
                sender.push(send_payload);
            }); }
    }

    fn recv_packet(
        &mut self,
        buf: RecvPayload,
        now: u64,
        entity_mut: &mut EntityCommands,
    ) -> Result<Option<RecvPayload>> {
        if buf.len() <= 1 {
            // Too small to be a packet
            return Ok(None);
        }
        let mut reader = io::Cursor::new(buf);
        let first_byte = reader.read_u8()?;
        let entity = entity_mut.id();
        // reader.rewind()?;
        let (key, replay_protection) = match self.conn_cache.find_by_entity(&entity) {
            // Regardless of whether an entry in the connection cache exists for the client or not,
            // if the packet is a connection request we need to use the server's private key to decrypt it.
            _ if first_byte == Packet::REQUEST => (self.private_key, None),
            Some(c) => {
                let client_id = c.client_id;
                (
                    // If the packet is not a connection request, use the receive key to decrypt it.
                    self.conn_cache
                        .clients
                        .get(&client_id)
                        .ok_or(Error::ClientNotFound(id::PeerId::Netcode(client_id)))?
                        .receive_key,
                    self.conn_cache.replay_protection.get_mut(&client_id),
                )
            }
            None => {
                // Not a connection request packet, and not a known client, so ignore
                return Err(Error::Ignored(entity));
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

        self.process_packet(packet, entity_mut)
    }

    fn recv_packets(
        &mut self,
        receiver: &mut LinkReceiver,
        entity_mut: &mut EntityCommands,
    ) -> Result<()> {
        let now = super::utils::now()?;

        // we pop every packet that is currently in the receiver, then we process them
        // Processing them might mean that we're re-adding them to the receiver so that
        // the Transport can read them later
        for _ in 0..receiver.len() {
            if let Some(recv_packet) = receiver.pop() {
                match self.recv_packet(recv_packet, now, entity_mut) {
                    Ok(Some(payload)) => receiver.push_raw(payload),
                    Err(e) => self.handle_client_error(e),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Updates the server state without receiving packets.
    pub fn update_state(&mut self, delta_ms: f64) {
        self.time += delta_ms;
        self.conn_cache.update(delta_ms);
        self.check_for_timeouts();
    }

    /// Receive packets from the links, process them.
    /// We might buffer some packets to the link as well (for Timeouts or ConnectionRequests, etc.)
    pub fn receive(
        &mut self,
        link: &mut Link,
        entity_mut: &mut EntityCommands,
    ) -> Result<Vec<Error>> {
        self.recv_packets(&mut link.recv, entity_mut)?;
        Ok(self.client_errors.drain(..).collect())
    }

    /// Sends a packet to a client.
    ///
    /// The provided buffer must be smaller than [`MAX_PACKET_SIZE`].
    #[cfg_attr(feature = "trace", instrument(level = Level::INFO, skip_all))]
    pub fn send(
        &mut self,
        buf: SendPayload,
        client_id: ClientId,
        sender: &mut LinkSender,
    ) -> Result<()> {
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
                    continue;
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
    /// # use lightyear_netcode::{generate_key, Server};
    ///
    /// let private_key = generate_key();
    /// let protocol_id = 0x123456789ABCDEF0;
    /// let bind_addr = "0.0.0.0:0";
    /// let mut server = Server::new(protocol_id, private_key).unwrap();
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
        let entity = conn.entity;
        debug!("server disconnecting client {client_id}");
        self.on_disconnect(client_id, entity);
        for _ in 0..self.cfg.num_disconnect_packets {
            // we do not use ? here because we want to continue even if the send fails
            let _ = self
                .send_to_client(DisconnectPacket::create(), client_id, sender)
                .inspect_err(|e| {
                    error!("server failed to send disconnect packet: {e}");
                });
        }
        self.conn_cache.remove(client_id);
        self.send_queue.remove(&entity);
        Ok(())
    }

    /// Disconnects a client.
    ///
    /// The server will send a number of redundant disconnect packets to the client, and then remove its connection info.
    pub(crate) fn disconnect_by_entity(
        &mut self,
        entity: Entity,
        sender: &mut LinkSender,
    ) -> Result<()> {
        let Some(client_id) = self.conn_cache.client_id_map.get(&entity) else {
            return Err(Error::EntityNotFound(entity));
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

    /// Gets the entity of a client.
    pub fn client_entity(&self, client_id: ClientId) -> Option<Entity> {
        self.conn_cache.clients.get(&client_id).map(|c| c.entity)
    }

    /// Gets the address of the server
    pub fn local_addr(&self) -> SocketAddr {
        self.cfg.server_addr
    }
}
