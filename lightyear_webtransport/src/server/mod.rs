//! See [`WebTransportServer`].

mod backend;

use aeronet_io::connection::Disconnect;
use bevy_ecs::error::CommandWithEntity;
use lightyear_connection::client_of::ClientOf;
use lightyear_link::prelude::{LinkOf, ServerLink};
use lightyear_link::{Link, LinkSet, LinkStart, Linked, Linking, RecvPayload, SendPayload, Unlink, Unlinked};
use tracing::info;
use wtransport::Identity;
use {
    crate::{
        runtime::WebTransportRuntime,
        session::{
            self, SessionError, SessionMeta, WebTransportIo, WebTransportSessionPlugin, MIN_MTU,
        },
    },
    aeronet_io::{
        connection::{Disconnected, LocalAddr, PeerAddr}, packet::{PacketRtt, RecvPacket}, server::{Closed, Server, ServerEndpoint},
        IoSet,
        Session,
        SessionEndpoint,
    },
    bevy_app::prelude::*,
    bevy_ecs::{prelude::*, system::EntityCommand},
    bevy_platform::time::Instant,
    bevy_reflect::prelude::*,
    bytes::Bytes,
    core::{any::type_name, mem, net::SocketAddr, time::Duration},
    derive_more::{Display, Error},
    futures::channel::{mpsc, oneshot},
    tracing::{debug, debug_span, warn, Instrument},
    wtransport::error::ConnectionError,
};

/// Allows using [`WebTransportServer`].
pub struct WebTransportServerPlugin;

impl Plugin for WebTransportServerPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<WebTransportSessionPlugin>() {
            app.add_plugins(WebTransportSessionPlugin);
        }

        app.register_type::<SessionRequest>()
            .register_type::<SessionResponse>()
            .add_observer(WebTransportServer::link)
            .add_systems(
                PreUpdate,
                (poll_opening, poll_opened, poll_connecting, poll_connected)
                    .in_set(LinkSet::Receive)
                    .before(session::receive),
            );
    }
}

/// WebTransport server implementation which listens for client connections,
/// and coordinates messaging between multiple clients.
///
/// Use [`WebTransportServer::open`] to start opening a server.
///
/// When a client attempts to connect, the server will trigger a
/// [`SessionRequest`]. Your app **must** observe this, and use
/// [`SessionRequest::respond`] to set how the server should respond to this
/// connection attempt.
#[derive(Debug, Component)]
#[require(ServerLink)]
pub struct WebTransportServer {
    pub server_addr: SocketAddr,
    pub certificate: Identity,
}

/// Configuration for the [`WebTransportServer`].
pub type ServerConfig = wtransport::ServerConfig;

impl WebTransportServer {
    /// Creates an [`EntityCommand`] to set up a server and have it start
    /// listening for connections.
    ///
    /// # Examples
    ///
    /// ```
    /// use {
    ///     aeronet_webtransport::server::{ServerConfig, WebTransportServer},
    ///     bevy_ecs::{prelude::*, system::EntityCommand},
    /// };
    ///
    /// # fn run(mut commands: Commands, world: &mut World) {
    /// // set up a self-signed certificate to identify this server
    /// let identity = wtransport::Identity::self_signed(["localhost", "127.0.0.1", "::1"]).unwrap();
    ///
    /// let config = ServerConfig::builder()
    ///     .with_bind_default(12345) // server port
    ///     .with_identity(identity)
    ///     .build();
    ///
    /// // using `Commands`
    /// commands
    ///     .spawn_empty()
    ///     .queue(WebTransportServer::open(config));
    ///
    /// // using mutable `World` access
    /// # let config: ServerConfig = unimplemented!();
    /// let server = world.spawn_empty().id();
    /// WebTransportServer::open(config).apply(world.entity_mut(server));
    /// # }
    /// ```
    #[must_use]
    pub fn open(config: impl Into<ServerConfig>) -> impl EntityCommand {
        let config = config.into();
        move |entity: EntityWorldMut| open(entity, config)
    }

    fn link(
        trigger: Trigger<LinkStart>,
        mut query: Query<(Entity, &mut WebTransportServer), With<Unlinked>>,
        mut commands: Commands,
    ) {
        if let Ok((entity, mut io)) = query.get_mut(trigger.target()) {
            let config = ServerConfig::builder()
                .with_bind_address(io.server_addr)
                .with_identity(io.certificate.clone_identity())
                .keep_alive_interval(Some(Duration::from_secs(1)))
                .max_idle_timeout(Some(Duration::from_secs(5)))
                .expect("should be a valid idle timeout")
                .build();
            info!("Server WebTransport starting at {}", io.server_addr);
            commands.entity(entity).queue(move |entity_mut: EntityWorldMut| {
                open(entity_mut, config);
            });
        }
    }

    // TODO: add unlink
}

fn open(mut entity: EntityWorldMut, config: ServerConfig) {
    let runtime = entity.world().resource::<WebTransportRuntime>().clone();
    let (send_closed, recv_closed) = oneshot::channel::<Closed>();
    let (send_next, recv_next) = oneshot::channel::<ToOpen>();
    runtime.spawn_on_self(
        async move {
            let Err(closed) = backend::start(config, send_next).await;
            debug!("Server closed: {closed:?}");
            _ = send_closed.send(closed);
        }
        .instrument(debug_span!("server", entity = %entity.id())),
    );

    entity.insert((
        Linking,
        Opening {
            recv_closed,
            recv_next,
        },
    ));
}

/// How should a [`WebTransportServer`] respond to a client wishing to connect
/// to the server?
///
/// See [`SessionRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Reflect)]
pub enum SessionResponse {
    /// Allow the client to connect to the server.
    Accepted,
    /// Reject the client with a `403 Forbidden`.
    Forbidden,
    /// Reject the client with a `404 Not Found`.
    NotFound,
}

/// Triggered when a client requests to connect to a [`WebTransportServer`].
///
/// Use the fields in this event to decide whether to accept the client's
/// connection or not, and respond accordingly by calling
/// [`SessionRequest::respond`].
///
/// At least one of your observers must `respond` to this request, otherwise
/// the server will default to [`SessionResponse::NotFound`].
///
/// # Examples
///
/// Accept all clients without any extra checks:
///
/// ```
/// use {
///     aeronet_webtransport::server::{SessionRequest, SessionResponse},
///     bevy_ecs::prelude::*,
/// };
///
/// fn on_session_request(mut trigger: Trigger<SessionRequest>) {
///     let client = trigger.target();
///     trigger.respond(SessionResponse::Accepted);
/// }
/// ```
///
/// Check if the client has a given header before accepting them:
///
/// ```
/// use {
///     aeronet_webtransport::server::{SessionRequest, SessionResponse},
///     bevy_ecs::prelude::*,
/// };
///
/// fn on_session_request(mut request: Trigger<SessionRequest>) {
///     let mut response = SessionResponse::Forbidden;
///     if let Some(auth_token) = request.headers.get(":auth-token") {
///         if validate_auth_token(auth_token) {
///             response = SessionResponse::Accepted;
///         }
///     }
///
///     request.respond(response);
/// }
/// # fn validate_auth_token(_: &str) -> bool { unimplemented!() }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Event, Reflect)]
pub struct SessionRequest {
    /// `:authority` header.
    pub authority: String,
    /// `:path` header.
    pub path: String,
    /// `origin` header.
    pub origin: Option<String>,
    /// `user-agent` header.
    pub user_agent: Option<String>,
    /// Full map of request headers.
    pub headers: std::collections::HashMap<String, String>,
    /// How should the server respond to this request?
    pub response: Option<SessionResponse>,
}

impl SessionRequest {
    /// Sets how the server should respond to this request.
    pub const fn respond(&mut self, response: SessionResponse) {
        self.response = Some(response);
    }
}

/// [`WebTransportServer`]-specific error.
///
/// For generic WebTransport errors, see [`SessionError`].
#[derive(Debug, Display, Error)]
#[non_exhaustive]
pub enum ServerError {
    /// Failed to await an incoming session request.
    #[display("failed to await session request")]
    AwaitSessionRequest(ConnectionError),
    /// User rejected this incoming session request.
    #[display("user rejected session request")]
    Rejected,
    /// Failed to accept the incoming session request.
    #[display("failed to accept session")]
    AcceptSessionRequest(ConnectionError),
}

#[derive(Debug, Component)]
struct Opening {
    recv_closed: oneshot::Receiver<Closed>,
    recv_next: oneshot::Receiver<ToOpen>,
}

#[derive(Debug, Component)]
struct Opened {
    recv_closed: oneshot::Receiver<Closed>,
    recv_connecting: mpsc::Receiver<ToConnecting>,
}

#[derive(Debug, Component)]
struct Connecting {
    recv_dc: oneshot::Receiver<Disconnected>,
    recv_next: oneshot::Receiver<ToConnected>,
}

#[derive(Debug, Component)]
struct Connected {
    recv_dc: oneshot::Receiver<Disconnected>,
}

#[derive(Debug)]
struct ToOpen {
    local_addr: SocketAddr,
    recv_connecting: mpsc::Receiver<ToConnecting>,
}

#[derive(Debug)]
struct ToConnecting {
    authority: String,
    path: String,
    origin: Option<String>,
    user_agent: Option<String>,
    headers: std::collections::HashMap<String, String>,
    send_session_entity: oneshot::Sender<Entity>,
    send_session_response: oneshot::Sender<SessionResponse>,
    recv_dc: oneshot::Receiver<Disconnected>,
    recv_next: oneshot::Receiver<ToConnected>,
}

#[derive(Debug)]
struct ToConnected {
    initial_peer_addr: SocketAddr,
    initial_rtt: Duration,
    initial_mtu: usize,
    recv_meta: mpsc::Receiver<SessionMeta>,
    recv_packet_b2f: mpsc::UnboundedReceiver<RecvPayload>,
    send_packet_f2b: mpsc::UnboundedSender<SendPayload>,
    send_user_dc: oneshot::Sender<String>,
}

fn poll_opening(
    mut commands: Commands,
    mut servers: Query<(Entity, &mut Opening), With<WebTransportServer>>,
) {
    for (entity, mut server) in &mut servers {
        if try_close(&mut commands, entity, &mut server.recv_closed) {
            continue;
        }

        let Ok(Some(next)) = server.recv_next.try_recv() else {
            continue;
        };

        let (_, dummy) = oneshot::channel();
        let recv_closed = mem::replace(&mut server.recv_closed, dummy);
        commands.entity(entity).remove::<Opening>().insert((
            Linked,
            Opened {
                recv_closed,
                recv_connecting: next.recv_connecting,
            },
        ));
    }
}

fn poll_opened(
    mut commands: Commands,
    mut servers: Query<(Entity, &mut Opened, &WebTransportServer), With<WebTransportServer>>,
) {
    for (entity, mut server, webtransport_server) in &mut servers {
        if try_close(&mut commands, entity, &mut server.recv_closed) {
            continue;
        }

        while let Ok(Some(connecting)) = server.recv_connecting.try_next() {
            let mut link = Link::new(webtransport_server.server_addr, None);
            link.local_addr = Some(webtransport_server.server_addr);
            let client = commands
                .spawn((
                    LinkOf {
                        server: entity,
                    },
                    // TODO: required for Netcode for now
                    ClientOf {
                        server: entity,
                    },
                    Linking,
                    link,
                    Connecting {
                        recv_dc: connecting.recv_dc,
                        recv_next: connecting.recv_next,
                    },
                ))
                .id();
            _ = connecting.send_session_entity.send(client);

            commands.queue(move |world: &mut World| {
                let mut request = SessionRequest {
                    authority: connecting.authority,
                    path: connecting.path,
                    origin: connecting.origin,
                    user_agent: connecting.user_agent,
                    headers: connecting.headers,
                    response: None,
                };
                world.trigger_targets_ref(&mut request, client);

                let response = request.response.unwrap_or_else(|| {
                    warn!(
                        "Client session {client} created on server {entity} but no response was \
                         given, will not allow this client to connect; you must `respond` to `{}`",
                        type_name::<SessionRequest>()
                    );
                    SessionResponse::NotFound
                });
                _ = connecting.send_session_response.send(response);
            });
        }
    }
}

fn try_close(
    commands: &mut Commands,
    entity: Entity,
    recv_closed: &mut oneshot::Receiver<Closed>,
) -> bool {
    let closed = match recv_closed.try_recv() {
        Ok(None) => None,
        Ok(Some(closed)) => Some(closed),
        Err(_) => Some(SessionError::BackendClosed.into()),
    };
    closed.is_some_and(|closed| {
        if let Ok(mut entity_mut) = commands.get_entity(entity) {
            match closed {
                Closed::ByUser(reason) => {
                    entity_mut
                        .remove::<(Opening, Opened)>()
                        .trigger(
                        Unlink { reason: reason.clone() },
                    );
                }
                Closed::ByError(error) => {
                    entity_mut
                        .remove::<(Opening, Opened)>()
                        .trigger(
                        Unlink { reason: format!("Error: {:?}", error) },
                    );
                }
            }
        }
        true
    })
}

#[derive(Component)]
struct WebTransportClientOf;

fn poll_connecting(
    mut commands: Commands,
    mut clients: Query<(Entity, &mut Connecting, &mut Link), With<WebTransportClientOf>>,
) {
    for (entity, mut client, mut link) in &mut clients {
        if try_disconnect(&mut commands, entity, &mut client.recv_dc) {
            continue;
        }

        let Ok(Some(next)) = client.recv_next.try_recv() else {
            continue;
        };

        link.remote_addr = Some(next.initial_peer_addr);
        let (_, dummy) = oneshot::channel();
        let recv_dc = mem::replace(&mut client.recv_dc, dummy);
        commands.entity(entity).remove::<Connecting>().insert((
            WebTransportIo {
                recv_meta: next.recv_meta,
                recv_packet_b2f: next.recv_packet_b2f,
                send_packet_f2b: next.send_packet_f2b,
                send_user_dc: Some(next.send_user_dc),
            },
            WebTransportClientOf,
            Connected { recv_dc },
            Linked,
        ));
    }
}

fn poll_connected(
    mut commands: Commands,
    mut clients: Query<(Entity, &mut Connected), With<WebTransportClientOf>>,
) {
    for (entity, mut client) in &mut clients {
        try_disconnect(&mut commands, entity, &mut client.recv_dc);
    }
}

fn try_disconnect(
    commands: &mut Commands,
    entity: Entity,
    recv_dc: &mut oneshot::Receiver<Disconnected>,
) -> bool {
    let disconnected = match recv_dc.try_recv() {
        Ok(None) => None,
        Ok(Some(disconnected)) => Some(disconnected),
        Err(_) => Some(SessionError::BackendClosed.into()),
    };
    disconnected.is_some_and(|disconnected| {
        if let Ok(mut entity_mut) = commands.get_entity(entity) {
            entity_mut.remove::<(Connecting, Connected)>()
                .trigger(Unlink { reason: format!("{disconnected:?}") });
        }
        true
    })
}
