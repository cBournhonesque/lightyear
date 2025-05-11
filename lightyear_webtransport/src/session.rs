//! Implementation for WebTransport sessions, shared between clients and
//! servers.

use bevy_time::prelude::{Real, Time};
use lightyear_link::{Link, LinkPlugin, LinkSet, RecvPayload, SendPayload, Unlink};
use {
    crate::runtime::WebTransportRuntime,
    aeronet_io::{
        connection::{Disconnect, Disconnected, PeerAddr, DROP_DISCONNECT_REASON}, packet::{MtuTooSmall, PacketRtt, RecvPacket, IP_MTU}, AeronetIoPlugin,
        IoSet,
        Session,
    },
    alloc::sync::Arc,
    bevy_app::prelude::*,
    bevy_ecs::prelude::*,
    bevy_platform::time::Instant,
    bytes::Bytes,
    core::{num::Saturating, time::Duration},
    derive_more::{Display, Error},
    futures::{
        channel::{mpsc, oneshot}, never::Never, FutureExt,
        SinkExt,
        StreamExt,
    },
    std::io,
    tracing::{trace, trace_span},
    xwt_core::prelude::*,
};

cfg_if::cfg_if! {
    if #[cfg(target_family = "wasm")] {
        type Connection = xwt_web::Session;
        type ConnectionError = crate::JsError;
    } else {
        type Connection = xwt_wtransport::Connection;
        type ConnectionError = wtransport::error::ConnectionError;
    }
}

pub(crate) struct WebTransportSessionPlugin;

impl Plugin for WebTransportSessionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<LinkPlugin>() {
            app.add_plugins(LinkPlugin);
        }

        #[cfg(not(target_family = "wasm"))]
        {
            if wtransport::tls::rustls::crypto::ring::default_provider()
                .install_default()
                .is_ok()
            {
                tracing::debug!("Installed default `ring` CryptoProvider");
            } else {
                tracing::debug!("CryptoProvider is already installed");
            }
        }

        app.init_resource::<WebTransportRuntime>()
            .add_systems(PreUpdate, receive.in_set(LinkSet::Receive))
            .add_systems(PostUpdate, send.in_set(LinkSet::Send))
            .add_observer(on_disconnect);
    }
}

/// Manages a WebTransport session's connection.
///
/// This may represent either an outgoing client connection (this session is
/// connecting to a server), or an incoming client connection (this session is
/// a child of a server that the user has spawned).
///
/// You should not add or remove this component directly - it is managed
/// entirely by the client and server implementations.
#[derive(Debug, Component)]
#[require(Link)]
pub struct WebTransportIo {
    pub(crate) recv_meta: mpsc::Receiver<SessionMeta>,
    pub(crate) recv_packet_b2f: mpsc::UnboundedReceiver<RecvPayload>,
    pub(crate) send_packet_f2b: mpsc::UnboundedSender<SendPayload>,
    pub(crate) send_user_dc: Option<oneshot::Sender<String>>,
}

/// Minimum packet MTU that a [`WebTransportIo`] must support.
pub const MIN_MTU: usize = IP_MTU;

/// Error that occurs when polling a session using the [`WebTransportIo`]
/// IO layer.
#[derive(Debug, Display, Error)]
#[non_exhaustive]
pub enum SessionError {
    /// Frontend ([`WebTransportIo`]) was dropped.
    #[display("frontend closed")]
    FrontendClosed,
    /// Backend async task was unexpectedly cancelled and dropped.
    #[display("backend closed")]
    BackendClosed,
    /// Failed to create endpoint.
    #[display("failed to create endpoint")]
    CreateEndpoint(io::Error),
    /// Failed to read the local socket address of the endpoint.
    #[display("failed to get local socket address")]
    GetLocalAddr(io::Error),
    /// Successfully connected to the peer, but this connection does not support
    /// datagrams.
    #[display("datagrams not supported")]
    DatagramsNotSupported,
    /// Packet MTU is smaller than [`MIN_MTU`].
    ///
    /// This may occur either immediately after connecting to the peer, or after
    /// a connection has been established and the path MTU updates.
    MtuTooSmall(MtuTooSmall),
    /// Unexpectedly lost connection from the peer.
    #[display("connection lost")]
    Connection(ConnectionError),
}

impl Drop for WebTransportIo {
    fn drop(&mut self) {
        if let Some(send_dc) = self.send_user_dc.take() {
            _ = send_dc.send(DROP_DISCONNECT_REASON.to_owned());
        }
    }
}

#[derive(Debug)]
pub(crate) struct SessionMeta {
    #[cfg(not(target_family = "wasm"))]
    peer_addr: core::net::SocketAddr,
    #[cfg(not(target_family = "wasm"))]
    packet_rtt: Duration,
    mtu: usize,
}

fn on_disconnect(trigger: Trigger<Unlink>, mut sessions: Query<&mut WebTransportIo>) {
    let target = trigger.target();
    let Ok(mut io) = sessions.get_mut(target) else {
        return;
    };

    if let Some(send_dc) = io.send_user_dc.take() {
        _ = send_dc.send(trigger.reason.clone());
    }
}

pub(crate) fn receive(
    time: Res<Time<Real>>,
    mut sessions: Query<(
        Entity,
        &mut Link,
        &mut WebTransportIo,
    )>,
) {
    for (entity, mut link, mut io) in &mut sessions {
        #[cfg(target_family = "wasm")]
        {
            // suppress `unused_variables`, `unused_mut`
            _ = (&mut peer_addr, &mut packet_rtt);
        }

        let span = trace_span!("poll", %entity);
        let _span = span.enter();

        while let Ok(Some(meta)) = io.recv_meta.try_next() {
            #[cfg(not(target_family = "wasm"))]
            {
                link.remote_addr = Some(meta.peer_addr);
            }
        }

        while let Ok(Some(packet)) = io.recv_packet_b2f.try_next() {
            link.recv.push(packet, time.elapsed());
        }
    }
}

fn send(mut sessions: Query<(Entity, &mut Link, &WebTransportIo)>) {
    for (entity, mut link, io) in &mut sessions {
        let span = trace_span!("flush", %entity);
        let _span = span.enter();

        for packet in link.send.drain() {
            // handle connection errors in `poll`
            _ = io.send_packet_f2b.unbounded_send(packet);
        }

    }
}

#[derive(Debug)]
pub(crate) struct SessionBackend {
    pub conn: Connection,
    pub send_meta: mpsc::Sender<SessionMeta>,
    pub send_packet_b2f: mpsc::UnboundedSender<RecvPayload>,
    pub recv_packet_f2b: mpsc::UnboundedReceiver<SendPayload>,
    pub recv_user_dc: oneshot::Receiver<String>,
}

impl SessionBackend {
    pub async fn start(self) -> Disconnected {
        let Self {
            conn,
            send_meta,
            send_packet_b2f,
            recv_packet_f2b,
            mut recv_user_dc,
        } = self;

        let conn = Arc::new(conn);
        let (send_err, mut recv_err) = mpsc::channel::<SessionError>(1);

        let (_send_meta_closed, recv_meta_closed) = oneshot::channel();
        WebTransportRuntime::spawn({
            let conn = conn.clone();
            let mut send_err = send_err.clone();
            async move {
                let Err(err) = meta_loop(conn, recv_meta_closed, send_meta).await;
                _ = send_err.try_send(err);
            }
        });

        let (_send_receiving_closed, recv_receiving_closed) = oneshot::channel();
        WebTransportRuntime::spawn({
            let conn = conn.clone();
            let mut send_err = send_err.clone();
            async move {
                let Err(err) = recv_loop(conn, recv_receiving_closed, send_packet_b2f).await;
                _ = send_err.try_send(err);
            }
        });

        let (_send_sending_closed, recv_sending_closed) = oneshot::channel();
        WebTransportRuntime::spawn({
            let conn = conn.clone();
            let mut send_err = send_err.clone();
            async move {
                let Err(err) = send_loop(conn, recv_sending_closed, recv_packet_f2b).await;
                _ = send_err.try_send(err);
            }
        });

        futures::select! {
            err = recv_err.next() => {
                let err = err.unwrap_or(SessionError::BackendClosed);
                get_disconnect_reason(err)
            }
            reason = recv_user_dc => {
                if let Ok(reason) = reason {
                    disconnect(conn, &reason).await;
                    Disconnected::by_user(reason)
                } else {
                    Disconnected::by_error(SessionError::FrontendClosed)
                }
            }
        }
    }
}

async fn meta_loop(
    conn: Arc<Connection>,
    mut recv_closed: oneshot::Receiver<()>,
    mut send_meta: mpsc::Sender<SessionMeta>,
) -> Result<Never, SessionError> {
    const META_UPDATE_INTERVAL: Duration = Duration::from_millis(100);

    loop {
        futures::select! {
            () = WebTransportRuntime::sleep(META_UPDATE_INTERVAL).fuse() => {},
            _ = recv_closed => return Err(SessionError::FrontendClosed),
        };

        let meta = SessionMeta {
            #[cfg(not(target_family = "wasm"))]
            peer_addr: conn.0.remote_address(),
            #[cfg(not(target_family = "wasm"))]
            packet_rtt: conn.0.rtt(),
            mtu: conn
                .max_datagram_size()
                .ok_or(SessionError::DatagramsNotSupported)?,
        };
        match send_meta.try_send(meta) {
            Ok(()) => {}
            Err(err) if err.is_full() => {}
            Err(_) => {
                return Err(SessionError::FrontendClosed);
            }
        }
    }
}

async fn recv_loop(
    conn: Arc<Connection>,
    mut recv_closed: oneshot::Receiver<()>,
    mut send_packet_b2f: mpsc::UnboundedSender<RecvPayload>,
) -> Result<Never, SessionError> {
    loop {
        #[cfg_attr(
            not(target_family = "wasm"),
            expect(clippy::useless_conversion, reason = "conversion required for WASM")
        )]
        let packet = futures::select! {
            x = conn.receive_datagram().fuse() => x,
            _ = recv_closed => return Err(SessionError::FrontendClosed),
        }
        .map_err(|err| SessionError::Connection(err.into()))?;

        let packet = {
            #[cfg(target_family = "wasm")]
            {
                Bytes::from(packet)
            }

            #[cfg(not(target_family = "wasm"))]
            {
                packet.0.payload()
            }
        };
        send_packet_b2f
            .send(packet)
            .await
            .map_err(|_| SessionError::BackendClosed)?;
    }
}

async fn send_loop(
    conn: Arc<Connection>,
    mut recv_closed: oneshot::Receiver<()>,
    mut recv_packet_f2b: mpsc::UnboundedReceiver<Bytes>,
) -> Result<Never, SessionError> {
    loop {
        let packet = futures::select! {
            x = recv_packet_f2b.next() => x,
            _ = recv_closed => return Err(SessionError::FrontendClosed),
        }
        .ok_or(SessionError::FrontendClosed)?;

        #[cfg(target_family = "wasm")]
        {
            conn.send_datagram(packet)
                .await
                .map_err(|err| SessionError::Connection(err.into()))?;
        }

        #[cfg(not(target_family = "wasm"))]
        {
            use wtransport::error::SendDatagramError;

            let packet_len = packet.len();
            match conn.send_datagram(packet).await {
                Ok(()) => Ok(()),
                Err(SendDatagramError::NotConnected) => {
                    // we'll pick up connection errors in the recv loop,
                    // where we'll get a better error message
                    Ok(())
                }
                Err(SendDatagramError::TooLarge) => {
                    // the backend constantly informs the frontend about changes in the path MTU
                    // so hopefully the frontend will realise its packets are exceeding MTU,
                    // and shrink them accordingly; therefore this is just a one-off error
                    let mtu = conn.max_datagram_size();
                    tracing::debug!(
                        packet_len,
                        mtu,
                        "Attempted to send datagram larger than MTU"
                    );
                    Ok(())
                }
                Err(SendDatagramError::UnsupportedByPeer) => {
                    // this should be impossible, since we checked that the client does support
                    // datagrams before connecting, but we'll error-case it anyway
                    Err(SessionError::DatagramsNotSupported)
                }
            }?;
        }
    }
}

fn get_disconnect_reason(err: SessionError) -> Disconnected {
    #[cfg(target_family = "wasm")]
    {
        // TODO: I don't know how the app-initiated disconnect message looks
        // I suspect we need this fixed first
        // https://github.com/BiagioFesta/wtransport/issues/182
        //
        // Tested: when the server disconnects us, all we get is "Connection lost."
        Disconnected::by_error(err)
    }

    #[cfg(not(target_family = "wasm"))]
    {
        use wtransport::error::ConnectionError;

        match err {
            SessionError::Connection(ConnectionError::ApplicationClosed(err)) => {
                Disconnected::by_peer(String::from_utf8_lossy(err.reason()))
            }
            err => Disconnected::by_error(err),
        }
    }
}

async fn disconnect(conn: Arc<Connection>, reason: &str) {
    const DISCONNECT_ERROR_CODE: u32 = 0;

    #[cfg(target_family = "wasm")]
    {
        use {js_sys::JsString, xwt_web::web_wt_sys::WebTransportCloseInfo};

        let close_info = WebTransportCloseInfo::new();
        close_info.set_close_code(DISCONNECT_ERROR_CODE);
        close_info.set_reason(JsString::from(reason));

        // TODO: This seems to not close the connection properly
        // Could it be because of this?
        // https://github.com/BiagioFesta/wtransport/issues/182
        //
        // Tested: the server times us out instead of actually
        // reading the disconnect
        conn.transport.close_with_info(&close_info);
        _ = conn.transport.closed().await;
    }

    #[cfg(not(target_family = "wasm"))]
    {
        use wtransport::VarInt;

        const ERROR_CODE: VarInt = VarInt::from_u32(DISCONNECT_ERROR_CODE);

        conn.0.close(ERROR_CODE, reason.as_bytes());
        conn.0.closed().await;
    }
}
