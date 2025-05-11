//! See [`WebTransportClient`].

mod backend;

use crate::cert;
use aeronet_io::connection::Disconnected;
use lightyear_link::{Link, LinkStart, Linked, Linking, RecvPayload, SendPayload, Unlink, Unlinked};
use std::net::ToSocketAddrs;
use wtransport::endpoint::IntoConnectOptions;
use {
    crate::{
        runtime::WebTransportRuntime,
        session::{
            self, SessionError, SessionMeta, WebTransportIo, WebTransportSessionPlugin, MIN_MTU,
        },
    },
    bevy_app::prelude::*,
    bevy_ecs::error::{BevyError, Result},
    bevy_ecs::{prelude::*, system::EntityCommand},
    bevy_platform::time::Instant,
    bytes::Bytes,
    core::mem,
    derive_more::{Display, Error},
    futures::channel::{mpsc, oneshot},
    lightyear_link::LinkSet,
    tracing::{debug, debug_span, Instrument},
};

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        /// Configuration for the [`WebTransportClient`] on WASM platforms.
        pub type ClientConfig = xwt_web::WebTransportOptions;

        type ConnectTarget = String;

        type ConnectError = crate::JsError;
        type AwaitConnectError = crate::JsError;
    } else {
        use wtransport::endpoint::endpoint_side;
        use xwt_core::endpoint::{Connect as XwtConnect, connect::Connecting as XwtConnecting};

        /// Configuration for the [`WebTransportClient`] on non-WASM platforms.
        pub type ClientConfig = wtransport::ClientConfig;

        type ConnectTarget = wtransport::endpoint::ConnectOptions;
        type ClientEndpoint = xwt_wtransport::Endpoint<endpoint_side::Client>;

        type ConnectError = <ClientEndpoint as XwtConnect>::Error;
        type AwaitConnectError = <<ClientEndpoint as XwtConnect>::Connecting as XwtConnecting>::Error;
    }
}

/// Allows using [`WebTransportClient`].
pub struct WebTransportClientPlugin;

impl Plugin for WebTransportClientPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<WebTransportSessionPlugin>() {
            app.add_plugins(WebTransportSessionPlugin);
        }

        app.add_observer(WebTransportClient::link);
        app.add_systems(
            PreUpdate,
            (poll_connecting, poll_connected)
                .in_set(LinkSet::Receive)
                .before(session::receive),
        );
    }
}

#[derive(thiserror::Error, Debug)]
pub enum WebTransportError {
    #[error("the certificate hash `{0}` is invalid")]
    Certificate(String),
}

/// WebTransport session implementation which acts as a dedicated client,
/// connecting to a target endpoint.
///
/// Use [`WebTransportClient::connect`] to start a connection.
#[derive(Debug, Component)]
#[require(Link)]
pub struct WebTransportClient {
    pub server_addr: core::net::SocketAddr,
    pub certificate_digest: String,
}

impl WebTransportClient {
    /// Creates an [`EntityCommand`] to set up a session and connect it to the
    /// `target`.
    ///
    /// # Examples
    ///
    /// ```
    /// use {
    ///     aeronet_webtransport::client::{ClientConfig, WebTransportClient},
    ///     bevy_ecs::{prelude::*, system::EntityCommand},
    /// };
    ///
    /// # fn run(mut commands: Commands, world: &mut World) {
    /// let config = ClientConfig::default();
    /// let target = "https://[::1]:1234";
    ///
    /// // using `Commands`
    /// commands
    ///     .spawn_empty()
    ///     .queue(WebTransportClient::connect(config, target));
    ///
    /// // using mutable `World` access
    /// # let config: ClientConfig = unreachable!();
    /// let client = world.spawn_empty().id();
    /// WebTransportClient::connect(config, target).apply(world.entity_mut(client));
    /// # }
    /// ```
    #[must_use]
    pub fn link(
        trigger: Trigger<LinkStart>,
        mut query: Query<(Entity, &mut WebTransportClient), With<Unlinked>>,
        mut commands: Commands,
    ) -> Result {
        if let Ok((entity, mut client)) = query.get_mut(trigger.target()) {
            let config = Self::client_config(client.certificate_digest.clone())?;
            let server_url = format!("https://{}", client.server_addr);
            let target = {
                #[cfg(target_family = "wasm")]
                {
                    server_url.into()
                }

                #[cfg(not(target_family = "wasm"))]
                {
                    server_url.into_options()
                }
            };
            commands.entity(entity).queue(move |entity_mut: EntityWorldMut| {
                connect(entity_mut, config, target);
            });
        }
        Ok(())
    }

    // TODO: add unlink

    #[cfg(target_family = "wasm")]
    fn client_config(cert_hash: String) -> Result<ClientConfig> {
        use xwt_web::{CertificateHash, HashAlgorithm};

        let server_certificate_hashes = if cert_hash.is_empty() {
            Vec::new()
        } else {
            match cert::hash_from_b64(&cert_hash) {
                Ok(hash) => vec![CertificateHash {
                    algorithm: HashAlgorithm::Sha256,
                    value: Vec::from(hash),
                }],
                Err(err) => {
                    WebTransportError::Certificate("Failed to read certificate hash from string: {err:?}".to_string())?
                }
            }
        };

        Ok(ClientConfig {
            server_certificate_hashes,
            ..Default::default()
        })
    }

    #[cfg(not(target_family = "wasm"))]
    fn client_config(cert_hash: String) -> Result<ClientConfig> {
        use {core::time::Duration, wtransport::tls::Sha256Digest};

        let config = ClientConfig::builder().with_bind_default();

        let config = if cert_hash.is_empty() {
            #[cfg(feature = "dangerous-configuration")]
            {
                warn!("Connecting with no certificate validation");
                config.with_no_cert_validation()
            }
            #[cfg(not(feature = "dangerous-configuration"))]
            {
                config.with_server_certificate_hashes([])
            }
        } else {
            let hash = cert::hash_from_b64(&cert_hash)?;
            config.with_server_certificate_hashes([Sha256Digest::new(hash)])
        };

        Ok(config
            .keep_alive_interval(Some(Duration::from_secs(1)))
            .max_idle_timeout(Some(Duration::from_secs(5)))
            .expect("should be a valid idle timeout")
            .build())
    }
}

fn connect(mut entity: EntityWorldMut, config: ClientConfig, target: ConnectTarget) {
    let runtime = entity.world().resource::<WebTransportRuntime>().clone();
    let (send_dc, recv_dc) = oneshot::channel::<Disconnected>();
    let (send_next, recv_next) = oneshot::channel::<ToConnected>();
    runtime.spawn_on_self(
        async move {
            let Err(disconnected) = backend::start(config, target, send_next).await;
            debug!("Client disconnected: {disconnected:?}");
            _ = send_dc.send(disconnected);
        }
        .instrument(debug_span!("client", entity = %entity.id())),
    );

    entity.insert((Connecting { recv_dc, recv_next }, Linking));
}

/// [`WebTransportClient`]-specific error.
///
/// For generic WebTransport errors, see [`SessionError`].
#[derive(Debug, Display, Error)]
#[non_exhaustive]
pub enum ClientError {
    /// Failed to start connecting to the target.
    #[display("failed to connect")]
    Connect(ConnectError),
    /// Failed to await the connection to the target.
    #[display("failed to await connection")]
    AwaitConnect(AwaitConnectError),
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
struct ToConnected {
    #[cfg(not(target_family = "wasm"))]
    local_addr: core::net::SocketAddr,
    #[cfg(not(target_family = "wasm"))]
    initial_peer_addr: core::net::SocketAddr,
    #[cfg(not(target_family = "wasm"))]
    initial_rtt: core::time::Duration,
    initial_mtu: usize,
    recv_meta: mpsc::Receiver<SessionMeta>,
    recv_packet_b2f: mpsc::UnboundedReceiver<RecvPayload>,
    send_packet_f2b: mpsc::UnboundedSender<SendPayload>,
    send_user_dc: oneshot::Sender<String>,
}

fn poll_connecting(
    mut commands: Commands,
    mut clients: Query<(Entity, &mut Connecting, &mut Link), With<WebTransportClient>>,
) {
    for (entity, mut client, mut link) in &mut clients {
        if try_disconnect(&mut commands, entity, &mut client.recv_dc) {
            continue;
        }

        let Ok(Some(next)) = client.recv_next.try_recv() else {
            continue;
        };

        let (_, dummy) = oneshot::channel();
        let recv_dc = mem::replace(&mut client.recv_dc, dummy);
        #[cfg(not(target_family = "wasm"))]
        {
            link.local_addr = Some(next.local_addr);
            link.remote_addr = Some(next.initial_peer_addr);
        }
        commands.entity(entity).remove::<Connecting>().insert((
            Linked,
            WebTransportIo {
                recv_meta: next.recv_meta,
                recv_packet_b2f: next.recv_packet_b2f,
                send_packet_f2b: next.send_packet_f2b,
                send_user_dc: Some(next.send_user_dc),
            },
            Connected { recv_dc },
        ));
    }
}

fn poll_connected(
    mut commands: Commands,
    mut clients: Query<(Entity, &mut Connected), With<WebTransportClient>>,
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
        commands.trigger_targets(Unlink {
            reason: "disconnected".to_string(),
        }, entity);
        true
    })
}
